//! MAC-based LAN policy routing shared by the gateway features. Each feature owns
//! a distinct routing table, mark bit and chain prefix; scripts are literal
//! templates and never contain unvalidated configuration.
use crate::{process::BoundedCommand, storage};
use serde::Deserialize;
use std::{path::Path, process::Command, sync::Mutex, time::Duration};

/// Serializes gateway updates so only one feature can own LAN traffic at a time.
pub static GATE: Mutex<()> = Mutex::new(());

pub struct Spec {
    /// User-facing feature name used in error messages.
    pub name: &'static str,
    pub prefix: &'static str,
    pub table: &'static str,
    pub mark: &'static str,
    pub dir: &'static str,
    pub kind: Kind,
}

/// How marked LAN traffic leaves the router.
pub enum Kind {
    /// Routed out of a kernel tunnel interface with source NAT.
    Tunnel { iface: &'static str },
    /// Userspace proxy: TCP is NAT-redirected to a local listener on `port`; UDP
    /// and everything else is routed into the proxy's TUN interface. Both survive
    /// bridge netfilter, unlike TPROXY socket assignment.
    Proxy { iface: &'static str, port: u16 },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceChoice {
    pub mac: String,
    pub selected: bool,
}

pub fn mac(s: &str) -> bool {
    s.len() == 17
        && s.split(':').count() == 6
        && s.split(':')
            .all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_hexdigit()))
        && u8::from_str_radix(&s[..2], 16).is_ok_and(|b| b & 1 == 0)
        && s != "00:00:00:00:00:00"
}

/// Record per-device choices relative to the routing mode (exclusions in "all").
pub fn apply_devices(
    mode: &str,
    macs: &mut Vec<String>,
    choices: Vec<DeviceChoice>,
) -> Result<(), String> {
    if choices.len() > 128 {
        return Err("Too many device changes".into());
    }
    for c in choices {
        let m = c.mac.to_lowercase();
        if !mac(&m) {
            return Err("Invalid device MAC".into());
        }
        macs.retain(|v| v != &m);
        if c.selected == (mode == "selected") {
            macs.push(m);
        }
    }
    Ok(())
}

pub fn validate_selection(mode: &str, macs: &[String]) -> Result<(), String> {
    if mode != "all" && mode != "selected" {
        return Err("Invalid routing mode".into());
    }
    if macs.len() > 128 || macs.iter().any(|s| !mac(s)) {
        return Err("Invalid device MAC selection (maximum 128)".into());
    }
    Ok(())
}

pub fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let o = Command::new(program)
        .args(args)
        .bounded_output()
        .map_err(|_| format!("Cannot run {program}"))?;
    if !o.status.success() {
        return Err(format!("{program} failed"));
    }
    Ok(String::from_utf8_lossy(&o.stdout).trim().into())
}

// Templates are written with the WireGuard identifiers; `render` substitutes the
// identifiers of the feature that owns the policy.
const CLEANUP: &str = r#"
for spec in 'iptables mangle PREROUTING OUWG_MARK' 'iptables nat PREROUTING OUWG_DNS' 'iptables nat POSTROUTING OUWG_NAT' 'iptables filter FORWARD OUWG_FWD' 'iptables mangle FORWARD OUWG_MSS' 'ip6tables filter FORWARD OUWG_V6' 'ip6tables filter INPUT OUWG_DNS6' 'iptables filter INPUT OUWG_DNS4'; do
 set -- $spec
 while "$1" -w 2 -t "$2" -C "$3" -j "$4" 2>/dev/null; do "$1" -w 2 -t "$2" -D "$3" -j "$4" || exit 1; done
 "$1" -w 2 -t "$2" -F "$4" 2>/dev/null || true
 "$1" -w 2 -t "$2" -X "$4" 2>/dev/null || true
