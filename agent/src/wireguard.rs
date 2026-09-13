//! One managed kernel tunnel, with MAC-based LAN policy. Never changes main routes,
//! stock firewall chains, init.d, or UCI. Private keys are never returned by the API.
use crate::{process::BoundedCommand, storage};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    net::Ipv4Addr,
    path::Path,
    process::Command,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const DIR: &str = "/data/local/tmp/openui-wireguard";
const CONFIG: &str = "/data/local/tmp/openui-wireguard/config.json";
const WG: &str = "/data/bin/wg";
const IFACE: &str = "ou-wg0";
static LOCK: Mutex<()> = Mutex::new(());
static ERROR: Mutex<Option<String>> = Mutex::new(None);

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Settings {
    revision: u64,
    enabled: bool,
    mode: String,
    macs: Vec<String>,
    tunnel: Option<Tunnel>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            revision: 0,
            enabled: false,
            mode: "all".into(),
            macs: vec![],
            tunnel: None,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Tunnel {
    private_key: String,
    public_key: String,
    preshared_key: String,
    address: String,
    endpoint: String,
    dns: String,
    mtu: u16,
    keepalive: u16,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    enabled: Option<bool>,
    mode: Option<String>,
    macs: Option<Vec<String>>,
    config_text: Option<String>,
    expected_revision: Option<u64>,
    devices: Option<Vec<DeviceChoice>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceChoice {
    mac: String,
    selected: bool,
}

fn edited(old: &Settings, u: Update) -> Result<Settings, String> {
    if u.expected_revision.is_some_and(|r| r != old.revision) {
        return Err("Configuration changed; refresh before saving".into());
    }
    if u.expected_revision.is_none()
        && (u.enabled.is_none() || u.mode.is_none() || u.macs.is_none())
    {
        return Err("Partial updates require expected_revision".into());
    }
    if u.devices.is_some() && (u.mode.is_some() || u.macs.is_some()) {
        return Err("Cannot combine device changes with a routing mode change".into());
    }
    let mut next = old.clone();
    next.revision = old
        .revision
        .checked_add(1)
        .ok_or("Configuration revision exhausted")?;
    if let Some(v) = u.enabled {
        next.enabled = v;
    }
    if let Some(v) = u.mode {
        next.mode = v;
    }
    if let Some(v) = u.macs {
        next.macs = v.into_iter().map(|m| m.to_lowercase()).collect();
    }
    if let Some(choices) = u.devices {
        if choices.len() > 128 {
            return Err("Too many device changes".into());
        }
        for c in choices {
            let m = c.mac.to_lowercase();
            if !mac(&m) {
                return Err("Invalid device MAC".into());
            }
            next.macs.retain(|v| v != &m);
            if c.selected == (next.mode == "selected") {
                next.macs.push(m);
            }
        }
    }
    if let Some(text) = u.config_text.filter(|s| !s.trim().is_empty()) {
        next.tunnel = Some(parse(&text)?);
    }
    next.macs.sort();
    next.macs.dedup();
    validate(&next)?;
    Ok(next)
}

fn key(s: &str) -> bool {
    s.len() == 44
        && s.ends_with('=')
        && s[..43]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
        && s.bytes().any(|b| b != b'A' && b != b'=')
}
fn address(s: &str) -> bool {
    let Some((ip, prefix)) = s.split_once('/') else {
        return false;
    };
    ip.parse::<Ipv4Addr>()
        .is_ok_and(|i| !i.is_unspecified() && !i.is_loopback() && !i.is_multicast())
        && prefix.parse::<u8>().is_ok_and(|p| (1..=32).contains(&p))
}
fn endpoint(s: &str) -> bool {
    let Some((host, port)) = s.rsplit_once(':') else {
        return false;
    };
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with('-')
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && port.parse::<u16>().is_ok_and(|p| p > 0)
}
fn mac(s: &str) -> bool {
    s.len() == 17
        && s.split(':').count() == 6
        && s.split(':')
            .all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_hexdigit()))
        && u8::from_str_radix(&s[..2], 16).is_ok_and(|b| b & 1 == 0)
        && s != "00:00:00:00:00:00"
}
fn parse(text: &str) -> Result<Tunnel, String> {
    if text.len() > 16384 {
        return Err("Configuration exceeds 16 KiB".into());
    }
    let mut t = Tunnel {
        private_key: String::new(),
        public_key: String::new(),
        preshared_key: String::new(),
        address: String::new(),
        endpoint: String::new(),
        dns: "1.1.1.1".into(),
        mtu: 1280,
        keepalive: 25,
    };
    let mut section = "";
    let mut peers = 0;
    let mut allowed = false;
    let mut seen = std::collections::HashSet::new();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line == "[Interface]" {
            section = "Interface";
            continue;
        }
        if line == "[Peer]" {
            peers += 1;
            section = "Peer";
            continue;
        }
        let (name, value) = line.split_once('=').ok_or("Invalid configuration line")?;
        let (name, value) = (name.trim(), value.trim());
        if !seen.insert((section, name)) {
            return Err(format!("Duplicate {name}"));
        }
        match (section, name) {
            ("Interface", "PrivateKey") => t.private_key = value.into(),
            ("Interface", "Address") => t.address = value.split(',').map(str::trim).find(|s| address(s)).ok_or("An IPv4 tunnel address with prefix is required")?.into(),
            ("Interface", "DNS") => t.dns = value.split(',').map(str::trim).find(|s| s.parse::<Ipv4Addr>().is_ok()).ok_or("An IPv4 DNS server is required")?.into(),
            ("Interface", "MTU") => t.mtu = value.parse().map_err(|_| "Invalid MTU")?,
            ("Peer", "PublicKey") => t.public_key = value.into(),
            ("Peer", "PresharedKey") => t.preshared_key = value.into(),
            ("Peer", "Endpoint") => t.endpoint = value.into(),
            ("Peer", "PersistentKeepalive") => t.keepalive = value.parse().map_err(|_| "Invalid keepalive")?,
            ("Peer", "AllowedIPs") => allowed = value.split(',').any(|v| v.trim() == "0.0.0.0/0"),
            _ => return Err(format!("Unsupported field: {name}. Use a standard single-peer client configuration without hooks.")),
        }
    }
    if peers != 1 || !allowed {
        return Err("Exactly one peer with AllowedIPs including 0.0.0.0/0 is required".into());
    }
    validate_tunnel(&t)?;
    Ok(t)
}
fn validate_tunnel(t: &Tunnel) -> Result<(), String> {
    if !key(&t.private_key)
        || !key(&t.public_key)
        || (!t.preshared_key.is_empty() && !key(&t.preshared_key))
    {
        return Err("Invalid WireGuard key (expected 32-byte base64 key)".into());
    }
    if !address(&t.address) || !endpoint(&t.endpoint) {
        return Err("Invalid IPv4 tunnel address or hostname:port endpoint".into());
    }
    let dns = t
        .dns
        .parse::<Ipv4Addr>()
        .map_err(|_| "Invalid IPv4 DNS server")?;
    if dns.is_unspecified() || dns.is_loopback() || dns.is_multicast() {
        return Err("Invalid IPv4 DNS server".into());
    }
    if !(1280..=1380).contains(&t.mtu) {
        return Err("MTU must be 1280–1380; 1280 is recommended for cellular WAN".into());
    }
    Ok(())
}
fn validate(s: &Settings) -> Result<(), String> {
    if s.mode != "all" && s.mode != "selected" {
        return Err("Invalid routing mode".into());
    }
    if s.macs.len() > 128 || s.macs.iter().any(|s| !mac(s)) {
        return Err("Invalid device MAC selection (maximum 128)".into());
    }
    if s.enabled && s.tunnel.is_none() {
        return Err("Import a WireGuard client configuration first".into());
    }
    if let Some(t) = &s.tunnel {
        validate_tunnel(t)?;
    }
    Ok(())
}
fn load() -> Result<Settings, String> {
    match fs::read(CONFIG) {
        Ok(b) => {
            let s: Settings = serde_json::from_slice(&b)
                .map_err(|_| "Saved WireGuard configuration is invalid")?;
            validate(&s)?;
            Ok(s)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(_) => Err("Cannot read saved WireGuard configuration".into()),
    }
}
fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let o = Command::new(program)
        .args(args)
        .bounded_output()
        .map_err(|_| format!("Cannot run {program}"))?;
    if !o.status.success() {
        return Err(format!("{program} failed"));
    }
    Ok(String::from_utf8_lossy(&o.stdout).trim().into())
}
fn config_text(t: &Tunnel) -> String {
    let mut s = format!("[Interface]\nPrivateKey = {}\n[Peer]\nPublicKey = {}\nEndpoint = {}\nAllowedIPs = 0.0.0.0/0\nPersistentKeepalive = {}\n", t.private_key, t.public_key, t.endpoint, t.keepalive);
    if !t.preshared_key.is_empty() {
        s.push_str(&format!("PresharedKey = {}\n", t.preshared_key));
    }
    s
}
// Literal script is never combined with unvalidated configuration or arbitrary shell hooks.
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
fn selection(s: &Settings, bin: &str, table: &str, chain: &str, action: &str) -> String {
    let mut out = String::new();
    for mac in &s.macs {
        let target = if s.mode == "all" { "RETURN" } else { action };
        out.push_str(&format!(
            "{bin} -w 2 -t {table} -A {chain} -m mac --mac-source {mac} -j {target}\n"
        ));
    }
    if s.mode == "all" {
        out.push_str(&format!("{bin} -w 2 -t {table} -A {chain} -j {action}\n"));
    }
    out
}
fn policy(s: &Settings) -> String {
    let t = s.tunnel.as_ref().unwrap();
    let mut out = String::from("set -eu\n");
    // Create unreachable fallback first: marked packets must never fall through to WAN.
    out.push_str("ip -4 route replace unreachable default metric 32760 table 10420\nip -4 route replace default dev ou-wg0 metric 10 table 10420\nip -4 rule add priority 10420 fwmark 0x40000000/0x40000000 table 10420\n");
    out.push_str("for spec in 'iptables mangle OUWG_MARK' 'iptables nat OUWG_DNS' 'iptables nat OUWG_NAT' 'iptables filter OUWG_FWD' 'iptables mangle OUWG_MSS' 'ip6tables filter OUWG_V6' 'ip6tables filter OUWG_DNS6' 'iptables filter OUWG_DNS4'; do set -- $spec; \"$1\" -w 2 -t \"$2\" -N \"$3\"; done\n");
    out.push_str("iptables -w 2 -t mangle -A OUWG_MARK ! -i br-lan -j RETURN\n");
    // Keep router/LAN and broadcast discovery local. Actual connected subnet is discovered, never hardcoded.
    out.push_str("for net in $(ip -o -4 route show dev br-lan scope link | awk '{print $1}'); do\niptables -w 2 -t mangle -A OUWG_MARK -d \"$net\" -p tcp ! --dport 53 -j RETURN\niptables -w 2 -t mangle -A OUWG_MARK -d \"$net\" -p udp ! --dport 53 -j RETURN\niptables -w 2 -t mangle -A OUWG_MARK -d \"$net\" -p icmp -j RETURN\ndone\niptables -w 2 -t mangle -A OUWG_MARK -d 224.0.0.0/4 -j RETURN\niptables -w 2 -t mangle -A OUWG_MARK -d 255.255.255.255/32 -j RETURN\n");
    out.push_str(&selection(
        s,
        "iptables",
        "mangle",
        "OUWG_MARK",
        "MARK --set-xmark 0x40000000/0x40000000",
    ));
    for p in ["udp", "tcp"] {
        out.push_str(&format!("iptables -w 2 -t nat -A OUWG_DNS -i br-lan -m mark --mark 0x40000000/0x40000000 -p {p} --dport 53 -j DNAT --to-destination {}\n", t.dns));
    }
    out.push_str("iptables -w 2 -t nat -A OUWG_NAT -o ou-wg0 -m mark --mark 0x40000000/0x40000000 -j MASQUERADE\niptables -w 2 -A OUWG_FWD -i ou-wg0 -o br-lan -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT\niptables -w 2 -A OUWG_FWD -i br-lan -o ou-wg0 -m mark --mark 0x40000000/0x40000000 -j ACCEPT\niptables -w 2 -A OUWG_FWD -i br-lan -m mark --mark 0x40000000/0x40000000 ! -o br-lan -j REJECT\niptables -w 2 -t mangle -A OUWG_MSS -o ou-wg0 -p tcp --tcp-flags SYN,RST SYN -j TCPMSS --clamp-mss-to-pmtu\n");
    out.push_str("ip6tables -w 2 -A OUWG_V6 ! -i br-lan -j RETURN\nip6tables -w 2 -A OUWG_V6 -o br-lan -j RETURN\nip6tables -w 2 -A OUWG_DNS6 ! -i br-lan -j RETURN\n");
    out.push_str(&selection(s, "ip6tables", "filter", "OUWG_V6", "REJECT"));
    // DNS over IPv6 to the router must not escape through the stock resolver.
    for m in &s.macs {
        if s.mode == "all" {
            out.push_str(&format!(
                "ip6tables -w 2 -A OUWG_DNS6 -m mac --mac-source {m} -j RETURN\n"
            ));
        }
    }
    for p in ["tcp", "udp"] {
        if s.mode == "all" {
            out.push_str(&format!(
                "ip6tables -w 2 -A OUWG_DNS6 -p {p} --dport 53 -j REJECT\n"
            ));
        } else {
            for m in &s.macs {
                out.push_str(&format!("ip6tables -w 2 -A OUWG_DNS6 -m mac --mac-source {m} -p {p} --dport 53 -j REJECT\n"));
            }
        }
    }
    for p in ["tcp", "udp"] {
        out.push_str(&format!("iptables -w 2 -A OUWG_DNS4 -i br-lan -m mark --mark 0x40000000/0x40000000 -p {p} --dport 53 -j REJECT\n"));
    }
    // Attach marking last, after all forwarding and DNS protection is ready.
    out.push_str("for spec in 'ip6tables filter FORWARD OUWG_V6' 'ip6tables filter INPUT OUWG_DNS6' 'iptables filter INPUT OUWG_DNS4' 'iptables filter FORWARD OUWG_FWD' 'iptables mangle FORWARD OUWG_MSS' 'iptables nat POSTROUTING OUWG_NAT' 'iptables nat PREROUTING OUWG_DNS' 'iptables mangle PREROUTING OUWG_MARK'; do set -- $spec; \"$1\" -w 2 -t \"$2\" -I \"$3\" 1 -j \"$4\"; done\n");
    out
}
fn script(text: &str) -> Result<(), String> {
    let mut cmd = Command::new("sh");
    let out = process_runner::output(
        &mut cmd,
        Some(text.as_bytes()),
        Duration::from_secs(90),
        32768,
    )
    .map_err(|_| "WireGuard network operation timed out")?;
    if !out.status.success() {
        // Generated policy contains only validated non-secret network identifiers.
        let msg = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "WireGuard network operation failed: {}",
            msg.chars().take(350).collect::<String>()
        ));
    }
    Ok(())
}
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
fn claim_resources() -> Result<(), String> {
    if Path::new(&format!("{DIR}/owned")).exists() {
        return Ok(());
    }
    if Path::new(&format!("/sys/class/net/{IFACE}")).exists()
        || run("ip", &["-4", "rule", "show"]).is_ok_and(|s| {
            s.lines()
                .any(|l| l.starts_with("10420:") || l.contains("0x40000000"))
        })
        || run("ip", &["-4", "route", "show", "table", "10420"]).is_ok_and(|s| !s.is_empty())
    {
        return Err("WireGuard interface, mark or routing table is already in use".into());
    }
    for bin in ["iptables", "ip6tables"] {
        for table in ["filter", "nat", "mangle"] {
            if run(bin, &["-t", table, "-S"])
                .is_ok_and(|s| s.contains("OUWG_") || s.contains("0x40000000"))
            {
                return Err("Reserved WireGuard firewall resources are already in use".into());
            }
        }
    }
    storage::atomic_write(Path::new(&format!("{DIR}/owned")), b"openui-wireguard-v1\n")
        .map_err(|_| "Cannot record WireGuard resource ownership".into())
}
fn apply(s: &Settings) -> Result<(), String> {
    if !s.enabled {
        if Path::new(&format!("{DIR}/owned")).exists() {
            script(CLEANUP)?;
        }
        return Ok(());
    }
    validate(s)?;
    if !Path::new("/sys/module/wireguard").exists() {
        return Err("Kernel WireGuard support is unavailable".into());
    }
    if !Path::new(WG).exists() {
        return Err("WireGuard tools missing: install /data/bin/wg".into());
    }
    let t = s.tunnel.as_ref().unwrap();
    // Config file is root-only, and no key ever enters argv or error output.
    storage::atomic_write(
        Path::new(&format!("{DIR}/peer.conf")),
        config_text(t).as_bytes(),
    )
    .map_err(|_| "Cannot write private tunnel configuration")?;
    claim_resources()?;
    script(CLEANUP)?;
    run("ip", &["link", "add", IFACE, "type", "wireguard"])?;
    run(WG, &["setconf", IFACE, &format!("{DIR}/peer.conf")])?;
    run(
        "ip",
        &["address", "add", &t.address, "dev", IFACE, "noprefixroute"],
    )?;
    run(
        "ip",
        &["link", "set", "dev", IFACE, "mtu", &t.mtu.to_string(), "up"],
    )?;
    // Loose reverse-path checking only on our new interface (main route still uses WAN).
    fs::write(format!("/proc/sys/net/ipv4/conf/{IFACE}/rp_filter"), "2\n")
        .map_err(|_| "Cannot set tunnel reverse-path check")?;
    script(&policy(s))
}
fn response(s: &Settings) -> Value {
    let interface = Path::new(&format!("/sys/class/net/{IFACE}")).exists();
    let handshake = if interface {
        run(WG, &["show", IFACE, "latest-handshakes"])
            .ok()
            .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
            .unwrap_or(0)
    } else {
        0
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let traffic = if interface {
        run(WG, &["show", IFACE, "transfer"]).unwrap_or_default()
    } else {
        String::new()
    };
    let fields: Vec<&str> = traffic.split_whitespace().collect();
    let healthy = !s.enabled || policy_healthy();
    let error = ERROR.lock().unwrap().clone().or_else(|| {
        if healthy {
            None
        } else {
            Some("Tunnel policy is incomplete; automatic recovery is pending".into())
        }
    });
    json!({"revision":s.revision,"enabled":s.enabled,"mode":s.mode,"macs":s.macs,"has_config":s.tunnel.is_some(),
        "kernel_supported":Path::new("/sys/module/wireguard").exists(),"tools_installed":Path::new(WG).exists(),
        "interface_up":interface,"connected":s.enabled && healthy && interface && handshake>0 && now.saturating_sub(handshake)<180,
        "latest_handshake":handshake,"rx_bytes":fields.get(1).and_then(|v|v.parse::<u64>().ok()).unwrap_or(0),
        "tx_bytes":fields.get(2).and_then(|v|v.parse::<u64>().ok()).unwrap_or(0),"error":error,
        "endpoint":s.tunnel.as_ref().map(|t|t.endpoint.as_str()),"address":s.tunnel.as_ref().map(|t|t.address.as_str()),
        "dns":s.tunnel.as_ref().map(|t|t.dns.as_str()),"mtu":s.tunnel.as_ref().map(|t|t.mtu),"ipv6_policy":"blocked_for_tunnel_devices"})
}
pub fn get() -> (u16, Value) {
    let _guard = LOCK.lock().unwrap();
    match load() {
        Ok(s) => (200, json!({"ok":true,"data":response(&s)})),
        Err(e) => (500, json!({"ok":false,"error":e})),
    }
}
pub fn update(body: &[u8]) -> (u16, Value) {
    let result = (|| -> Result<Value, String> {
        let u: Update = serde_json::from_slice(body).map_err(|_| "Invalid WireGuard request")?;
        let _guard = LOCK.lock().unwrap();
        let old = load()?;
        let next = edited(&old, u)?;
        fs::create_dir_all(DIR).map_err(|_| "Cannot create WireGuard state directory")?;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(DIR, fs::Permissions::from_mode(0o700))
            .map_err(|_| "Cannot protect WireGuard state directory")?;
        // Validate prerequisites before interrupting any traffic.
        if next.enabled {
            if !Path::new(WG).exists() || !Path::new("/sys/module/wireguard").exists() {
                return Err("Kernel WireGuard or /data/bin/wg is unavailable".into());
            }
            claim_resources()?;
        }
        let changing_network =
            old.enabled || next.enabled || Path::new(&format!("{DIR}/owned")).exists();
        if changing_network {
            script(HOLD)?;
        }
        if let Err(e) = apply(&next) {
            let rollback = apply(&old);
            if rollback.is_ok() {
                let _ = script(RELEASE);
            }
            *ERROR.lock().unwrap() = Some(if rollback.is_ok() {
                e.clone()
            } else {
                format!("{e}; previous policy could not be restored")
            });
            return Err(ERROR.lock().unwrap().clone().unwrap());
        }
        if storage::atomic_write(Path::new(CONFIG), &serde_json::to_vec(&next).unwrap()).is_err() {
            if apply(&old).is_ok() {
                let _ = script(RELEASE);
            }
            return Err(
                "Cannot persist configuration; attempted to restore previous configuration".into(),
            );
        }
        if changing_network {
            script(RELEASE)?;
        }
        *ERROR.lock().unwrap() = None;
        Ok(response(&next))
    })();
    match result {
        Ok(v) => (200, json!({"ok":true,"data":v})),
        Err(e) => (
            if e.starts_with("Configuration changed;") {
                409
            } else {
                400
            },
            json!({"ok":false,"error":e}),
        ),
    }
}
fn policy_healthy() -> bool {
    run("sh", &["-c", r#"
set -e
for spec in 'iptables mangle PREROUTING OUWG_MARK' 'iptables nat PREROUTING OUWG_DNS' 'iptables nat POSTROUTING OUWG_NAT' 'iptables filter FORWARD OUWG_FWD' 'ip6tables filter FORWARD OUWG_V6' 'iptables filter INPUT OUWG_DNS4' 'ip6tables filter INPUT OUWG_DNS6'; do
 set -- $spec
 "$1" -w 2 -t "$2" -C "$3" -j "$4" 2>/dev/null
done
if iptables -w 2 -C FORWARD -j OUWG_HOLD 2>/dev/null; then exit 1; fi
ip -4 rule show | grep -q '^10420:.*fwmark 0x40000000/0x40000000.*lookup 10420'
ip -4 route show table 10420 | grep -q 'default dev ou-wg0'
ip -4 route show table 10420 | grep -q 'unreachable default'
"#]).is_ok()
}
pub fn start() {
    std::thread::spawn(|| {
        let mut first = true;
        loop {
            {
                let _guard = LOCK.lock().unwrap();
                if let Ok(s) = load() {
                    if s.enabled
                        && (first
                            || !Path::new(&format!("/sys/class/net/{IFACE}")).exists()
                            || !policy_healthy())
                    {
                        if let Err(e) = claim_resources()
                            .and_then(|_| script(HOLD))
                            .and_then(|_| apply(&s))
                            .and_then(|_| script(RELEASE))
                        {
                            *ERROR.lock().unwrap() = Some(e);
                        } else {
                            *ERROR.lock().unwrap() = None;
                        }
                    }
                }
                if first {
                    if let Ok(s) = load() {
                        if !s.enabled && Path::new(&format!("{DIR}/owned")).exists() {
                            if let Err(e) = apply(&s).and_then(|_| script(RELEASE)) {
                                *ERROR.lock().unwrap() = Some(e);
                            }
                        }
                    }
                }
                first = false;
            }
            std::thread::sleep(Duration::from_secs(30));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> String {
        format!("[Interface]\nPrivateKey={}\nAddress=10.7.0.2/32, fd00::2/128\nDNS=1.1.1.1\n[Peer]\nPublicKey={}\nEndpoint=vpn.example.com:51820\nAllowedIPs=0.0.0.0/0, ::/0\n", "a".repeat(43)+"=", "b".repeat(43)+"=")
    }
    #[test]
    fn import_and_reject_hooks() {
        let t = parse(&sample()).unwrap();
        assert_eq!(t.address, "10.7.0.2/32");
        assert_eq!(t.mtu, 1280);
        assert!(parse(&(sample() + "PostUp=touch /tmp/pwn\n")).is_err());
        assert!(parse(&sample().replace("vpn.example.com:51820", "$(id):51820")).is_err());
        assert!(parse(&sample().replace("0.0.0.0/0", "10.0.0.0/8")).is_err());
        assert!(parse(&(sample() + "[Peer]\n")).is_err());
    }
    #[test]
    fn screen_edits_preserve_mode_and_reject_stale_revisions() {
        let old = Settings {
            revision: 8,
            macs: vec!["02:11:22:33:44:55".into()],
            ..Default::default()
        };
        let update = |v: Value| serde_json::from_value::<Update>(v).unwrap();
        let next = edited(&old,update(json!({"expected_revision":8,"devices":[{"mac":"02:11:22:33:44:55","selected":true},{"mac":"02:11:22:33:44:56","selected":false}]}))).unwrap();
        assert_eq!(next.mode, "all");
        assert_eq!(next.macs, vec!["02:11:22:33:44:56"]);
        assert_eq!(next.revision, 9);
        assert!(edited(
            &next,
            update(json!({"expected_revision":8,"enabled":false}))
        )
        .is_err());
        assert!(edited(&old, update(json!({"enabled":false}))).is_err());
        let selected = Settings {
            mode: "selected".into(),
            ..old.clone()
        };
        let next=edited(&selected,update(json!({"expected_revision":8,"devices":[{"mac":"02:11:22:33:44:56","selected":true}]}))).unwrap();
        assert_eq!(next.macs.len(), 2);
        assert_eq!(next.mode, "selected");
        assert!(edited(
            &old,
            update(json!({"expected_revision":8,"devices":[{"mac":"bad","selected":true}]}))
        )
        .is_err());
    }
    #[test]
    fn mac_rules_and_kill_switch() {
        let mut s = Settings {
            enabled: true,
            tunnel: Some(parse(&sample()).unwrap()),
            macs: vec!["02:11:22:33:44:55".into()],
            ..Default::default()
        };
        let all = policy(&s);
        assert!(all.contains("--mac-source 02:11:22:33:44:55 -j RETURN"));
        assert!(all.contains("unreachable default"));
        assert!(!all.contains("route replace default dev ou-wg0 table main"));
        s.mode = "selected".into();
        let selected = policy(&s);
        assert!(selected.contains("--mac-source 02:11:22:33:44:55 -j MARK"));
        assert!(selected.contains("--mac-source 02:11:22:33:44:55 -j REJECT"));
        assert!(selected.find("OUWG_DNS -i br-lan").unwrap() < selected.find("-I \"$3\"").unwrap());
    }
    #[test]
    fn export_integration_policies() {
        if let Ok(dir) = std::env::var("OPENUI_WG_TEST_POLICIES") {
            let p = Path::new(&dir);
            fs::create_dir_all(p).unwrap();
            let mut s = Settings {
                enabled: true,
                tunnel: Some(parse(&sample()).unwrap()),
                ..Default::default()
            };
            fs::write(p.join("all.sh"), policy(&s)).unwrap();
            s.mode = "selected".into();
            s.macs = vec!["02:11:22:33:44:55".into()];
            fs::write(p.join("selected.sh"), policy(&s)).unwrap();
            s.mode = "all".into();
            fs::write(p.join("excluded.sh"), policy(&s)).unwrap();
            fs::write(p.join("cleanup.sh"), CLEANUP).unwrap();
            fs::write(p.join("hold.sh"), HOLD).unwrap();
            fs::write(p.join("release.sh"), RELEASE).unwrap();
        }
    }
    #[test]
    fn status_does_not_return_secrets() {
        let t = parse(&sample()).unwrap();
        let private = t.private_key.clone();
        let s = Settings {
            tunnel: Some(t),
            ..Default::default()
        };
        let v = response(&s);
        assert!(!v.to_string().contains(&private));
        assert!(v.get("private_key").is_none());
        assert!(v.get("preshared_key").is_none());
        assert_eq!(v["has_config"], true);
    }
    #[test]
    fn untrusted_identifiers_rejected() {
        assert!(!mac("02:11:22:33:44:55;id"));
        assert!(!mac("ff:ff:ff:ff:ff:ff"));
        assert!(!endpoint("--help:80"));
        assert!(!endpoint("a\n:80"));
        assert!(!address("0.0.0.0/0"));
        assert!(!key(&("A".repeat(43) + "=")));
    }
}
