//! Authenticated web lifecycle + root-only local screen protocol. No credentials or keys.
use crate::{
    handlers::{self, AppState},
    network_ext, singbox, system, wireguard,
};
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::Path,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
const DIR: &str = "/tmp/openui-screen";
const BIN: &str = "/data/bin/openui-screen";
const VERIFIED: &str = "/data/local/tmp/openui-screen-verified";
static SUPERVISOR: AtomicI32 = AtomicI32::new(0);
static STARTING: AtomicBool = AtomicBool::new(false);
static LIFECYCLE: Mutex<()> = Mutex::new(());
static SNAPSHOT: Mutex<String> = Mutex::new(String::new());
static INTEREST: AtomicU64 = AtomicU64::new(0);
static BUSY: AtomicBool = AtomicBool::new(false);
static JOB: AtomicU64 = AtomicU64::new(0);
static RESULT: Mutex<(u64, u8)> = Mutex::new((0, 0));
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn owner() -> Option<i32> {
    let pid = fs::read_to_string(format!("{DIR}/owner"))
        .ok()?
        .trim()
        .parse::<i32>()
        .ok()?;
    if pid <= 1 {
        return None;
    }
    let exe = fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    (exe == Path::new(BIN)).then_some(pid)
}
pub fn running() -> bool {
    STARTING.load(Ordering::Acquire) || owner().is_some()
}
pub fn get() -> (u16, Value) {
    let running = running();
    let state = fs::read_to_string(format!("{DIR}/state")).unwrap_or_else(|_| "idle".into());
    (
        200,
        json!({"ok":true,"data":{"available":Path::new(BIN).is_file(),"verified":Path::new(VERIFIED).is_file(),"running":running,"shortcut":crate::screen_shortcut::status(),"settings_revision":crate::screen_settings::current().revision,"state":if STARTING.load(Ordering::Acquire) && owner().is_none() { "starting" } else if running || state.trim()=="error" {state.trim()} else {"idle"}}}),
    )
}
pub fn open() -> (u16, Value) {
    let _guard = LIFECYCLE.lock().unwrap();
    if running() {
        return get();
    }
    if !Path::new(BIN).is_file() || !Path::new(VERIFIED).is_file() {
        return (
            503,
            json!({"ok":false,"error":"Screen component has not passed device recovery verification"}),
        );
    }
    // A separate supervisor survives an agent restart. It owns the DRM child and restores stock UI.
    STARTING.store(true, Ordering::Release);
    match Command::new(BIN)
        .arg("--supervise")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            SUPERVISOR.store(child.id() as i32, Ordering::Release);
            std::thread::spawn(move || {
                let _ = child.wait();
                SUPERVISOR.store(0, Ordering::Release);
                STARTING.store(false, Ordering::Release);
            });
            (202, json!({"ok":true,"data":{"state":"starting"}}))
        }
        Err(_) => {
            STARTING.store(false, Ordering::Release);
            (
                500,
                json!({"ok":false,"error":"Cannot start screen supervisor"}),
            )
        }
    }
}
pub fn close() -> (u16, Value) {
    let _guard = LIFECYCLE.lock().unwrap();
    if let Some(pid) = owner().or_else(|| {
        let pid = SUPERVISOR.load(Ordering::Acquire);
        (pid > 1).then_some(pid)
    }) {
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    } else if Path::new(BIN).is_file() && Path::new(&format!("{DIR}/ui")).is_file() {
        match Command::new(BIN)
            .arg("--restore")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(_) => {
                return (
                    500,
                    json!({"ok":false,"error":"Cannot start screen recovery"}),
                )
            }
        }
    }
    (202, json!({"ok":true,"data":{"state":"restoring"}}))
}
fn clean(s: &str) -> String {
    let s: String = s.chars().filter(|c| !c.is_control()).take(48).collect();
    if s.is_empty() {
        "-".into()
    } else {
        s
    }
}
fn projection(w: &Value, sb: Option<&singbox::ScreenState>, clients: &[Value]) -> String {
    let b = |k: &str| u8::from(w[k].as_bool().unwrap_or(false));
    let n = |k: &str| w[k].as_u64().unwrap_or(0);
    let all = w["mode"] == "all";
    let macs: Vec<&str> = w["macs"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut out = format!(
        "S {} {} {} {} {} {} {} {} {} {}\n",
        n("revision"),
        b("has_config"),
        b("enabled"),
        b("connected"),
        u8::from(w["error"].is_string()),
        n("latest_handshake"),
        n("rx_bytes"),
        n("tx_bytes"),
        u8::from(all),
        now()
    );
    // Only profile IDs, names and selection leave the agent. No endpoints or keys.
    if let Some(profiles) = w["profiles"].as_array() {
        for p in profiles.iter().take(5) {
            if let Some(id) = p["id"].as_u64() {
                out.push_str(&format!(
                    "P\t{}\t{}\t{}\n",
                    id,
                    u8::from(w["active_profile"].as_u64() == Some(id)),
                    clean(p["name"].as_str().unwrap_or("Profile"))
                ));
            }
        }
    }
    let mut rows = std::collections::BTreeMap::<String, Value>::new();
    for c in clients {
        if let Some(m) = c["mac"].as_str() {
            rows.insert(m.to_lowercase(), c.clone());
        }
    }
    let sb_macs: Vec<&str> = sb
        .map(|s| s.macs.iter().map(String::as_str).collect())
        .unwrap_or_default();
    for m in macs.iter().chain(&sb_macs) {
        rows.entry(m.to_string())
            .or_insert_with(|| json!({"mac":m}));
    }
    // "all" mode records exclusions; "selected" mode records the chosen devices.
    let chosen = |all: bool, list: &[&str], m: &str| all != list.contains(&m);
    for (m, c) in rows.into_iter().take(128) {
        let online = c["medium"].is_string(); // Same discovery evidence as the dashboard, not a reachability probe.
        out.push_str(&format!(
            "D\t{}\t{}\t{}\t{}\t{}\t{}\n",
            clean(&m),
            u8::from(chosen(all, &macs, &m)),
            u8::from(online),
            clean(c["ip"].as_str().unwrap_or("-")),
            clean(c["hostname"].as_str().unwrap_or("-")),
            u8::from(sb.is_some_and(|s| chosen(s.all, &sb_macs, &m)))
        ));
    }
    if let Some(sb) = sb {
        out.push_str(&sb.records);
    }
    out
}
/// Radio summary from the same `zte_nwinfo_api` data as the dashboard.
fn signal_line(v: &Value) -> String {
    let text = |k: &str| {
        v[k].as_str()
            .map(|s| s.trim().to_string())
            .or_else(|| v[k].as_i64().map(|n| n.to_string()))
            .filter(|s| !s.is_empty())
    };
    let raw = text("network_type").unwrap_or_default().to_uppercase();
    let kind = match raw.as_str() {
        "SA" => "5G SA".to_string(),
        "NSA" | "ENDC" => "5G NSA".to_string(),
        "LTE" | "4G" => "4G".to_string(),
        "WCDMA" | "UMTS" | "HSPA" | "3G" => "3G".to_string(),
        "" | "NO_SERVICE" | "NONE" | "LIMITED_SERVICE" => "-".to_string(),
        other => clean(other),
    };
    let keys = if kind == "5G SA" {
        ["nr5g_rsrp", "lte_rsrp"]
    } else {
        ["lte_rsrp", "nr5g_rsrp"]
    };
    let rsrp = keys.iter().find_map(|k| {
        text(k)?
            .parse::<i32>()
            .ok()
            .filter(|r| (-160..0).contains(r))
    });
    let bars = text("signalbar")
        .and_then(|b| b.parse::<u8>().ok())
        .unwrap_or(0)
        .min(5);
    let operator = text("network_provider_fullname").or_else(|| text("network_provider"));
    format!(
        "G\t{}\t{}\t{}\t{}\n",
        kind,
        if kind == "-" { 0 } else { bars },
        rsrp.map_or("-".into(), |r| r.to_string()),
        operator.as_deref().map_or("-".into(), clean)
    )
}
enum Action {
    WireGuard(Value),
    SingBox(Value),
    Node(u64, usize),
    Delay(u64),
}
fn devices(list: &str) -> Result<Value, &'static str> {
    let mut devices = vec![];
    for choice in list.split(',') {
        let (mac, selected) = choice.split_once('=').ok_or("Invalid device choice")?;
        if selected != "0" && selected != "1" {
            return Err("Invalid selection");
        }
        devices.push(json!({"mac":mac,"selected":selected=="1"}));
    }
    if devices.len() > 128 {
        return Err("Too many devices");
    }
    Ok(json!(devices))
}
fn action(line: &str) -> Result<Action, &'static str> {
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.len() != 3 {
        return Err("Invalid action");
    }
    let revision = words[1].parse::<u64>().map_err(|_| "Invalid revision")?;
    let profile = |word: &str| match word.parse::<u64>() {
        Ok(id) if id > 0 => {
            Ok(json!({"expected_revision":revision,"profile":{"action":"activate","id":id}}))
        }
        _ => Err("Invalid profile ID"),
    };
    match words[0] {
        "toggle" if words[2] == "0" || words[2] == "1" => Ok(Action::WireGuard(
            json!({"expected_revision":revision,"enabled":words[2]=="1"}),
        )),
        "profile" => profile(words[2]).map(Action::WireGuard),
        "devices" => Ok(Action::WireGuard(
            json!({"expected_revision":revision,"devices":devices(words[2])?}),
        )),
        "sb-toggle" if words[2] == "0" || words[2] == "1" => Ok(Action::SingBox(
            json!({"expected_revision":revision,"enabled":words[2]=="1"}),
        )),
        "sb-profile" => profile(words[2]).map(Action::SingBox),
        "sb-devices" => Ok(Action::SingBox(
            json!({"expected_revision":revision,"devices":devices(words[2])?}),
        )),
        // Nodes are addressed by index within a versioned list: names may hold spaces.
        "sb-node" => Ok(Action::Node(
            revision,
            words[2].parse::<usize>().map_err(|_| "Invalid node")?,
        )),
        "sb-delay" if words[2] == "0" => Ok(Action::Delay(revision)),
        _ => Err("Unsupported action"),
    }
}
fn request(line: &str) -> String {
    INTEREST.store(now(), Ordering::Relaxed);
    if line.trim() == "status" {
        let snapshot = SNAPSHOT.lock().unwrap().clone();
        let result = *RESULT.lock().unwrap();
        return format!(
            "{}{}J {} {} {} {}\n.\n",
            snapshot,
            crate::screen_settings::current().projection(),
            u8::from(BUSY.load(Ordering::Acquire)),
            JOB.load(Ordering::Acquire),
            result.0,
            result.1
        );
    }
    let action = match action(line) {
        Ok(v) => v,
        Err(e) => return format!("ERR {e}\n"),
    };
    if BUSY
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return "ERR Busy\n".into();
    }
    let job = JOB.fetch_add(1, Ordering::AcqRel) + 1;
    std::thread::spawn(move || {
        let status = |code: u16| match code {
            200 => 0,
            409 => 1,
            _ => 2,
        };
        let result = match action {
            Action::WireGuard(v) => status(wireguard::update(&serde_json::to_vec(&v).unwrap()).0),
            Action::SingBox(v) => status(singbox::update(&serde_json::to_vec(&v).unwrap()).0),
            Action::Node(version, index) => singbox::screen_select(version, index),
            Action::Delay(version) => singbox::screen_delay(version),
        };
        *RESULT.lock().unwrap() = (job, result);
        BUSY.store(false, Ordering::Release);
    });
    format!("OK {job}\n")
}
pub fn start(state: Arc<AppState>) {
    if fs::create_dir_all(DIR).is_err()
        || fs::set_permissions(DIR, fs::Permissions::from_mode(0o700)).is_err()
    {
        return;
    }
    // Only one agent instance should own the listener; reject a live existing endpoint.
    let path = format!("{DIR}/control.sock");
    if std::os::unix::net::UnixStream::connect(&path).is_ok() {
        return;
    }
    let _ = fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(v) => v,
        Err(_) => return,
    };
    if fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).is_err() {
        return;
    }
    crate::screen_shortcut::start();
    std::thread::spawn(move || {
        let mut clients = vec![];
        let mut refreshed = 0;
        loop {
            if now().saturating_sub(INTEREST.load(Ordering::Relaxed)) < 10 {
                if now().saturating_sub(refreshed) >= 10 {
                    let (_, v) = network_ext::network_clients(&state);
                    clients = v["data"]["clients"].as_array().cloned().unwrap_or_default();
                    refreshed = now();
                }
                let (code, v) = wireguard::get();
                if code == 200 {
                    let sb = singbox::screen_state();
                    let mut snapshot = projection(&v["data"], sb.as_ref(), &clients);
                    let battery = system::read_battery();
                    let device = system::read_device_info();
                    let memory = system::read_meminfo();
                    let cpu = state.cpu.sample();
                    snapshot.push_str(&format!(
                        "O {} {} {} {:.1} {:.1} {}\n",
                        battery.as_ref().map_or(-1, |b| b.capacity),
                        battery.as_ref().map_or(-999, |b| b.temperature),
                        device.uptime_secs,
                        memory.map_or(-1.0, |m| m.usage_pct),
                        cpu.overall,
                        u8::from(battery.as_ref().is_some_and(|b| b.status == "Charging"))
                    ));
                    snapshot.push_str(&signal_line(
                        &handlers::read_radio(&state.radio)
                            .value
                            .unwrap_or(Value::Null),
                    ));
                    *SNAPSHOT.lock().unwrap() = snapshot;
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
            let mut line = String::new();
            if BufReader::new((&stream).take(4097))
                .read_line(&mut line)
                .is_ok()
                && line.len() <= 4096
                && line.ends_with('\n')
            {
                let _ = stream.write_all(request(&line).as_bytes());
            }
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn action_is_narrow_and_versioned() {
        let wg = |line: &str| match action(line) {
            Ok(Action::WireGuard(v)) => v,
            _ => panic!("{line}"),
        };
        let sb = |line: &str| match action(line) {
            Ok(Action::SingBox(v)) => v,
            _ => panic!("{line}"),
        };
        assert_eq!(
            wg("toggle 8 1"),
            json!({"expected_revision":8,"enabled":true})
        );
        assert_eq!(
            wg("profile 8 2"),
            json!({"expected_revision":8,"profile":{"action":"activate","id":2}})
        );
        assert_eq!(
            sb("sb-toggle 3 0"),
            json!({"expected_revision":3,"enabled":false})
        );
        assert_eq!(
            sb("sb-profile 3 2"),
            json!({"expected_revision":3,"profile":{"action":"activate","id":2}})
        );
        assert_eq!(
            sb("sb-devices 3 02:11:22:33:44:55=1"),
            json!({"expected_revision":3,"devices":[{"mac":"02:11:22:33:44:55","selected":true}]})
        );
        assert!(matches!(action("sb-node 99 2"), Ok(Action::Node(99, 2))));
        assert!(matches!(action("sb-delay 99 0"), Ok(Action::Delay(99))));
        for input in [
            "sb-node 99 x",
            "sb-node 99 -1",
            "sb-delay 99 1",
            "sb-toggle 3 yes",
            "sb-config 3 x",
        ] {
            assert!(action(input).is_err(), "{input}");
        }
        for input in [
            "profile 8 0",
            "profile 8 -1",
            "profile 8 x",
            "profile 8 2 extra",
        ] {
            assert!(action(input).is_err());
        }
        assert!(action("toggle 8 true").is_err());
        assert!(action("config 8 secret").is_err());
        assert!(action("devices 8 aa=2").is_err());
    }
    #[test]
    fn profile_projection_omits_credentials_and_escapes_protocol() {
        let v = json!({"active_profile":2,"profiles":[
            {"id":1,"name":"Home\nVPN","endpoint":"private.example","private_key":"secret"},
            {"id":2,"name":"Travel\tVPN","preshared_key":"secret"}]});
        let s = projection(&v, None, &[]);
        assert!(s.contains("P\t1\t0\tHomeVPN\n"));
        assert!(s.contains("P\t2\t1\tTravelVPN\n"));
        assert!(!s.contains("secret") && !s.contains("private.example"));
    }
    #[test]
    fn snapshot_redacts_and_keeps_offline_choices() {
        let v = json!({"revision":3,"mode":"all","macs":["02:11:22:33:44:55"],"private_key":"secret","enabled":false});
        let s = projection(&v, None, &[]);
        assert!(!s.contains("secret"));
        assert!(s.contains("D\t02:11:22:33:44:55\t0\t0\t-\t-\t0\n"));
        assert_eq!(clean("a\nb\t\u{1b}"), "ab");
    }
    #[test]
    fn device_rows_merge_both_gateway_selections() {
        let wg = json!({"revision":3,"mode":"selected","macs":["02:11:22:33:44:55"]});
        let sb = singbox::ScreenState {
            records: "B 1\n".into(),
            all: true,
            macs: vec!["02:11:22:33:44:66".into()],
        };
        let clients = [
            json!({"mac":"02:11:22:33:44:77","ip":"192.0.2.7","hostname":"phone","medium":"wifi"}),
        ];
        let s = projection(&wg, Some(&sb), &clients);
        assert!(s.contains("D\t02:11:22:33:44:55\t1\t0\t-\t-\t1\n"));
        // sing-box "all" mode: the recorded MAC is an exclusion.
        assert!(s.contains("D\t02:11:22:33:44:66\t0\t0\t-\t-\t0\n"));
        assert!(s.contains("D\t02:11:22:33:44:77\t0\t1\t192.0.2.7\tphone\t1\n"));
        assert!(s.ends_with("B 1\n"));
    }
    #[test]
    fn signal_summary_matches_dashboard_fields() {
        let sa = json!({"network_type":"SA","signalbar":"4","nr5g_rsrp":"-100","lte_rsrp":"","network_provider_fullname":"China Unicom"});
        assert_eq!(signal_line(&sa), "G\t5G SA\t4\t-100\tChina Unicom\n");
        let nsa = json!({"network_type":"ENDC","signalbar":9,"lte_rsrp":-88,"nr5g_rsrp":"-95","network_provider":"Op\tX"});
        assert_eq!(signal_line(&nsa), "G\t5G NSA\t5\t-88\tOpX\n");
        assert_eq!(
            signal_line(&json!({"network_type":"NO_SERVICE","signalbar":"3"})),
            "G\t-\t0\t-\t-\n"
        );
        assert_eq!(signal_line(&Value::Null), "G\t-\t0\t-\t-\n");
    }
}
