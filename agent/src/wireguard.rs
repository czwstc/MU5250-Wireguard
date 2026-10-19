//! One managed kernel tunnel, with MAC-based LAN policy. Never changes main routes,
//! stock firewall chains, init.d, or UCI. Private keys are never returned by the API.
use crate::{
    lan_policy::{self, run},
    storage,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    net::Ipv4Addr,
    path::Path,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const DIR: &str = "/data/local/tmp/openui-wireguard";
const CONFIG: &str = "/data/local/tmp/openui-wireguard/config.json";
const WG: &str = "/data/bin/wg";
const IFACE: &str = "ou-wg0";
static LOCK: Mutex<()> = Mutex::new(());
const SPEC: lan_policy::Spec = lan_policy::Spec {
    name: "WireGuard",
    prefix: "OUWG",
    table: "10420",
    mark: "0x40000000",
    dir: DIR,
    kind: lan_policy::Kind::Tunnel { iface: IFACE },
};
static ERROR: Mutex<Option<String>> = Mutex::new(None);

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Settings {
    revision: u64,
    enabled: bool,
    mode: String,
    macs: Vec<String>,
    tunnel: Option<Tunnel>,
    profiles: Vec<Profile>,
    active_profile: Option<u64>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            revision: 0,
            enabled: false,
            mode: "all".into(),
            macs: vec![],
            tunnel: None,
            profiles: vec![],
            active_profile: None,
        }
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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
const MAX_PROFILES: usize = 5;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    id: u64,
    name: String,
    tunnel: Tunnel,
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum ProfileAction {
    Save { name: String, config_text: String },
    Activate { id: u64 },
    Rename { id: u64, name: String },
    Delete { id: u64 },
}
// Legacy single-tunnel files gain a profile without changing revision or network state.
fn migrate(s: &mut Settings) {
    if s.profiles.is_empty() && s.active_profile.is_none() {
        if let Some(tunnel) = &s.tunnel {
            s.profiles.push(Profile {
                id: 1,
                name: "Current configuration".into(),
                tunnel: tunnel.clone(),
            });
            s.active_profile = Some(1);
        }
    }
}
fn profile_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 48 || name.chars().any(char::is_control) {
        return Err("Profile name must contain 1–48 characters without control characters".into());
    }
    Ok(name.into())
}
fn profile_edit(s: &mut Settings, action: ProfileAction) -> Result<(), String> {
    match action {
        ProfileAction::Save { name, config_text } => {
            if s.profiles.len() >= MAX_PROFILES {
                return Err("A maximum of 5 profiles can be saved; delete one first".into());
            }
            let name = profile_name(&name)?;
            if s.profiles
                .iter()
                .any(|p| p.name.to_lowercase() == name.to_lowercase())
            {
                return Err("A profile with this name already exists".into());
            }
            let tunnel = parse(&config_text)?;
            let id = s
                .profiles
                .iter()
                .map(|p| p.id)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or("Profile IDs exhausted")?;
            if s.active_profile.is_none() {
                s.active_profile = Some(id);
                s.tunnel = Some(tunnel.clone());
            }
            s.profiles.push(Profile { id, name, tunnel });
        }
        ProfileAction::Activate { id } => {
            let p = s
                .profiles
                .iter()
                .find(|p| p.id == id)
                .ok_or("Profile not found")?;
            s.tunnel = Some(p.tunnel.clone());
            s.active_profile = Some(id);
        }
        ProfileAction::Rename { id, name } => {
            let name = profile_name(&name)?;
            if s.profiles
                .iter()
                .any(|p| p.id != id && p.name.to_lowercase() == name.to_lowercase())
            {
                return Err("A profile with this name already exists".into());
            }
            s.profiles
                .iter_mut()
                .find(|p| p.id == id)
                .ok_or("Profile not found")?
                .name = name;
        }
        ProfileAction::Delete { id } => {
            if !s.profiles.iter().any(|p| p.id == id) {
                return Err("Profile not found".into());
            }
            if s.active_profile == Some(id) {
                if s.enabled {
                    return Err(
                        "Switch profiles or turn WireGuard off before deleting the active profile"
                            .into(),
                    );
                }
                s.active_profile = None;
                s.tunnel = None;
            }
            s.profiles.retain(|p| p.id != id);
        }
    }
    Ok(())
}
fn same_network(a: &Settings, b: &Settings) -> bool {
    a.enabled == b.enabled && a.mode == b.mode && a.macs == b.macs && a.tunnel == b.tunnel
}
fn profile_status(s: &Settings) -> Value {
    json!(s.profiles.iter().map(|p| json!({"id":p.id,"name":p.name,"endpoint":p.tunnel.endpoint,"address":p.tunnel.address})).collect::<Vec<_>>())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    enabled: Option<bool>,
    mode: Option<String>,
    macs: Option<Vec<String>>,
    config_text: Option<String>,
    expected_revision: Option<u64>,
    devices: Option<Vec<lan_policy::DeviceChoice>>,
    profile: Option<ProfileAction>,
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
    if u.profile.is_some()
        && (u.expected_revision.is_none()
            || u.enabled.is_some()
            || u.mode.is_some()
            || u.macs.is_some()
            || u.devices.is_some()
            || u.config_text.is_some())
    {
        return Err(
            "Profile actions require expected_revision and cannot be combined with routing changes"
                .into(),
        );
    }
    let mut next = old.clone();
    migrate(&mut next);
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
        lan_policy::apply_devices(&next.mode, &mut next.macs, choices)?;
    }
    if let Some(text) = u.config_text.filter(|s| !s.trim().is_empty()) {
        let tunnel = parse(&text)?;
        if let Some(id) = next.active_profile {
            next.profiles
                .iter_mut()
                .find(|p| p.id == id)
                .ok_or("Active profile not found")?
                .tunnel = tunnel.clone();
            next.tunnel = Some(tunnel);
        } else {
            profile_edit(
                &mut next,
                ProfileAction::Save {
                    name: format!("Configuration {}", old.revision + 1),
                    config_text: text,
                },
            )?;
        }
    }
    if let Some(action) = u.profile {
        profile_edit(&mut next, action)?;
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
    if s.profiles.len() > MAX_PROFILES {
        return Err("Too many saved WireGuard profiles".into());
    }
    let mut ids = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    for p in &s.profiles {
        if p.id == 0
            || !ids.insert(p.id)
            || profile_name(&p.name)? != p.name
            || !names.insert(p.name.to_lowercase())
        {
            return Err("Invalid or duplicate saved profile".into());
        }
        validate_tunnel(&p.tunnel)?;
    }
    if let Some(id) = s.active_profile {
        if !s
            .profiles
            .iter()
            .any(|p| p.id == id && Some(&p.tunnel) == s.tunnel.as_ref())
        {
            return Err("Active profile does not match the saved tunnel".into());
        }
    } else if !s.profiles.is_empty() && s.tunnel.is_some() {
        return Err("Saved tunnel is missing its active profile".into());
    }
    lan_policy::validate_selection(&s.mode, &s.macs)?;
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
            let mut s: Settings = serde_json::from_slice(&b)
                .map_err(|_| "Saved WireGuard configuration is invalid")?;
            migrate(&mut s);
            validate(&s)?;
            Ok(s)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(_) => Err("Cannot read saved WireGuard configuration".into()),
    }
}
fn config_text(t: &Tunnel) -> String {
    let mut s = format!("[Interface]\nPrivateKey = {}\n[Peer]\nPublicKey = {}\nEndpoint = {}\nAllowedIPs = 0.0.0.0/0\nPersistentKeepalive = {}\n", t.private_key, t.public_key, t.endpoint, t.keepalive);
    if !t.preshared_key.is_empty() {
        s.push_str(&format!("PresharedKey = {}\n", t.preshared_key));
    }
    s
}
fn policy(s: &Settings) -> String {
    SPEC.tunnel_policy(&s.mode, &s.macs, &s.tunnel.as_ref().unwrap().dns)
}
fn apply(s: &Settings) -> Result<(), String> {
    if !s.enabled {
        if SPEC.is_owned() {
            SPEC.script(&SPEC.cleanup())?;
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
    SPEC.claim_resources()?;
    SPEC.script(&SPEC.cleanup())?;
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
    SPEC.script(&policy(s))
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
    json!({"profiles":profile_status(s),"active_profile":s.active_profile,"max_profiles":MAX_PROFILES,"revision":s.revision,"enabled":s.enabled,"mode":s.mode,"macs":s.macs,"has_config":s.tunnel.is_some(),
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
        let _gate = lan_policy::GATE.lock().unwrap();
        let _guard = LOCK.lock().unwrap();
        let old = load()?;
        let next = edited(&old, u)?;
        if next.enabled && crate::singbox::enabled() {
            return Err("Turn sing-box off before enabling WireGuard".into());
        }
        fs::create_dir_all(DIR).map_err(|_| "Cannot create WireGuard state directory")?;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(DIR, fs::Permissions::from_mode(0o700))
            .map_err(|_| "Cannot protect WireGuard state directory")?;
        // Saving/renaming/deleting an inactive profile must not restart a live tunnel.
        if same_network(&old, &next) {
            storage::atomic_write(Path::new(CONFIG), &serde_json::to_vec(&next).unwrap())
                .map_err(|_| "Cannot persist WireGuard profiles")?;
            return Ok(response(&next));
        }
        // Validate prerequisites before interrupting any traffic.
        if next.enabled {
            if !Path::new(WG).exists() || !Path::new("/sys/module/wireguard").exists() {
                return Err("Kernel WireGuard or /data/bin/wg is unavailable".into());
            }
            SPEC.claim_resources()?;
        }
        let changing_network = old.enabled || next.enabled || SPEC.is_owned();
        if changing_network {
            SPEC.script(&SPEC.hold())?;
        }
        if let Err(e) = lan_policy::commit_change(
            &old,
            &next,
            apply,
            |s| {
                storage::atomic_write(Path::new(CONFIG), &serde_json::to_vec(s).unwrap())
                    .map_err(|_| "Cannot persist configuration".into())
            },
            || {
                if changing_network {
                    SPEC.script(&SPEC.release())
                } else {
                    Ok(())
                }
            },
        ) {
            *ERROR.lock().unwrap() = Some(e.clone());
            return Err(e);
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
    SPEC.healthy()
}
/// Persisted state only; used to keep gateway features mutually exclusive.
pub fn enabled() -> bool {
    load().is_ok_and(|s| s.enabled)
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
                        if let Err(e) = SPEC
                            .claim_resources()
                            .and_then(|_| SPEC.script(&SPEC.hold()))
                            .and_then(|_| apply(&s))
                            .and_then(|_| SPEC.script(&SPEC.release()))
                        {
                            *ERROR.lock().unwrap() = Some(e);
                        } else {
                            *ERROR.lock().unwrap() = None;
                        }
                    }
                }
                if first {
                    if let Ok(s) = load() {
                        if !s.enabled && SPEC.is_owned() {
                            if let Err(e) = apply(&s).and_then(|_| SPEC.script(&SPEC.release())) {
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
    fn profile_change(s: &Settings, action: Value) -> Result<Settings, String> {
        edited(
            s,
            serde_json::from_value(json!({"expected_revision":s.revision,"profile":action}))
                .unwrap(),
        )
    }
    fn saved(s: &Settings, name: &str, endpoint: &str) -> Settings {
        profile_change(s, json!({"action":"save","name":name,"config_text":sample().replace("vpn.example.com:51820",endpoint)})).unwrap()
    }
    #[test]
    fn legacy_configuration_migrates_without_network_changes() {
        let mut s: Settings = serde_json::from_value(json!({"revision":7,"enabled":true,"mode":"selected","macs":["02:11:22:33:44:55"],"tunnel":parse(&sample()).unwrap()})).unwrap();
        let old = s.clone();
        migrate(&mut s);
        migrate(&mut s);
        assert_eq!(s.profiles.len(), 1);
        assert_eq!(s.active_profile, Some(1));
        assert_eq!(s.revision, 7);
        assert!(same_network(&old, &s));
        validate(&s).unwrap();
        let restored: Settings = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        validate(&restored).unwrap();
        assert_eq!(restored.active_profile, s.active_profile);
    }
    #[test]
    fn five_profile_limit_and_atomic_validation() {
        let mut s = Settings::default();
        for i in 1..=5 {
            s = saved(&s, &format!("VPN {i}"), "vpn.example.com:51820");
        }
        let before = serde_json::to_vec(&s).unwrap();
        assert!(profile_change(
            &s,
            json!({"action":"save","name":"six","config_text":sample()})
        )
        .err()
        .unwrap()
        .contains("maximum of 5"));
        assert_eq!(before, serde_json::to_vec(&s).unwrap());
        assert!(profile_change(&s, json!({"action":"rename","id":2,"name":" vpn 1 "})).is_err());
        assert!(profile_change(&s, json!({"action":"rename","id":2,"name":"x\ny"})).is_err());
        assert!(
            profile_change(&s, json!({"action":"rename","id":2,"name":"x".repeat(49)})).is_err()
        );
        let s = profile_change(&s, json!({"action":"delete","id":5})).unwrap();
        assert_eq!(
            saved(&s, "Replacement", "two.example.com:51820")
                .profiles
                .len(),
            5
        );
    }
    #[test]
    fn switch_preserves_device_policy_and_enabled_state() {
        let mut s = saved(&Settings::default(), "Home", "one.example.com:51820");
        s.enabled = true;
        s.mode = "selected".into();
        s.macs = vec!["02:11:22:33:44:55".into()];
        let backup = saved(&s, "Travel", "two.example.com:51820");
        assert!(same_network(&s, &backup));
        let next = profile_change(&backup, json!({"action":"activate","id":2})).unwrap();
        assert!(next.enabled);
        assert_eq!(next.mode, s.mode);
        assert_eq!(next.macs, s.macs);
        assert_eq!(
            next.tunnel.as_ref().unwrap().endpoint,
            "two.example.com:51820"
        );
        assert!(!same_network(&backup, &next));
        let renamed =
            profile_change(&next, json!({"action":"rename","id":2,"name":"Office"})).unwrap();
        assert!(same_network(&next, &renamed));
        let deleted = profile_change(&renamed, json!({"action":"delete","id":1})).unwrap();
        assert!(same_network(&renamed, &deleted));
    }
    #[test]
    fn active_deletion_requires_off_and_never_chooses_another_profile() {
        let mut s = saved(&Settings::default(), "One", "one.example.com:51820");
        s.enabled = true;
        assert!(profile_change(&s, json!({"action":"delete","id":1})).is_err());
        s.enabled = false;
        let s = profile_change(&s, json!({"action":"delete","id":1})).unwrap();
        assert!(s.profiles.is_empty() && s.tunnel.is_none() && s.active_profile.is_none());
        let s = saved(&s, "New", "new.example.com:51820");
        assert!(!s.enabled);
        assert_eq!(s.profiles.len(), 1);
    }
    #[test]
    fn profile_actions_reject_stale_unknown_and_mixed_updates() {
        let s = saved(&Settings::default(), "Home", "one.example.com:51820");
        for v in [
            json!({"expected_revision":0,"profile":{"action":"activate","id":1}}),
            json!({"profile":{"action":"activate","id":1}}),
            json!({"expected_revision":1,"enabled":false,"profile":{"action":"activate","id":1}}),
        ] {
            assert!(edited(&s, serde_json::from_value(v).unwrap()).is_err());
        }
        assert!(profile_change(&s, json!({"action":"activate","id":99})).is_err());
        assert!(profile_change(
            &s,
            json!({"action":"save","name":"Bad","config_text":"invalid"})
        )
        .is_err());
        let next=edited(&s,serde_json::from_value(json!({"expected_revision":s.revision,"config_text":sample().replace("vpn.example.com:51820","changed.example.com:123")})).unwrap()).unwrap();
        assert_eq!(next.profiles.len(), 1);
        assert_eq!(next.profiles[0].tunnel.endpoint, "changed.example.com:123");
        validate(&next).unwrap();
    }
    #[test]
    fn every_profile_is_redacted_and_invalid_persisted_state_is_rejected() {
        let mut s = saved(&Settings::default(), "Home", "one.example.com:51820");
        s = saved(&s, "Travel", "two.example.com:51820");
        s.profiles[1].tunnel.preshared_key = "c".repeat(43) + "=";
        let public = response(&s).to_string();
        for p in &s.profiles {
            assert!(!public.contains(&p.tunnel.private_key));
            assert!(!public.contains(&p.tunnel.public_key));
        }
        assert!(!public.contains(&s.profiles[1].tunnel.preshared_key));
        assert!(public.contains("Travel"));
        s.active_profile = Some(99);
        assert!(validate(&s).is_err());
        s.active_profile = Some(1);
        s.profiles[1].id = 1;
        assert!(validate(&s).is_err());
    }
    #[test]
    fn failed_switch_restores_previous_tunnel_and_saved_selection() {
        use std::cell::RefCell;
        let old = saved(
            &saved(&Settings::default(), "One", "one.example.com:51820"),
            "Two",
            "two.example.com:51820",
        );
        let next = profile_change(&old, json!({"action":"activate","id":2})).unwrap();
        for fail_apply in [true, false] {
            let attempts = RefCell::new(vec![]);
            let released = RefCell::new(false);
            let result = lan_policy::commit_change(
                &old,
                &next,
                |s| {
                    attempts.borrow_mut().push(s.active_profile);
                    if fail_apply && s.active_profile == Some(2) {
                        Err("Apply failed".into())
                    } else {
                        Ok(())
                    }
                },
                |s| {
                    assert!(!fail_apply, "A failed apply must never reach persistence");
                    assert_eq!(s.active_profile, Some(2));
                    Err("Disk full".into())
                },
                || {
                    *released.borrow_mut() = true;
                    Ok(())
                },
            );
            assert!(result.is_err());
            assert_eq!(*attempts.borrow(), vec![Some(2), Some(1)]);
            assert!(*released.borrow());
        }
        let mut released = false;
        let error = lan_policy::commit_change(
            &old,
            &next,
            |_| Err("Apply failed".into()),
            |_| panic!("must not persist"),
            || {
                released = true;
                Ok(())
            },
        )
        .unwrap_err();
        assert!(error.contains("previous policy could not be restored"));
        assert!(!released);
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
            fs::write(p.join("cleanup.sh"), SPEC.cleanup()).unwrap();
            fs::write(p.join("hold.sh"), SPEC.hold()).unwrap();
            fs::write(p.join("release.sh"), SPEC.release()).unwrap();
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
        assert!(!lan_policy::mac("02:11:22:33:44:55;id"));
        assert!(!lan_policy::mac("ff:ff:ff:ff:ff:ff"));
        assert!(!endpoint("--help:80"));
        assert!(!endpoint("a\n:80"));
        assert!(!address("0.0.0.0/0"));
        assert!(!key(&("A".repeat(43) + "=")));
    }
}