done
while ip -4 rule del priority 10420 fwmark 0x40000000/0x40000000 table 10420 2>/dev/null; do :; done
ip -4 route flush table 10420 2>/dev/null || true
ip link del ou-wg0 2>/dev/null || true
"#;
// During a policy replacement, briefly pause forwarded LAN traffic and client DNS.
// This guard remains if the agent is interrupted mid-transaction. LAN management
// and the router's own WAN connection remain available for recovery.
const HOLD: &str = r#"
set -eu
for bin in iptables ip6tables; do
 $bin -w 2 -N OUWG_HOLD 2>/dev/null || true
 $bin -w 2 -C OUWG_HOLD -i br-lan ! -o br-lan -j REJECT 2>/dev/null || $bin -w 2 -A OUWG_HOLD -i br-lan ! -o br-lan -j REJECT
 $bin -w 2 -C FORWARD -j OUWG_HOLD 2>/dev/null || $bin -w 2 -I FORWARD 1 -j OUWG_HOLD
 $bin -w 2 -N OUWG_HD 2>/dev/null || true
 for proto in udp tcp; do
 $bin -w 2 -C OUWG_HD -i br-lan -p $proto --dport 53 -j REJECT 2>/dev/null || $bin -w 2 -A OUWG_HD -i br-lan -p $proto --dport 53 -j REJECT
 done
 $bin -w 2 -C INPUT -j OUWG_HD 2>/dev/null || $bin -w 2 -I INPUT 1 -j OUWG_HD
done
"#;
const RELEASE: &str = r#"
set -eu
for bin in iptables ip6tables; do
 while $bin -w 2 -C FORWARD -j OUWG_HOLD 2>/dev/null; do $bin -w 2 -D FORWARD -j OUWG_HOLD; done
 while $bin -w 2 -C INPUT -j OUWG_HD 2>/dev/null; do $bin -w 2 -D INPUT -j OUWG_HD; done
 $bin -w 2 -F OUWG_HOLD 2>/dev/null || true
 $bin -w 2 -X OUWG_HOLD 2>/dev/null || true
 $bin -w 2 -F OUWG_HD 2>/dev/null || true
 $bin -w 2 -X OUWG_HD 2>/dev/null || true
done
"#;
const HEALTH: &str = r#"
set -e
for spec in 'iptables mangle PREROUTING OUWG_MARK' 'iptables nat PREROUTING OUWG_DNS' 'iptables nat POSTROUTING OUWG_NAT' 'iptables filter FORWARD OUWG_FWD' 'ip6tables filter FORWARD OUWG_V6' 'iptables filter INPUT OUWG_DNS4' 'ip6tables filter INPUT OUWG_DNS6'; do
 set -- $spec
 "$1" -w 2 -t "$2" -C "$3" -j "$4" 2>/dev/null
done
if iptables -w 2 -C FORWARD -j OUWG_HOLD 2>/dev/null; then exit 1; fi
ip -4 rule show | grep -q '^10420:.*fwmark 0x40000000/0x40000000.*lookup 10420'
ip -4 route show table 10420 | grep -q 'default dev ou-wg0'
ip -4 route show table 10420 | grep -q 'unreachable default'
"#;

const PROXY_CLEANUP: &str = r#"
for spec in 'iptables mangle PREROUTING OUWG_MARK' 'iptables nat PREROUTING OUWG_DNS' 'iptables filter FORWARD OUWG_FWD' 'iptables filter INPUT OUWG_IN' 'ip6tables filter FORWARD OUWG_V6' 'ip6tables filter INPUT OUWG_DNS6'; do
 set -- $spec
 while "$1" -w 2 -t "$2" -C "$3" -j "$4" 2>/dev/null; do "$1" -w 2 -t "$2" -D "$3" -j "$4" || exit 1; done
 "$1" -w 2 -t "$2" -F "$4" 2>/dev/null || true
 "$1" -w 2 -t "$2" -X "$4" 2>/dev/null || true
done
while ip -4 rule del priority 10420 fwmark 0x40000000/0x40000000 table 10420 2>/dev/null; do :; done
ip -4 route flush table 10420 2>/dev/null || true
ip link del ou-wg0 2>/dev/null || true
"#;
const PROXY_HEALTH: &str = r#"
set -e
for spec in 'iptables mangle PREROUTING OUWG_MARK' 'iptables nat PREROUTING OUWG_DNS' 'iptables filter FORWARD OUWG_FWD' 'iptables filter INPUT OUWG_IN' 'ip6tables filter FORWARD OUWG_V6' 'ip6tables filter INPUT OUWG_DNS6'; do
 set -- $spec
 "$1" -w 2 -t "$2" -C "$3" -j "$4" 2>/dev/null
done
if iptables -w 2 -C FORWARD -j OUWG_HOLD 2>/dev/null; then exit 1; fi
ip -4 rule show | grep -q '^10420:.*fwmark 0x40000000/0x40000000.*lookup 10420'
ip -4 route show table 10420 | grep -q 'default dev ou-wg0'
ip -4 route show table 10420 | grep -q 'unreachable default'
"#;
const ROUTE: &str = "ip -4 route replace default dev ou-wg0 metric 10 table 10420\n";

