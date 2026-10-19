//! sing-box gateway. The active imported configuration runs as one supervised child
//! process; selected LAN devices reach it through the shared MAC policy: TCP is
//! NAT-redirected to its `redirect` inbound and UDP is routed into its TUN.
//! (TPROXY cannot be used: bridge netfilter drops its socket assignment.)
//! Imported configurations and subscription URLs can contain credentials and are
//! never returned by the API.
use crate::{lan_policy, storage};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const DIR: &str = "/data/local/tmp/openui-singbox";
const CONFIG: &str = "/data/local/tmp/openui-singbox/config.json";
const PROFILES: &str = "/data/local/tmp/openui-singbox/profiles";
const RUN: &str = "/data/local/tmp/openui-singbox/run.json";
const CHECK: &str = "/data/local/tmp/openui-singbox/check.json";
const PIDFILE: &str = "/data/local/tmp/openui-singbox/sing-box.pid";
const BIN: &str = "/data/bin/sing-box";
const CURL: &str = "/usr/bin/curl";
// Redirected TCP arrives here; the policy refuses direct connections to it.
const REDIRECT_PORT: u16 = 9096;
const IFACE: &str = "ou-sb0";
const TUN_ADDRESS: &str = "172.19.0.1/30";
// Marked client UDP DNS is DNATed here; the address routes into the TUN and is hijacked.
const DNS: &str = "172.19.0.2";
const API: &str = "127.0.0.1:9095";
const TEST_URL: &str = "https://www.gstatic.com/generate_204";
const MAX_PROFILES: usize = 5;
const MAX_CONFIG: usize = 512 * 1024;
const LOG_LINES: usize = 200;
// First starts download remote rule sets before the listener opens; only a
// crash within the first seconds fails a start, and a stalled start is retried.
const EARLY_EXIT: Duration = Duration::from_secs(3);
const STARTUP_LIMIT: Duration = Duration::from_secs(300);
const SPEC: lan_policy::Spec = lan_policy::Spec {
    name: "sing-box",
    prefix: "OUSB",
    table: "10430",
    mark: "0x20000000",
    dir: DIR,
    kind: lan_policy::Kind::Proxy {
        iface: IFACE,
        port: REDIRECT_PORT,
    },
};
static LOCK: Mutex<()> = Mutex::new(());
static ERROR: Mutex<Option<String>> = Mutex::new(None);
static CHILD: Mutex<Option<Running>> = Mutex::new(None);
static LOG: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
// Bumped on every start and stop so a stale TUN-route waiter gives up.
static GENERATION: AtomicU64 = AtomicU64::new(0);
// Binary (mtime, size) and the version it reported.
type VersionCache = Option<((u64, u64), Option<String>)>;
static VERSION: Mutex<VersionCache> = Mutex::new(None);

struct Running {
    child: Child,
    started: Instant,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Settings {
    revision: u64,
    enabled: bool,
    mode: String,
    macs: Vec<String>,
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
            profiles: vec![],
            active_profile: None,
        }
    }
}
/// Profile metadata. The configuration itself is stored by content digest so a
/// failed switch or refresh can always restore the previous file.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    id: u64,
    name: String,
    #[serde(default)]
    url: Option<String>,
    digest: String,
    nodes: usize,
    updated: u64,
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum ProfileAction {
    Save { name: String, config_text: String },
    Subscribe { name: String, url: String },
    Refresh { id: u64 },
    Activate { id: u64 },
    Rename { id: u64, name: String },
    Delete { id: u64 },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    expected_revision: u64,
    enabled: Option<bool>,
    mode: Option<String>,
    macs: Option<Vec<String>>,
    devices: Option<Vec<lan_policy::DeviceChoice>>,
    profile: Option<ProfileAction>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn blob(digest: &str) -> String {
    format!("{PROFILES}/{digest}.json")
}
fn profile_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 48 || name.chars().any(char::is_control) {
        return Err("Profile name must contain 1–48 characters without control characters".into());
    }
    Ok(name.into())
}
fn subscription_url(url: &str) -> Result<String, String> {
    let url = url.trim();
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or("Subscription URL must start with https:// or http://")?;
    if rest.is_empty() || url.len() > 2048 || !url.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("Invalid subscription URL".into());
    }
    Ok(url.into())
}
/// Host only: subscription paths and queries usually carry an access token.
fn url_host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    authority.rsplit('@').next().unwrap_or("").into()
}

const NON_NODES: [&str; 5] = ["direct", "block", "dns", "selector", "urltest"];

/// Turn an imported configuration into the router's runtime configuration. Rejects
/// anything that would read local files, run programs, set routing marks or open
/// listeners; returns the configuration and its proxy-node count.
fn runtime(original: &Value, cache_id: &str, secret: &str) -> Result<(Value, usize), String> {
    let mut c = original
        .as_object()
        .cloned()
        .ok_or("Configuration must be a sing-box JSON object")?;
    // Keep imported inbound tags so rules that reference them still match.
    let imported = |kinds: &[&str], default: &str| {
        c.get("inbounds")
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find(|i| kinds.iter().any(|k| i["type"] == *k)))
            .and_then(|i| i["tag"].as_str())
            .unwrap_or(default)
            .to_string()
    };
    let tun_tag = imported(&["tun"], "tun-in");
    let mut redirect_tag = imported(&["redirect", "tproxy"], "redirect-in");
    if redirect_tag == tun_tag {
        redirect_tag = format!("{tun_tag}-redirect");
    }
    for removed in ["inbounds", "services", "log", "experimental"] {
        c.remove(removed);
    }
    if let Some(ntp) = c.get_mut("ntp").and_then(Value::as_object_mut) {
        ntp.remove("write_to_system");
    }
    if let Some(route) = c.get_mut("route").and_then(Value::as_object_mut) {
        route.remove("default_mark");
    }
    forbidden(&Value::Object(c.clone()))?;
    let mut outbounds = c
        .get("outbounds")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let endpoints = c
        .get("endpoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut tags = HashSet::new();
    let mut nodes = vec![];
    for (o, endpoint) in outbounds
        .iter()
        .map(|o| (o, false))
        .chain(endpoints.iter().map(|o| (o, true)))
    {
        let kind = o["type"].as_str().ok_or("Every outbound needs a type")?;
        let tag = o["tag"].as_str().unwrap_or(kind);
        if !tags.insert(tag.to_string()) {
            return Err(format!("Duplicate outbound tag: {tag}"));
        }
        if kind == "tor" {
            return Err("Tor outbounds are not supported on the router".into());
        }
        if endpoint && o["system"] == true {
            return Err("System interfaces are not supported; remove \"system\": true".into());
        }
        if endpoint || !NON_NODES.contains(&kind) {
            nodes.push(tag.to_string());
        }
    }
    if nodes.is_empty() {
        return Err("No proxy outbounds found in the configuration".into());
    }
    let route = c
        .entry("route")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("route must be an object")?;
    if let Some(sets) = route.get("rule_set").and_then(Value::as_array) {
        if sets.iter().any(|r| r["type"] == "local") {
            return Err("Local rule sets are not supported; use inline or remote rule sets".into());
        }
    }
    // A configuration without a selector still gets one switchable group.
    if !outbounds.iter().any(|o| o["type"] == "selector") && !tags.contains("OpenUI") {
        outbounds.push(json!({"type":"selector","tag":"OpenUI","outbounds":nodes,"interrupt_exist_connections":true}));
        route.entry("final").or_insert_with(|| json!("OpenUI"));
    }
    let mut rules = vec![
        json!({"inbound":[&tun_tag,&redirect_tag],"port":53,"action":"hijack-dns"}),
        json!({"inbound":[&tun_tag,&redirect_tag],"action":"sniff"}),
        json!({"protocol":"dns","action":"hijack-dns"}),
    ];
    if let Some(existing) = route.get("rules").and_then(Value::as_array) {
        rules.extend(existing.iter().cloned());
    }
    route.insert("rules".into(), Value::Array(rules));
    c.insert("outbounds".into(), Value::Array(outbounds));
    c.insert(
        "inbounds".into(),
        json!([{"type":"redirect","tag":redirect_tag,"listen":"0.0.0.0","listen_port":REDIRECT_PORT},
            {"type":"tun","tag":tun_tag,"interface_name":IFACE,"address":[TUN_ADDRESS],"mtu":9000,"auto_route":false}]),
    );
    c.insert("log".into(), json!({"level":"warn","timestamp":true}));
    c.insert(
        "experimental".into(),
        json!({"clash_api":{"external_controller":API,"secret":secret},"cache_file":{"enabled":true,"path":"cache.db","cache_id":cache_id}}),
    );
    Ok((Value::Object(c), nodes.len()))
}
fn forbidden(v: &Value) -> Result<(), String> {
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                if k.ends_with("_path") || k.ends_with("_directory") || k == "routing_mark" {
                    return Err(format!(
                        "Unsupported field \"{k}\": local files and routing marks are not allowed on the router"
                    ));
                }
                forbidden(v)?;
            }
        }
        Value::Array(a) => a.iter().try_for_each(forbidden)?,
        _ => {}
    }
    Ok(())
}
/// Validate an imported configuration; returns its digest and node count.
fn inspect(bytes: &[u8]) -> Result<(String, usize), String> {
    if bytes.len() > MAX_CONFIG {
        return Err("Configuration exceeds 512 KiB".into());
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| {
        "Not a sing-box JSON configuration (share links and Clash YAML are not supported)"
            .to_string()
    })?;
    let (_, nodes) = runtime(&value, "check", "check")?;
    Ok((digest(bytes), nodes))
}

fn add_profile(
    s: &mut Settings,
    name: String,
    url: Option<String>,
    bytes: &[u8],
) -> Result<(), String> {
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
    let (digest, nodes) = inspect(bytes)?;
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
    }
    s.profiles.push(Profile {
        id,
        name,
        url,
        digest,
        nodes,
        updated: now(),
    });
    Ok(())
}
/// Applies a profile action; returns new configuration bytes that must be stored.
fn profile_edit(
    s: &mut Settings,
    action: ProfileAction,
    download: Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>, String> {
    match action {
        ProfileAction::Save { name, config_text } => {
            let bytes = config_text.into_bytes();
            add_profile(s, name, None, &bytes)?;
            return Ok(Some(bytes));
        }
        ProfileAction::Subscribe { name, url } => {
            let url = subscription_url(&url)?;
            let bytes = download.ok_or("Subscription was not downloaded")?;
            add_profile(s, name, Some(url), &bytes)?;
            return Ok(Some(bytes));
        }
        ProfileAction::Refresh { id } => {
            let p = s
                .profiles
                .iter_mut()
                .find(|p| p.id == id)
                .ok_or("Profile not found")?;
            if p.url.is_none() {
                return Err("Only subscription profiles can be refreshed".into());
            }
            let bytes = download.ok_or("Subscription was not downloaded")?;
            (p.digest, p.nodes) = inspect(&bytes)?;
            p.updated = now();
            return Ok(Some(bytes));
        }
        ProfileAction::Activate { id } => {
            if !s.profiles.iter().any(|p| p.id == id) {
                return Err("Profile not found".into());
            }
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
                        "Switch profiles or turn sing-box off before deleting the active profile"
                            .into(),
                    );
                }
                s.active_profile = None;
            }
            s.profiles.retain(|p| p.id != id);
        }
    }
    Ok(None)
}
fn edited(
    old: &Settings,
    u: Update,
    download: Option<Vec<u8>>,
) -> Result<(Settings, Option<Vec<u8>>), String> {
    if u.expected_revision != old.revision {
        return Err("Configuration changed; refresh before saving".into());
    }
    if u.devices.is_some() && (u.mode.is_some() || u.macs.is_some()) {
        return Err("Cannot combine device changes with a routing mode change".into());
    }
    if u.profile.is_some()
        && (u.enabled.is_some() || u.mode.is_some() || u.macs.is_some() || u.devices.is_some())
    {
        return Err("Profile actions cannot be combined with routing changes".into());
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
        lan_policy::apply_devices(&next.mode, &mut next.macs, choices)?;
    }
    let mut stored = None;
    if let Some(action) = u.profile {
        stored = profile_edit(&mut next, action, download)?;
    }
    next.macs.sort();
    next.macs.dedup();
    validate(&next)?;
    Ok((next, stored))
}
fn active(s: &Settings) -> Option<&Profile> {
    s.profiles.iter().find(|p| Some(p.id) == s.active_profile)
}
fn same_network(a: &Settings, b: &Settings) -> bool {
    a.enabled == b.enabled
        && a.mode == b.mode
        && a.macs == b.macs
        && a.active_profile == b.active_profile
        && active(a).map(|p| &p.digest) == active(b).map(|p| &p.digest)
}
fn validate(s: &Settings) -> Result<(), String> {
    if s.profiles.len() > MAX_PROFILES {
        return Err("Too many saved sing-box profiles".into());
    }
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    for p in &s.profiles {
        if p.id == 0
            || !ids.insert(p.id)
            || profile_name(&p.name)? != p.name
            || !names.insert(p.name.to_lowercase())
            || p.digest.len() != 64
            || !p.digest.bytes().all(|b| b.is_ascii_hexdigit())
            || p.url
                .as_deref()
                .is_some_and(|u| subscription_url(u).is_err())
        {
            return Err("Invalid or duplicate saved profile".into());
        }
    }
    if s.active_profile.is_some() && active(s).is_none() {
        return Err("Active profile not found".into());
    }
    lan_policy::validate_selection(&s.mode, &s.macs)?;
    if s.enabled && s.active_profile.is_none() {
        return Err("Import a sing-box configuration first".into());
    }
    Ok(())
}
fn load() -> Result<Settings, String> {
    match fs::read(CONFIG) {
        Ok(b) => {
            let s: Settings = serde_json::from_slice(&b)
                .map_err(|_| "Saved sing-box configuration is invalid")?;
            validate(&s)?;
            Ok(s)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(_) => Err("Cannot read saved sing-box configuration".into()),
    }
}
fn persist(s: &Settings) -> Result<(), String> {
    storage::atomic_write(Path::new(CONFIG), &serde_json::to_vec(s).unwrap())
        .map_err(|_| "Cannot persist sing-box configuration".into())
}
fn ensure_dirs() -> Result<(), String> {
    for dir in [DIR, PROFILES] {
        fs::create_dir_all(dir).map_err(|_| "Cannot create sing-box state directory")?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .map_err(|_| "Cannot protect sing-box state directory")?;
    }
    Ok(())
}
/// Remove stored configurations no longer referenced by the saved settings.
fn collect_garbage(s: &Settings) {
    let keep: HashSet<String> = s
        .profiles
        .iter()
        .map(|p| format!("{}.json", p.digest))
        .collect();
    if let Ok(entries) = fs::read_dir(PROFILES) {
        for e in entries.flatten() {
            if !keep.contains(&*e.file_name().to_string_lossy()) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

/// Clash API credential; a fresh value per agent process, never sent to clients.
fn secret() -> &'static str {
    static SECRET: OnceLock<String> = OnceLock::new();
    SECRET.get_or_init(|| {
        let mut bytes = [0u8; 16];
        if fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut bytes))
            .is_err()
        {
            bytes.copy_from_slice(
                &Sha256::digest(format!("{:?}{}", SystemTime::now(), std::process::id()))[..16],
            );
        }
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    })
}
fn runtime_for(s: &Settings) -> Result<Value, String> {
    let p = active(s).ok_or("Import a sing-box configuration first")?;
    let bytes = fs::read(blob(&p.digest)).map_err(|_| "Saved sing-box profile is missing")?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "Saved sing-box profile is invalid")?;
    Ok(runtime(&value, &format!("profile-{}", p.id), secret())?.0)
}
fn clean(text: &str) -> String {
    strip_ansi(text)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(400)
        .collect()
}
/// Validate a runtime configuration with the installed binary, without starting it.
fn check(config: &Value) -> Result<(), String> {
    if !Path::new(BIN).exists() {
        return Ok(());
    }
    ensure_dirs()?;
    storage::atomic_write(Path::new(CHECK), config.to_string().as_bytes())
        .map_err(|_| "Cannot write sing-box configuration")?;
    let out = process_runner::output(
        Command::new(BIN).args(["check", "-c", CHECK, "-D", DIR]),
        None,
        Duration::from_secs(30),
        65536,
    );
    let _ = fs::remove_file(CHECK);
    let out = out.map_err(|_| "sing-box configuration check timed out")?;
    if !out.status.success() {
        let msg = clean(&String::from_utf8_lossy(&out.stderr));
        return Err(format!("sing-box rejected the configuration: {msg}"));
    }
    Ok(())
}
fn version() -> Option<String> {
    let meta = fs::metadata(BIN).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    let key = (mtime, meta.len());
    let mut cache = VERSION.lock().unwrap();
    if let Some((k, v)) = cache.as_ref() {
        if *k == key {
            return v.clone();
        }
    }
    let v = process_runner::output(
        Command::new(BIN).arg("version"),
        None,
        Duration::from_secs(10),
        65536,
    )
    .ok()
    .filter(|o| o.status.success())
    .and_then(|o| {
        let text = String::from_utf8_lossy(&o.stdout).into_owned();
        text.lines()
            .next()?
            .strip_prefix("sing-box version ")
            .map(|v| v.trim().to_string())
    });
    *cache = Some((key, v.clone()));
    v
}