impl Spec {
    fn render(&self, template: &str) -> String {
        let out = template
            .replace("OUWG_", &format!("{}_", self.prefix))
            .replace("10420", self.table)
            .replace("0x40000000", self.mark);
        match self.kind {
            Kind::Tunnel { iface } => out.replace("ou-wg0", iface),
            Kind::Proxy { iface, port } => out
                .replace("ou-wg0", iface)
                .replace("@PORT@", &port.to_string()),
        }
    }
    pub fn cleanup(&self) -> String {
        self.render(match self.kind {
            Kind::Tunnel { .. } => CLEANUP,
            Kind::Proxy { .. } => PROXY_CLEANUP,
        })
    }
    pub fn hold(&self) -> String {
        self.render(HOLD)
    }
    pub fn release(&self) -> String {
        self.render(RELEASE)
    }
    fn owned(&self) -> String {
        format!("{}/owned", self.dir)
    }
    pub fn is_owned(&self) -> bool {
        Path::new(&self.owned()).exists()
    }
    pub fn interface_exists(&self) -> bool {
        let (Kind::Tunnel { iface } | Kind::Proxy { iface, .. }) = self.kind;
        Path::new(&format!("/sys/class/net/{iface}")).exists()
    }
    fn selection(
        mode: &str,
        macs: &[String],
        bin: &str,
        table: &str,
        chain: &str,
        action: &str,
    ) -> String {
        let mut out = String::new();
        for mac in macs {
            let target = if mode == "all" { "RETURN" } else { action };
            out.push_str(&format!(
                "{bin} -w 2 -t {table} -A {chain} -m mac --mac-source {mac} -j {target}\n"
            ));
        }
        if mode == "all" {
            out.push_str(&format!("{bin} -w 2 -t {table} -A {chain} -j {action}\n"));
        }
        out
    }
    /// LAN-ingress exclusions at the top of the mark chain: router, LAN and
    /// broadcast discovery stay local, but DNS to the router is captured.
    fn mark_exclusions() -> &'static str {
        // Actual connected subnet is discovered, never hardcoded.
        "iptables -w 2 -t mangle -A OUWG_MARK ! -i br-lan -j RETURN\nfor net in $(ip -o -4 route show dev br-lan scope link | awk '{print $1}'); do\niptables -w 2 -t mangle -A OUWG_MARK -d \"$net\" -p tcp ! --dport 53 -j RETURN\niptables -w 2 -t mangle -A OUWG_MARK -d \"$net\" -p udp ! --dport 53 -j RETURN\niptables -w 2 -t mangle -A OUWG_MARK -d \"$net\" -p icmp -j RETURN\ndone\niptables -w 2 -t mangle -A OUWG_MARK -d 224.0.0.0/4 -j RETURN\niptables -w 2 -t mangle -A OUWG_MARK -d 255.255.255.255/32 -j RETURN\n"
    }
    /// IPv6 is not handled by either gateway: block it for selected devices so
    /// they cannot bypass the policy, including IPv6 DNS to the router.
    fn ipv6_block(mode: &str, macs: &[String]) -> String {
        let mut out = String::from("ip6tables -w 2 -A OUWG_V6 ! -i br-lan -j RETURN\nip6tables -w 2 -A OUWG_V6 -o br-lan -j RETURN\nip6tables -w 2 -A OUWG_DNS6 ! -i br-lan -j RETURN\n");
        out.push_str(&Self::selection(
            mode,
            macs,
            "ip6tables",
            "filter",
            "OUWG_V6",
            "REJECT",
        ));
        // DNS over IPv6 to the router must not escape through the stock resolver.
        for m in macs {
            if mode == "all" {
                out.push_str(&format!(
                    "ip6tables -w 2 -A OUWG_DNS6 -m mac --mac-source {m} -j RETURN\n"
                ));
            }
        }
        for p in ["tcp", "udp"] {
            if mode == "all" {
                out.push_str(&format!(
                    "ip6tables -w 2 -A OUWG_DNS6 -p {p} --dport 53 -j REJECT\n"
                ));
            } else {
                for m in macs {
                    out.push_str(&format!("ip6tables -w 2 -A OUWG_DNS6 -m mac --mac-source {m} -p {p} --dport 53 -j REJECT\n"));
                }
            }
        }
        out
    }
    /// Kernel tunnel policy for validated identifiers only; `dns` must be a
    /// validated IPv4 address.
    pub fn tunnel_policy(&self, mode: &str, macs: &[String], dns: &str) -> String {
        let mut out = String::from("set -eu\n");
        // Create unreachable fallback first: marked packets must never fall through to WAN.
        out.push_str("ip -4 route replace unreachable default metric 32760 table 10420\nip -4 route replace default dev ou-wg0 metric 10 table 10420\nip -4 rule add priority 10420 fwmark 0x40000000/0x40000000 table 10420\n");
        out.push_str("for spec in 'iptables mangle OUWG_MARK' 'iptables nat OUWG_DNS' 'iptables nat OUWG_NAT' 'iptables filter OUWG_FWD' 'iptables mangle OUWG_MSS' 'ip6tables filter OUWG_V6' 'ip6tables filter OUWG_DNS6' 'iptables filter OUWG_DNS4'; do set -- $spec; \"$1\" -w 2 -t \"$2\" -N \"$3\"; done\n");
        out.push_str(Self::mark_exclusions());
        out.push_str(&Self::selection(
            mode,
            macs,
            "iptables",
            "mangle",
            "OUWG_MARK",
            "MARK --set-xmark 0x40000000/0x40000000",
        ));
        for p in ["udp", "tcp"] {
            out.push_str(&format!("iptables -w 2 -t nat -A OUWG_DNS -i br-lan -m mark --mark 0x40000000/0x40000000 -p {p} --dport 53 -j DNAT --to-destination {dns}\n"));
        }
        out.push_str("iptables -w 2 -t nat -A OUWG_NAT -o ou-wg0 -m mark --mark 0x40000000/0x40000000 -j MASQUERADE\n");
        out.push_str("iptables -w 2 -A OUWG_FWD -i ou-wg0 -o br-lan -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT\niptables -w 2 -A OUWG_FWD -i br-lan -o ou-wg0 -m mark --mark 0x40000000/0x40000000 -j ACCEPT\niptables -w 2 -A OUWG_FWD -i br-lan -m mark --mark 0x40000000/0x40000000 ! -o br-lan -j REJECT\niptables -w 2 -t mangle -A OUWG_MSS -o ou-wg0 -p tcp --tcp-flags SYN,RST SYN -j TCPMSS --clamp-mss-to-pmtu\n");
        out.push_str(&Self::ipv6_block(mode, macs));
        for p in ["tcp", "udp"] {
            out.push_str(&format!("iptables -w 2 -A OUWG_DNS4 -i br-lan -m mark --mark 0x40000000/0x40000000 -p {p} --dport 53 -j REJECT\n"));
        }
        // Attach marking last, after all forwarding and DNS protection is ready.
        out.push_str("for spec in 'ip6tables filter FORWARD OUWG_V6' 'ip6tables filter INPUT OUWG_DNS6' 'iptables filter INPUT OUWG_DNS4' 'iptables filter FORWARD OUWG_FWD' 'iptables mangle FORWARD OUWG_MSS' 'iptables nat POSTROUTING OUWG_NAT' 'iptables nat PREROUTING OUWG_DNS' 'iptables mangle PREROUTING OUWG_MARK'; do set -- $spec; \"$1\" -w 2 -t \"$2\" -I \"$3\" 1 -j \"$4\"; done\n");
        self.render(&out)
    }
    /// Userspace proxy policy for validated identifiers only; `dns` must be a
    /// validated IPv4 address inside the TUN network. Marked TCP is redirected to
    /// the local listener (refused while it is down); everything else is routed
    /// into the TUN, whose route `attach_route` adds once the interface exists.
    /// Until then the unreachable fallback blocks it, so nothing falls through to WAN.
    pub fn proxy_policy(&self, mode: &str, macs: &[String], dns: &str) -> String {
        let mut out = String::from("set -eu\n");
        out.push_str("ip -4 route replace unreachable default metric 32760 table 10420\nip -4 rule add priority 10420 fwmark 0x40000000/0x40000000 table 10420\n");
        out.push_str("for spec in 'iptables mangle OUWG_MARK' 'iptables nat OUWG_DNS' 'iptables filter OUWG_FWD' 'iptables filter OUWG_IN' 'ip6tables filter OUWG_V6' 'ip6tables filter OUWG_DNS6'; do set -- $spec; \"$1\" -w 2 -t \"$2\" -N \"$3\"; done\n");
        out.push_str(Self::mark_exclusions());
        out.push_str(&Self::selection(
            mode,
            macs,
            "iptables",
            "mangle",
            "OUWG_MARK",
            "MARK --set-xmark 0x40000000/0x40000000",
        ));
        // UDP DNS goes to the proxy's address inside the TUN; TCP (DNS included) to the listener.
        out.push_str(&format!("iptables -w 2 -t nat -A OUWG_DNS -i br-lan -m mark --mark 0x40000000/0x40000000 -p udp --dport 53 -j DNAT --to-destination {dns}\n"));
        out.push_str("iptables -w 2 -t nat -A OUWG_DNS -i br-lan -m mark --mark 0x40000000/0x40000000 -p tcp -j REDIRECT --to-ports @PORT@\n");
        out.push_str("iptables -w 2 -A OUWG_FWD -i ou-wg0 -o br-lan -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT\niptables -w 2 -A OUWG_FWD -i br-lan -o ou-wg0 -m mark --mark 0x40000000/0x40000000 -j ACCEPT\niptables -w 2 -A OUWG_FWD -i br-lan -m mark --mark 0x40000000/0x40000000 ! -o br-lan -j REJECT\n");
        // Only redirected connections may reach the listener; direct ones are refused.
        out.push_str("iptables -w 2 -A OUWG_IN -i br-lan -m mark --mark 0x40000000/0x40000000 -p tcp --dport @PORT@ -m conntrack --ctstate DNAT -j ACCEPT\niptables -w 2 -A OUWG_IN -p tcp --dport @PORT@ -j REJECT --reject-with tcp-reset\n");
        for p in ["tcp", "udp"] {
            out.push_str(&format!("iptables -w 2 -A OUWG_IN -i br-lan -m mark --mark 0x40000000/0x40000000 -p {p} --dport 53 -j REJECT\n"));
        }
        out.push_str(&Self::ipv6_block(mode, macs));
        // Attach marking last, after forwarding, delivery and DNS protection are ready.
        out.push_str("for spec in 'ip6tables filter FORWARD OUWG_V6' 'ip6tables filter INPUT OUWG_DNS6' 'iptables filter INPUT OUWG_IN' 'iptables filter FORWARD OUWG_FWD' 'iptables nat PREROUTING OUWG_DNS' 'iptables mangle PREROUTING OUWG_MARK'; do set -- $spec; \"$1\" -w 2 -t \"$2\" -I \"$3\" 1 -j \"$4\"; done\n");
        self.render(&out)
    }
    /// Route marked traffic into the proxy's TUN once the interface exists.
    pub fn route_script(&self) -> String {
        self.render(&format!("set -eu\n{ROUTE}"))
    }
    pub fn script(&self, text: &str) -> Result<(), String> {
        let mut cmd = Command::new("sh");
        let out = process_runner::output(
            &mut cmd,
            Some(text.as_bytes()),
            Duration::from_secs(90),
            32768,
        )
        .map_err(|_| format!("{} network operation timed out", self.name))?;
        if !out.status.success() {
            // Generated policy contains only validated non-secret network identifiers.
            let msg = String::from_utf8_lossy(&out.stderr);
            return Err(format!(
                "{} network operation failed: {}",
                self.name,
                msg.chars().take(350).collect::<String>()
            ));
        }
        Ok(())
    }
    pub fn claim_resources(&self) -> Result<(), String> {
        if self.is_owned() {
            return Ok(());
        }
        let rule = format!("{}:", self.table);
        if self.interface_exists()
            || run("ip", &["-4", "rule", "show"]).is_ok_and(|s| {
                s.lines()
                    .any(|l| l.starts_with(&rule) || l.contains(self.mark))
            })
            || run("ip", &["-4", "route", "show", "table", self.table]).is_ok_and(|s| !s.is_empty())
        {
            return Err(format!(
                "{} interface, mark or routing table is already in use",
                self.name
            ));
        }
        let chains = format!("{}_", self.prefix);
        for bin in ["iptables", "ip6tables"] {
            for table in ["filter", "nat", "mangle"] {
                if run(bin, &["-t", table, "-S"])
                    .is_ok_and(|s| s.contains(&chains) || s.contains(self.mark))
                {
                    return Err(format!(
                        "Reserved {} firewall resources are already in use",
                        self.name
                    ));
                }
            }
        }
        storage::atomic_write(
            Path::new(&self.owned()),
            format!("openui-{}-v1\n", self.prefix.to_lowercase()).as_bytes(),
        )
        .map_err(|_| format!("Cannot record {} resource ownership", self.name))
    }
    /// Exits 0 only when the policy is attached and no transition guard remains.
    pub fn health_script(&self) -> String {
        self.render(match self.kind {
            Kind::Tunnel { .. } => HEALTH,
            Kind::Proxy { .. } => PROXY_HEALTH,
        })
    }
    pub fn healthy(&self) -> bool {
        run("sh", &["-c", &self.health_script()]).is_ok()
    }
}