/// sing-box colors its console output; keep logs and errors as plain text.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}
fn push_log(line: String) {
    let mut log = LOG.lock().unwrap();
    if log.len() >= LOG_LINES {
        log.pop_front();
    }
    log.push_back(strip_ansi(&line).chars().take(500).collect());
}
fn running_pid() -> Option<u32> {
    let mut child = CHILD.lock().unwrap();
    let r = child.as_mut()?;
    match r.child.try_wait() {
        Ok(None) => Some(r.child.id()),
        result => {
            let status = result
                .ok()
                .flatten()
                .map_or("unknown status".into(), |s| s.to_string());
            push_log(format!("[openui] sing-box exited ({status})"));
            *child = None;
            None
        }
    }
}
fn signal(pid: u32, sig: i32) {
    unsafe {
        libc::kill(pid as i32, sig);
    }
}
fn stop_child() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    // Release the lock before waiting so status reads are not blocked.
    let taken = CHILD.lock().unwrap().take();
    if let Some(mut r) = taken {
        signal(r.child.id(), libc::SIGTERM);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && matches!(r.child.try_wait(), Ok(None)) {
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = r.child.kill();
        let _ = r.child.wait();
        push_log("[openui] sing-box stopped".into());
    }
    let _ = fs::remove_file(PIDFILE);
}
/// A previous agent instance may have left its child running.
fn kill_stale() {
    let Some(pid) = fs::read_to_string(PIDFILE)
        .ok()
        .and_then(|p| p.trim().parse::<u32>().ok())
    else {
        return;
    };
    let ours = fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| {
        let c = String::from_utf8_lossy(&c);
        c.contains(BIN) && c.contains(RUN)
    });
    if ours {
        signal(pid, libc::SIGTERM);
        for _ in 0..50 {
            if !Path::new(&format!("/proc/{pid}")).exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if Path::new(&format!("/proc/{pid}")).exists() {
            signal(pid, libc::SIGKILL);
        }
    }
    let _ = fs::remove_file(PIDFILE);
}
fn start_child() -> Result<(), String> {
    stop_child();
    let mut cmd = Command::new(BIN);
    cmd.args(["run", "-c", RUN, "-D", DIR])
        .env("GOMEMLIMIT", "96MiB")
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|_| "Cannot start sing-box")?;
    let out = child
        .stdout
        .take()
        .map(|p| Box::new(p) as Box<dyn Read + Send>);
    let err = child
        .stderr
        .take()
        .map(|p| Box::new(p) as Box<dyn Read + Send>);
    for pipe in [out, err].into_iter().flatten() {
        std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                push_log(line);
            }
        });
    }
    let _ = storage::atomic_write(Path::new(PIDFILE), child.id().to_string().as_bytes());
    push_log("[openui] sing-box started".into());
    *CHILD.lock().unwrap() = Some(Running {
        child,
        started: Instant::now(),
    });
    // The TUN appears only after rule sets are downloaded; route into it then.
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        let deadline = Instant::now() + STARTUP_LIMIT;
        while GENERATION.load(Ordering::SeqCst) == generation && Instant::now() < deadline {
            if SPEC.interface_exists() {
                match attach_route() {
                    Ok(()) => push_log("[openui] TUN route attached".into()),
                    Err(e) => push_log(format!("[openui] {e}")),
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    });
    // Selected devices stay blocked by the policy until sing-box is ready.
    let deadline = Instant::now() + EARLY_EXIT;
    while !ready() && Instant::now() < deadline {
        if running_pid().is_none() {
            // Let the pipe readers capture the final error line.
            std::thread::sleep(Duration::from_millis(300));
            return Err(format!("sing-box exited during startup: {}", last_log()));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}
fn last_log() -> String {
    LOG.lock()
        .unwrap()
        .iter()
        .rev()
        .find(|l| !l.starts_with("[openui]"))
        .cloned()
        .unwrap_or_default()
}
fn startup_age() -> Duration {
    CHILD
        .lock()
        .unwrap()
        .as_ref()
        .map_or(Duration::ZERO, |r| r.started.elapsed())
}
/// Loose reverse-path checking on the TUN (replies arrive from it), then route into it.
fn attach_route() -> Result<(), String> {
    fs::write(format!("/proc/sys/net/ipv4/conf/{IFACE}/rp_filter"), "2\n")
        .map_err(|_| "Cannot set TUN reverse-path check")?;
    SPEC.script(&SPEC.route_script())
}
/// The TUN exists and the redirect listener accepts connections.
fn ready() -> bool {
    let local = format!("00000000:{REDIRECT_PORT:04X}");
    SPEC.interface_exists()
        && fs::read_to_string("/proc/net/tcp").is_ok_and(|t| {
            t.lines().skip(1).any(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                f.get(1) == Some(&local.as_str()) && f.get(3) == Some(&"0A")
            })
        })
}
fn kernel_supported() -> bool {
    Path::new("/dev/net/tun").exists()
        && fs::read_to_string("/proc/net/ip_tables_targets")
            .is_ok_and(|t| t.lines().any(|l| l == "REDIRECT"))
}
fn prerequisites() -> Result<(), String> {
    if !Path::new(BIN).exists() {
        return Err("sing-box is not installed: deploy /data/bin/sing-box".into());
    }
    if !kernel_supported() {
        return Err("Kernel TUN (/dev/net/tun) or iptables REDIRECT support is unavailable".into());
    }
    Ok(())
}
/// Install the fail-closed policy first, then start the proxy. Until it is ready,
/// redirected TCP is refused and UDP hits the unreachable fallback.
fn install(s: &Settings) -> Result<(), String> {
    stop_child();
    if !s.enabled {
        if SPEC.is_owned() {
            SPEC.script(&SPEC.cleanup())?;
        }
        return Ok(());
    }
    validate(s)?;
    prerequisites()?;
    let config = runtime_for(s)?;
    storage::atomic_write(Path::new(RUN), config.to_string().as_bytes())
        .map_err(|_| "Cannot write sing-box configuration")?;
    SPEC.claim_resources()?;
    SPEC.script(&SPEC.cleanup())?;
    SPEC.script(&SPEC.proxy_policy(&s.mode, &s.macs, DNS))
}
fn apply(s: &Settings) -> Result<(), String> {
    install(s)?;
    if s.enabled {
        start_child()?;
    }
    Ok(())
}

fn profile_status(s: &Settings) -> Value {
    json!(s.profiles.iter().map(|p| json!({"id":p.id,"name":p.name,"source":if p.url.is_some() {"subscription"} else {"file"},
        "host":p.url.as_deref().map(url_host),"nodes":p.nodes,"updated":p.updated})).collect::<Vec<_>>())
}
fn response(s: &Settings) -> Value {
    let pid = running_pid();
    let uptime = CHILD
        .lock()
        .unwrap()
        .as_ref()
        .map_or(0, |r| r.started.elapsed().as_secs());
    let rss = pid
        .and_then(|p| fs::read_to_string(format!("/proc/{p}/status")).ok())
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("VmRSS:"))
                .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
        })
        .map_or(0, |kb| kb * 1024);
    let traffic = pid
        .and_then(|_| clash("GET", "/connections", None, Duration::from_secs(2)).ok())
        .map(|(_, v)| v)
        .unwrap_or(Value::Null);
    let ready = pid.is_some() && ready();
    let healthy = !s.enabled || (ready && SPEC.healthy());
    // Rule-set downloads delay the TUN and listener; that is progress, not an error.
    let starting = s.enabled && pid.is_some() && !ready;
    let error = ERROR.lock().unwrap().clone().or_else(|| {
        if healthy || starting {
            None
        } else {
            let last = last_log();
            Some(if last.is_empty() {
                "sing-box is not running; automatic recovery is pending".into()
            } else {
                format!(
                    "sing-box is not running; automatic recovery is pending. Last message: {last}"
                )
            })
        }
    });
    json!({"profiles":profile_status(s),"active_profile":s.active_profile,"max_profiles":MAX_PROFILES,"revision":s.revision,
        "enabled":s.enabled,"mode":s.mode,"macs":s.macs,"has_config":s.active_profile.is_some(),
        "binary_installed":Path::new(BIN).exists(),"version":version(),"kernel_supported":kernel_supported(),
        "running":pid.is_some(),"pid":pid,"uptime":uptime,"memory_rss":rss,"ready":ready,"starting":starting,"healthy":s.enabled && healthy,
        "rx_bytes":traffic["downloadTotal"].as_u64().unwrap_or(0),"tx_bytes":traffic["uploadTotal"].as_u64().unwrap_or(0),
        "error":error,"wireguard_enabled":crate::wireguard::enabled(),"ipv6_policy":"blocked_for_proxied_devices"})
}
pub fn get() -> (u16, Value) {
    let _guard = LOCK.lock().unwrap();
    match load() {
        Ok(s) => (200, json!({"ok":true,"data":response(&s)})),
        Err(e) => (500, json!({"ok":false,"error":e})),
    }
}
fn download(url: &str) -> Result<Vec<u8>, String> {
    if !Path::new(CURL).exists() {
        return Err("curl is unavailable on this device".into());
    }
    let agent = format!("sing-box/{}", version().unwrap_or_else(|| "1.14".into()));
    let max = MAX_CONFIG.to_string();
    let out = process_runner::output(
        Command::new(CURL).args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--max-redirs",
            "5",
            "--proto",
            "=http,https",
            "--proto-redir",
            "=http,https",
            "--connect-timeout",
            "10",
            "--max-time",
            "30",
            "--max-filesize",
            &max,
            "--user-agent",
            &agent,
            "--url",
            url,
        ]),
        None,
        Duration::from_secs(40),
        MAX_CONFIG + 1,
    )
    .map_err(|_| "Subscription download timed out or exceeds 512 KiB")?;
    // curl messages can echo the URL and its token; report the exit reason only.
    if !out.status.success() {
        return Err(match out.status.code() {
            Some(6) => "Cannot resolve the subscription host",
            Some(7) => "Cannot connect to the subscription server",
            Some(22) => "The subscription server returned an error",
            Some(28) => "Subscription download timed out",
            Some(35 | 60 | 77) => "TLS verification failed for the subscription server",
            Some(63) => "Subscription exceeds 512 KiB",
            _ => "Subscription download failed",
        }
        .into());
    }
    Ok(out.stdout)
}
pub fn update(body: &[u8]) -> (u16, Value) {
    let result = (|| -> Result<Value, String> {
        let u: Update = serde_json::from_slice(body).map_err(|_| "Invalid sing-box request")?;
        // Subscriptions download before any lock is held.
        let url = match &u.profile {
            Some(ProfileAction::Subscribe { url, .. }) => Some(subscription_url(url)?),
            Some(ProfileAction::Refresh { id }) => load()?
                .profiles
                .iter()
                .find(|p| p.id == *id)
                .and_then(|p| p.url.clone()),
            _ => None,
        };
        let download = url.as_deref().map(download).transpose()?;
        let _gate = lan_policy::GATE.lock().unwrap();
        let _guard = LOCK.lock().unwrap();
        let old = load()?;
        let (next, stored) = edited(&old, u, download)?;
        if next.enabled && crate::wireguard::enabled() {
            return Err("Turn WireGuard off before enabling sing-box".into());
        }
        ensure_dirs()?;
        if let Some(bytes) = &stored {
            let value: Value = serde_json::from_slice(bytes).unwrap();
            check(&runtime(&value, "check", secret())?.0)?;
            storage::atomic_write(Path::new(&blob(&digest(bytes))), bytes)
                .map_err(|_| "Cannot store sing-box profile")?;
        }
        // Saving/renaming/deleting an inactive profile must not restart the proxy.
        if same_network(&old, &next) {
            persist(&next)?;
            collect_garbage(&next);
            return Ok(response(&next));
        }
        // Validate prerequisites before interrupting any traffic.
        if next.enabled {
            prerequisites()?;
            if stored.is_none() {
                check(&runtime_for(&next)?)?;
            }
            SPEC.claim_resources()?;
        }
        let changing_network = old.enabled || next.enabled || SPEC.is_owned();
        if changing_network {
            SPEC.script(&SPEC.hold())?;
        }
        let result = lan_policy::commit_change(&old, &next, apply, persist, || {
            if changing_network {
                SPEC.script(&SPEC.release())
            } else {
                Ok(())
            }
        });
        collect_garbage(if result.is_ok() { &next } else { &old });
        if let Err(e) = result {
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
/// Persisted state only; used to keep gateway features mutually exclusive.
pub fn enabled() -> bool {
    load().is_ok_and(|s| s.enabled)
}

fn escape(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
/// Minimal HTTP/1.0 client for the loopback-only Clash API (no chunked encoding).
fn clash(
    method: &str,
    path: &str,
    body: Option<&Value>,
    timeout: Duration,
) -> Result<(u16, Value), String> {
    let unavailable = || "sing-box control API is unavailable".to_string();
    let addr: SocketAddr = API.parse().unwrap();
    let mut stream =
        TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|_| unavailable())?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
    let body = body.map(Value::to_string).unwrap_or_default();
    write!(stream, "{method} {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", secret(), body.len())
        .map_err(|_| unavailable())?;
    let mut raw = Vec::new();
    stream
        .take(1024 * 1024 + 1)
        .read_to_end(&mut raw)
        .map_err(|_| "sing-box control API timed out".to_string())?;
    if raw.len() > 1024 * 1024 {
        return Err("sing-box control API response is too large".into());
    }
    let text = String::from_utf8_lossy(&raw);
    let (head, payload) = text.split_once("\r\n\r\n").ok_or_else(unavailable)?;
    let code = head
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(unavailable)?;
    Ok((code, serde_json::from_str(payload).unwrap_or(Value::Null)))
}
fn last_delay(info: &Value) -> Value {
    info["history"]
        .as_array()
        .and_then(|h| h.last())
        .map_or(Value::Null, |h| h["delay"].clone())
}
/// Groups in configuration order. Node tags and types only; no server details.
fn groups() -> Result<Vec<Value>, String> {
    if running_pid().is_none() {
        return Ok(vec![]);
    }
    let (code, v) = clash("GET", "/proxies", None, Duration::from_secs(5))?;
    let proxies = v["proxies"]
        .as_object()
        .filter(|_| code == 200)
        .ok_or("sing-box control API returned an invalid response")?;
    let order: HashMap<String, usize> = fs::read(RUN)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .map(|c| {
            let tags = c["outbounds"].as_array().cloned().unwrap_or_default();
            tags.iter()
                .filter_map(|o| o["tag"].as_str().map(String::from))
                .enumerate()
                .map(|(i, t)| (t, i))
                .collect()
        })
        .unwrap_or_default();
    let mut out: Vec<(usize, Value)> = proxies
        .iter()
        .filter(|(name, p)| p["all"].is_array() && name.as_str() != "GLOBAL")
        .map(|(name, p)| {
            let options: Vec<Value> = p["all"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .map(|o| json!({"name":o,"type":proxies.get(o).map_or(Value::Null, |i| i["type"].clone()),"delay":proxies.get(o).map_or(Value::Null, last_delay)}))
                .collect();
            let group = json!({"name":name,"type":p["type"],"now":p["now"],"selectable":p["type"] == "Selector","options":options});
            (order.get(name).copied().unwrap_or(usize::MAX), group)
        })
        .collect();
    out.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1["name"].as_str().cmp(&b.1["name"].as_str()))
    });
    Ok(out.into_iter().map(|(_, g)| g).collect())
}
fn wrap(result: Result<Value, String>) -> (u16, Value) {
    match result {
        Ok(v) => (200, json!({"ok":true,"data":v})),
        Err(e) => (400, json!({"ok":false,"error":e})),
    }
}
pub fn proxies() -> (u16, Value) {
    wrap(groups().map(|g| json!({"running":running_pid().is_some(),"groups":g})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Select {
    group: String,
    name: String,
}
pub fn select(body: &[u8]) -> (u16, Value) {
    wrap((|| {
        let r: Select = serde_json::from_slice(body).map_err(|_| "Invalid node selection")?;
        let groups = groups()?;
        let group = groups
            .iter()
            .find(|g| g["name"] == r.group.as_str())
            .ok_or("sing-box is not running or the group does not exist")?;
        if group["selectable"] != true {
            return Err("This group selects nodes automatically".into());
        }
        if !group["options"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["name"] == r.name.as_str())
        {
            return Err("Node not found in this group".into());
        }
        let path = format!("/proxies/{}", escape(&r.group));
        let (code, _) = clash(
            "PUT",
            &path,
            Some(&json!({"name":r.name})),
            Duration::from_secs(5),
        )?;
        if !(200..300).contains(&code) {
            return Err("sing-box rejected the node selection".into());
        }
        push_log(format!("[openui] {} → {}", r.group, r.name));
        Ok(json!({"running":true,"groups":groups_or_empty()}))
    })())
}
fn groups_or_empty() -> Vec<Value> {
    groups().unwrap_or_default()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DelayRequest {
    group: String,
}
pub fn delay(body: &[u8]) -> (u16, Value) {
    wrap((|| {
        let r: DelayRequest = serde_json::from_slice(body).map_err(|_| "Invalid latency test")?;
        let groups = groups()?;
        let group = groups
            .iter()
            .find(|g| g["name"] == r.group.as_str())
            .ok_or("sing-box is not running or the group does not exist")?;
        let names: Vec<String> = group["options"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|o| o["name"].as_str().map(String::from))
            .take(128)
            .collect();
        let results = Mutex::new(HashMap::new());
        for chunk in names.chunks(8) {
            std::thread::scope(|scope| {
                for name in chunk {
                    let results = &results;
                    scope.spawn(move || {
                        let path = format!(
                            "/proxies/{}/delay?url={}&timeout=5000",
                            escape(name),
                            escape(TEST_URL)
                        );
                        let delay = clash("GET", &path, None, Duration::from_secs(8))
                            .ok()
                            .filter(|(code, _)| *code == 200)
                            .and_then(|(_, v)| v["delay"].as_u64())
                            .unwrap_or(0);
                        results.lock().unwrap().insert(name.clone(), delay);
                    });
                }
            });
        }
        let results = results.into_inner().unwrap();
        let mut groups = groups_or_empty();
        for g in &mut groups {
            for o in g["options"].as_array_mut().into_iter().flatten() {
                if let Some(d) = o["name"].as_str().and_then(|n| results.get(n)) {
                    o["delay"] = json!(d);
                }
            }
        }
        Ok(json!({"running":true,"groups":groups}))
    })())
}
// ── Device screen ────────────────────────────────────────────────────────────
// Redacted projections for the root-only screen socket: names, states and
// counters only; never configuration, credentials, servers or subscription links.

/// The first manual selector group, as shown on the screen's Nodes page.
#[derive(Clone)]
struct MainGroup {
    name: String,
    now: String,
    /// (name, type, last delay in ms: -1 untested / 0 failed, auto-group pick)
    options: Vec<(String, String, i64, String)>,
    version: u64,
}
// The Clash API is local, but avoid querying it on every 2-second screen poll.
type GroupCache = Option<(Instant, Option<MainGroup>)>;
static MAIN_GROUP: Mutex<GroupCache> = Mutex::new(None);
fn screen_text(s: &str) -> String {
    let s: String = s.chars().filter(|c| !c.is_control()).take(48).collect();
    if s.is_empty() {
        "-".into()
    } else {
        s
    }
}
fn main_group(fresh: bool) -> Option<MainGroup> {
    if running_pid().is_none() || !ready() {
        return None;
    }
    let mut cache = MAIN_GROUP.lock().unwrap();
    if let Some((at, group)) = cache.as_ref() {
        if !fresh && at.elapsed() < Duration::from_secs(5) {
            return group.clone();
        }
    }
    let group = groups().ok().and_then(|groups| {
        let picks: HashMap<&str, &str> = groups
            .iter()
            .filter_map(|g| Some((g["name"].as_str()?, g["now"].as_str()?)))
            .collect();
        let g = groups.iter().find(|g| g["selectable"] == true)?;
        let name = g["name"].as_str()?.to_string();
        let options: Vec<_> = g["options"]
            .as_array()?
            .iter()
            .take(32)
            .filter_map(|o| {
                let option = o["name"].as_str()?;
                Some((
                    option.to_string(),
                    o["type"].as_str().unwrap_or("Unknown").to_string(),
                    o["delay"].as_i64().unwrap_or(-1),
                    picks.get(option).map_or(String::new(), |p| p.to_string()),
                ))
            })
            .collect();
        // FNV-1a over the group and option names: screen actions carry it so a
        // changed list can never switch to the wrong node.
        let mut version: u64 = 0xcbf29ce484222325;
        for part in std::iter::once(&name).chain(options.iter().map(|o| &o.0)) {
            for b in part.bytes().chain([0]) {
                version = (version ^ u64::from(b)).wrapping_mul(0x100000001b3);
            }
        }
        Some(MainGroup {
            now: g["now"].as_str().unwrap_or_default().to_string(),
            name,
            options,
            version,
        })
    });
    *cache = Some((Instant::now(), group.clone()));
    group
}
/// Screen view of the gateway plus its device selection for the shared device rows.
pub struct ScreenState {
    pub records: String,
    pub all: bool,
    pub macs: Vec<String>,
}
pub fn screen_state() -> Option<ScreenState> {
    let s = load().ok()?;
    let pid = running_pid();
    let is_ready = pid.is_some() && ready();
    let traffic = if is_ready {
        clash("GET", "/connections", None, Duration::from_secs(2))
            .map(|(_, v)| v)
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let rss = pid
        .and_then(|p| fs::read_to_string(format!("/proc/{p}/status")).ok())
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("VmRSS:"))
                .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
        })
        .map_or(0, |kb| kb * 1024);
    let error = ERROR.lock().unwrap().is_some() || (s.enabled && pid.is_none());
    let mut out = format!(
        "B {} {} {} {} {} {} {} {} {} {} {} {}\n",
        s.revision,
        u8::from(Path::new(BIN).exists()),
        u8::from(s.enabled),
        u8::from(pid.is_some()),
        u8::from(is_ready),
        u8::from(s.enabled && pid.is_some() && !is_ready),
        u8::from(error),
        u8::from(s.mode == "all"),
        traffic["downloadTotal"].as_u64().unwrap_or(0),
        traffic["uploadTotal"].as_u64().unwrap_or(0),
        startup_age().as_secs(),
        rss
    );
    for p in s.profiles.iter().take(MAX_PROFILES) {
        out.push_str(&format!(
            "Q\t{}\t{}\t{}\t{}\t{}\n",
            p.id,
            u8::from(Some(p.id) == s.active_profile),
            if p.url.is_some() { "s" } else { "f" },
            p.nodes,
            screen_text(&p.name)
        ));
    }
    if let Some(g) = s.enabled.then(|| main_group(false)).flatten() {
        let now = g.options.iter().position(|o| o.0 == g.now);
        out.push_str(&format!(
            "M\t{}\t{}\t{}\n",
            g.version,
            now.map_or(-1, |i| i as i64),
            screen_text(&g.name)
        ));
        for (name, kind, delay, pick) in &g.options {
            out.push_str(&format!(
                "N\t{}\t{}\t{}\t{}\n",
                delay,
                screen_text(kind),
                if pick.is_empty() {
                    "-".into()
                } else {
                    screen_text(pick)
                },
                screen_text(name)
            ));
        }
    }
    Some(ScreenState {
        records: out,
        all: s.mode == "all",
        macs: s.macs,
    })
}
/// Screen job result: 0 done, 1 the node list changed (refresh), 2 failed.
pub fn screen_select(version: u64, index: usize) -> u8 {
    let Some(g) = main_group(true) else { return 2 };
    let Some(option) = g.options.get(index).filter(|_| g.version == version) else {
        return 1;
    };
    let body = json!({"group":g.name,"name":option.0});
    let (code, _) = select(&serde_json::to_vec(&body).unwrap());
    *MAIN_GROUP.lock().unwrap() = None;
    if code == 200 {
        0
    } else {
        2
    }
}
pub fn screen_delay(version: u64) -> u8 {
    let Some(g) = main_group(true).filter(|g| g.version == version) else {
        return 1;
    };
    let (code, _) = delay(&serde_json::to_vec(&json!({"group":g.name})).unwrap());
    *MAIN_GROUP.lock().unwrap() = None;
    if code == 200 {
        0
    } else {
        2
    }
}
pub fn logs() -> (u16, Value) {
    (
        200,
        json!({"ok":true,"data":{"lines":LOG.lock().unwrap().iter().collect::<Vec<_>>()}}),
    )
}

pub fn start() {
    kill_stale();
    std::thread::spawn(|| {
        let mut first = true;
        loop {
            {
                let _guard = LOCK.lock().unwrap();
                if let Ok(s) = load() {
                    let alive = running_pid().is_some();
                    let running = alive && ready();
                    let action = if !s.enabled {
                        (first && SPEC.is_owned())
                            .then(|| install(&s).and_then(|_| SPEC.script(&SPEC.release())))
                    } else if first
                        || !SPEC.is_owned()
                        || (running
                            && !SPEC.healthy()
                            && !(attach_route().is_ok() && SPEC.healthy()))
                    {
                        // Policy first under the transition guard; the proxy starts after release
                        // so a start failure blocks only the selected devices.
                        Some(
                            SPEC.claim_resources()
                                .and_then(|_| SPEC.script(&SPEC.hold()))
                                .and_then(|_| install(&s))
                                .and_then(|_| SPEC.script(&SPEC.release()))
                                .and_then(|_| start_child()),
                        )
                    } else if !alive {
                        // Policy stays in place: selected devices are blocked until restart.
                        Some(start_child())
                    } else if !running && startup_age() > STARTUP_LIMIT {
                        let stalled = "sing-box did not become ready within 5 minutes; restarted";
                        Some(start_child().and(Err(stalled.to_string())))
                    } else {
                        None
                    };
                    if let Some(result) = action {
                        *ERROR.lock().unwrap() = result.err();
                    } else if running {
                        // A running proxy supersedes earlier start or apply errors.
                        *ERROR.lock().unwrap() = None;
                    }
                }
                first = false;
            }
            std::thread::sleep(Duration::from_secs(10));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Value {
        json!({
            "log":{"level":"debug","output":"/tmp/box.log"},
            "inbounds":[{"type":"tun","tag":"tun-mine","auto_route":true,"address":["172.18.0.1/30"]},{"type":"mixed","listen":"0.0.0.0","listen_port":7890}],
            "outbounds":[
                {"type":"selector","tag":"Proxy","outbounds":["HK 01","JP 01","auto"]},
                {"type":"urltest","tag":"auto","outbounds":["HK 01","JP 01"]},
                {"type":"vless","tag":"HK 01","server":"hk.example.com","server_port":443,"uuid":"8c0b4d2e-secret-uuid"},
                {"type":"shadowsocks","tag":"JP 01","server":"jp.example.com","server_port":8388,"method":"aes-128-gcm","password":"hunter2-secret"},
                {"type":"direct","tag":"direct"}
            ],
            "route":{"rules":[{"rule_set":"geosite-cn","outbound":"direct"}],"final":"Proxy","default_mark":1},
            "experimental":{"clash_api":{"external_controller":"0.0.0.0:9090","external_ui":"/www"},"cache_file":{"enabled":true,"path":"/etc/cache.db"}},
            "services":[{"type":"ssm-api"}]
        })
    }
    fn bytes(v: &Value) -> Vec<u8> {
        serde_json::to_vec(v).unwrap()
    }
    fn update(s: &Settings, v: Value, download: Option<Vec<u8>>) -> Result<Settings, String> {
        let mut v = v;
        v["expected_revision"] = json!(s.revision);
        edited(s, serde_json::from_value(v).unwrap(), download).map(|(s, _)| s)
    }
    fn saved(s: &Settings, name: &str, config: &Value) -> Settings {
        update(
            s,
            json!({"profile":{"action":"save","name":name,"config_text":config.to_string()}}),
            None,
        )
        .unwrap()
    }
    #[test]
    fn runtime_replaces_inbounds_and_control_surfaces() {
        let (c, nodes) = runtime(&sample(), "profile-1", "s3cret").unwrap();
        assert_eq!(nodes, 2);
        let inbounds = c["inbounds"].as_array().unwrap();
        assert_eq!(inbounds.len(), 2);
        assert_eq!(inbounds[0]["type"], "redirect");
        assert_eq!(inbounds[0]["tag"], "redirect-in");
        assert_eq!(inbounds[0]["listen_port"], REDIRECT_PORT);
        assert_eq!(inbounds[1]["type"], "tun");
        assert_eq!(inbounds[1]["tag"], "tun-mine");
        assert_eq!(inbounds[1]["interface_name"], IFACE);
        assert_eq!(inbounds[1]["auto_route"], false);
        assert_eq!(inbounds[1]["address"], json!([TUN_ADDRESS]));
        let rules = c["route"]["rules"].as_array().unwrap();
        assert_eq!(rules[0]["action"], "hijack-dns");
        assert_eq!(rules[0]["port"], 53);
        assert_eq!(rules[0]["inbound"], json!(["tun-mine", "redirect-in"]));
        assert_eq!(rules[1]["action"], "sniff");
        assert_eq!(rules[2]["protocol"], "dns");
        assert_eq!(rules[3]["rule_set"], "geosite-cn");
        assert!(c["route"].get("default_mark").is_none());
        assert_eq!(c["experimental"]["clash_api"]["external_controller"], API);
        assert_eq!(c["experimental"]["clash_api"]["secret"], "s3cret");
        assert!(c["experimental"]["clash_api"].get("external_ui").is_none());
        assert_eq!(c["experimental"]["cache_file"]["path"], "cache.db");
        assert_eq!(c["experimental"]["cache_file"]["cache_id"], "profile-1");
        assert!(c["log"].get("output").is_none());
        assert!(c.get("services").is_none());
        // An existing selector is kept as the user's switchable group.
        assert!(!c["outbounds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["tag"] == "OpenUI"));
    }
    #[test]
    fn runtime_adds_selector_when_missing() {
        let c = json!({"outbounds":[{"type":"trojan","tag":"US","server":"us.example.com","server_port":443,"password":"x"},{"type":"direct","tag":"direct"}]});
        let (r, nodes) = runtime(&c, "p", "s").unwrap();
        assert_eq!(nodes, 1);
        let selector = r["outbounds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["tag"] == "OpenUI")
            .unwrap();
        assert_eq!(selector["outbounds"], json!(["US"]));
        assert_eq!(r["route"]["final"], "OpenUI");
        assert_eq!(r["inbounds"][0]["tag"], "redirect-in");
        assert_eq!(r["inbounds"][1]["tag"], "tun-in");
    }
    #[test]
    fn runtime_rejects_files_programs_and_marks() {
        let base = || json!({"outbounds":[{"type":"vless","tag":"A","server":"a.example.com","server_port":443,"uuid":"u"}]});
        let mut v = base();
        v["outbounds"][0]["tls"] = json!({"enabled":true,"certificate_path":"/etc/shadow"});
        assert!(runtime(&v, "p", "s")
            .unwrap_err()
            .contains("certificate_path"));
        let mut v = base();
        v["outbounds"][0]["routing_mark"] = json!(536870912);
        assert!(runtime(&v, "p", "s").is_err());
        let mut v = base();
        v["outbounds"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"tor","tag":"T"}));
        assert!(runtime(&v, "p", "s").is_err());
        let mut v = base();
        v["route"] = json!({"rule_set":[{"type":"local","tag":"x","format":"binary"}]});
        assert!(runtime(&v, "p", "s").is_err());
        let mut v = base();
        v["endpoints"] = json!([{"type":"wireguard","tag":"wg","system":true}]);
        assert!(runtime(&v, "p", "s").is_err());
        assert!(runtime(
            &json!({"outbounds":[{"type":"direct","tag":"d"}]}),
            "p",
            "s"
        )
        .is_err());
        assert!(runtime(&json!([1]), "p", "s").is_err());
        assert!(inspect(b"vless://abc@host:443").is_err());
        assert!(inspect(&vec![b' '; MAX_CONFIG + 1]).is_err());
        let mut v = base();
        v["outbounds"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"vless","tag":"A"}));
        assert!(runtime(&v, "p", "s").unwrap_err().contains("Duplicate"));
    }
    #[test]
    fn profiles_limit_switching_and_deletion() {
        let mut s = Settings::default();
        for i in 1..=5 {
            s = saved(&s, &format!("Box {i}"), &sample());
        }
        assert_eq!(s.active_profile, Some(1));
        assert!(!s.enabled);
        let err = update(
            &s,
            json!({"profile":{"action":"save","name":"six","config_text":sample().to_string()}}),
            None,
        )
        .err()
        .unwrap();
        assert!(err.contains("maximum of 5"));
        assert!(update(
            &s,
            json!({"profile":{"action":"rename","id":2,"name":" box 1 "}}),
            None
        )
        .is_err());
        let mut on = update(
            &s,
            json!({"enabled":true,"mode":"selected","macs":["02:11:22:33:44:55"]}),
            None,
        )
        .unwrap();
        assert!(!same_network(&s, &on));
        let renamed = update(
            &on,
            json!({"profile":{"action":"rename","id":3,"name":"Office"}}),
            None,
        )
        .unwrap();
        assert!(same_network(&on, &renamed));
        assert!(update(&on, json!({"profile":{"action":"delete","id":1}}), None).is_err());
        let switched = update(&on, json!({"profile":{"action":"activate","id":2}}), None).unwrap();
        assert!(switched.enabled && switched.mode == "selected" && switched.macs == on.macs);
        on.enabled = false;
        let deleted = update(&on, json!({"profile":{"action":"delete","id":1}}), None).unwrap();
        assert_eq!(deleted.active_profile, None);
        assert!(update(&deleted, json!({"enabled":true}), None).is_err());
    }
    #[test]
    fn subscriptions_refresh_and_stale_requests() {
        let s = update(
            &Settings::default(),
            json!({"profile":{"action":"subscribe","name":"Sub","url":"https://user:pw@sub.example.com/link?token=abc"}}),
            Some(bytes(&sample())),
        )
        .unwrap();
        assert_eq!(s.profiles[0].nodes, 2);
        let mut changed = sample();
        changed["outbounds"][3]["tag"] = json!("JP 02");
        changed["outbounds"][0]["outbounds"] = json!(["HK 01", "JP 02"]);
        changed["outbounds"][1]["outbounds"] = json!(["HK 01", "JP 02"]);
        let refreshed = update(
            &s,
            json!({"profile":{"action":"refresh","id":1}}),
            Some(bytes(&changed)),
        )
        .unwrap();
        assert_ne!(refreshed.profiles[0].digest, s.profiles[0].digest);
        assert!(!same_network(&s, &refreshed));
        let file = saved(&s, "File", &sample());
        assert!(update(
            &file,
            json!({"profile":{"action":"refresh","id":2}}),
            Some(bytes(&sample()))
        )
        .is_err());
        assert!(update(
            &s,
            json!({"profile":{"action":"subscribe","name":"Bad","url":"file:///etc/passwd"}}),
            Some(bytes(&sample()))
        )
        .is_err());
        assert!(update(
            &s,
            json!({"profile":{"action":"subscribe","name":"Bad","url":"https://a b"}}),
            Some(bytes(&sample()))
        )
        .is_err());
        let stale = edited(
            &s,
            serde_json::from_value(json!({"expected_revision":0,"enabled":false})).unwrap(),
            None,
        );
        assert!(stale.err().unwrap().starts_with("Configuration changed;"));
        assert!(update(
            &s,
            json!({"enabled":true,"profile":{"action":"activate","id":1}}),
            None
        )
        .is_err());
    }
    #[test]
    fn status_never_returns_configuration_or_subscription_secrets() {
        let s = update(
            &Settings::default(),
            json!({"profile":{"action":"subscribe","name":"Sub","url":"https://user:pw@sub.example.com/link?token=abc"}}),
            Some(bytes(&sample())),
        )
        .unwrap();
        let s = saved(&s, "File", &sample());
        let public = response(&s).to_string();
        for secret in [
            "token=abc",
            "user:pw",
            "/link",
            "hunter2-secret",
            "8c0b4d2e",
            "hk.example.com",
        ] {
            assert!(!public.contains(secret), "{secret} leaked");
        }
        assert!(public.contains("sub.example.com"));
        assert_eq!(
            url_host("https://a@b.example.com:8443/x?y#z"),
            "b.example.com:8443"
        );
    }
    #[test]
    fn proxy_policy_redirects_tcp_and_routes_the_rest_into_the_tun() {
        let p = SPEC.proxy_policy("all", &[], DNS);
        assert!(p.contains("-p tcp -j REDIRECT --to-ports 9096"));
        assert!(p.contains("-p udp --dport 53 -j DNAT --to-destination 172.19.0.2"));
        assert!(p.contains("unreachable default metric 32760 table 10430"));
        assert!(!p.contains("default dev ou-sb0") && !p.contains("TPROXY"));
        assert!(SPEC
            .route_script()
            .contains("default dev ou-sb0 metric 10 table 10430"));
    }
    #[test]
    fn export_policies() {
        // For an isolated network-namespace check of the generated rules.
        if let Ok(dir) = std::env::var("OPENUI_SB_TEST_POLICIES") {
            let p = Path::new(&dir);
            fs::create_dir_all(p).unwrap();
            let macs = vec!["02:11:22:33:44:55".to_string()];
            fs::write(p.join("all.sh"), SPEC.proxy_policy("all", &macs, DNS)).unwrap();
            fs::write(
                p.join("selected.sh"),
                SPEC.proxy_policy("selected", &macs, DNS),
            )
            .unwrap();
            fs::write(p.join("route.sh"), SPEC.route_script()).unwrap();
            fs::write(p.join("cleanup.sh"), SPEC.cleanup()).unwrap();
            fs::write(p.join("hold.sh"), SPEC.hold()).unwrap();
            fs::write(p.join("release.sh"), SPEC.release()).unwrap();
            fs::write(p.join("health.sh"), SPEC.health_script()).unwrap();
        }
    }
    #[test]
    fn export_runtime_configs() {
        // Validate with a real binary: `sing-box check -c <dir>/<name>.json`.
        if let Ok(dir) = std::env::var("OPENUI_SB_TEST_CONFIG") {
            let p = Path::new(&dir);
            fs::create_dir_all(p).unwrap();
            let bare = json!({"outbounds":[{"type":"trojan","tag":"US","server":"us.example.com","server_port":443,"password":"x","tls":{"enabled":true,"server_name":"us.example.com"}}]});
            // End-to-end namespace check: one SOCKS hop and a predefined DNS answer.
            let e2e = json!({"dns":{"servers":[{"type":"hosts","tag":"hosts","predefined":{"test.openui":"10.99.0.2"}}]},
                "outbounds":[{"type":"socks","tag":"wan-socks","server":"10.99.0.2","server_port":1080}]});
            for (name, config) in [("selector", sample()), ("bare", bare), ("e2e", e2e)] {
                let (c, _) = runtime(&config, "profile-1", "secret").unwrap();
                fs::write(p.join(format!("{name}.json")), c.to_string()).unwrap();
            }
        }
    }
    #[test]
    fn console_colors_are_removed() {
        assert_eq!(
            strip_ansi("\u{1b}[31mFATAL\u{1b}[0m[0000] decode config"),
            "FATAL[0000] decode config"
        );
    }
    #[test]
    fn path_escaping_for_node_names() {
        assert_eq!(escape("🇭🇰 HK/01"), "%F0%9F%87%AD%F0%9F%87%B0%20HK%2F01");
        assert_eq!(escape("a-b_c.d~"), "a-b_c.d~");
    }
}