// Persist the selected profile only after applying it. Keep the transition guard
// in place when the previous policy cannot be restored.
pub fn commit_change<S>(
    old: &S,
    next: &S,
    mut apply: impl FnMut(&S) -> Result<(), String>,
    mut persist: impl FnMut(&S) -> Result<(), String>,
    mut release: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let failure = apply(next).err().or_else(|| persist(next).err());
    if let Some(error) = failure {
        if apply(old).is_err() {
            return Err(format!("{error}; previous policy could not be restored"));
        }
        release().map_err(|e| {
            format!(
                "{error}; previous policy restored but transition guard could not be released: {e}"
            )
        })?;
        return Err(error);
    }
    release()
}

#[cfg(test)]
mod tests {
    use super::*;
    const PROXY: Spec = Spec {
        name: "Test",
        prefix: "OUSB",
        table: "10430",
        mark: "0x20000000",
        dir: "/nonexistent",
        kind: Kind::Proxy {
            iface: "ou-sb0",
            port: 9096,
        },
    };
    #[test]
    fn proxy_policy_uses_only_its_own_resources() {
        let macs = vec!["02:11:22:33:44:55".to_string()];
        let selected = PROXY.proxy_policy("selected", &macs, "172.19.0.2");
        for text in [
            selected.clone(),
            PROXY.proxy_policy("all", &macs, "172.19.0.2"),
            PROXY.cleanup(),
            PROXY.hold(),
            PROXY.release(),
            PROXY.health_script(),
            PROXY.route_script(),
        ] {
            for wireguard in ["OUWG", "ou-wg0", "10420", "0x40000000", "@PORT@"] {
                assert!(!text.contains(wireguard), "{wireguard} leaked into\n{text}");
            }
            for unwanted in ["TPROXY", "MASQUERADE", "_NAT", "_MSS"] {
                assert!(!text.contains(unwanted), "{unwanted} in\n{text}");
            }
        }
        // The TUN route is added only after the interface exists; the fallback blocks until then.
        assert!(!selected.contains("default dev ou-sb0"));
        assert!(
            selected.contains("ip -4 route replace unreachable default metric 32760 table 10430")
        );
        assert!(PROXY
            .route_script()
            .contains("default dev ou-sb0 metric 10 table 10430"));
        assert!(selected
            .contains("--mac-source 02:11:22:33:44:55 -j MARK --set-xmark 0x20000000/0x20000000"));
        assert!(selected.contains("-p tcp -j REDIRECT --to-ports 9096"));
        assert!(selected.contains("-p udp --dport 53 -j DNAT --to-destination 172.19.0.2"));
        assert!(selected.contains("--dport 9096 -m conntrack --ctstate DNAT -j ACCEPT"));
        assert!(
            selected.find("--ctstate DNAT -j ACCEPT").unwrap()
                < selected.find("--dport 9096 -j REJECT").unwrap()
        );
        assert!(selected.ends_with("'iptables mangle PREROUTING OUSB_MARK'; do set -- $spec; \"$1\" -w 2 -t \"$2\" -I \"$3\" 1 -j \"$4\"; done\n"));
        let all = PROXY.proxy_policy("all", &macs, "172.19.0.2");
        assert!(all.contains("--mac-source 02:11:22:33:44:55 -j RETURN"));
        assert!(all.contains("-A OUSB_MARK -j MARK --set-xmark 0x20000000/0x20000000"));
    }
    #[test]
    fn device_choices_follow_mode() {
        let mut macs = vec!["02:11:22:33:44:55".to_string()];
        let choice = |mac: &str, selected| DeviceChoice {
            mac: mac.into(),
            selected,
        };
        apply_devices(
            "all",
            &mut macs,
            vec![
                choice("02:11:22:33:44:55", true),
                choice("02:11:22:33:44:56", false),
            ],
        )
        .unwrap();
        assert_eq!(macs, vec!["02:11:22:33:44:56"]);
        assert!(apply_devices("all", &mut macs, vec![choice("bad", true)]).is_err());
        assert!(validate_selection("other", &macs).is_err());
    }
}
