//! Read-only B31 runtime bearer QoS. The firmware's `qci` is never relabelled 5QI.
use crate::process::BoundedCommand;
use crate::util::MutexExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Sample {
    attempted: Instant,
    value: Value,
}
static SAMPLE: Mutex<Option<Sample>> = Mutex::new(None);
fn command(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .bounded_output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!("{program} exited unsuccessfully"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
fn read(state: &crate::handlers::AppState) -> Result<Value, String> {
    let data = command("uci", &["-q", "show", "zwrt_data_tmp"])?;
    let routes = command("ip", &["-4", "route", "show", "default"])?;
    let sample = crate::handlers::read_radio(&state.radio);
    if sample.freshness.stale {
        return Err(sample
            .freshness
            .error
            .unwrap_or_else(|| "radio sample stale".into()));
    }
    let radio = sample.value.ok_or("radio unavailable")?;
    let apns = command("uci", &["-q", "show", "zwrt_wan"]).unwrap_or_default();
    Ok(parse(
        &data,
        &routes,
        radio.get("network_type").and_then(Value::as_str),
        &apns,
    ))
}
fn parse(data: &str, routes: &str, technology: Option<&str>, apns: &str) -> Value {
    let fields: HashMap<_, _> = data
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k, v.trim_matches('\'')))
        .collect();
    let apns: HashMap<_, _> = apns
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k, v.trim_matches('\'')))
        .collect();
    let default_interfaces: Vec<_> = routes
        .lines()
        .filter_map(|line| {
            let parts: Vec<_> = line.split_whitespace().collect();
            parts
                .iter()
                .position(|p| *p == "dev")
                .and_then(|i| parts.get(i + 1).copied())
        })
        .collect();
    let mut bearers = Vec::new();
    let mut supported = false;
    for cid in 1..=8 {
        let prefix = format!("zwrt_data_tmp.wwaniface{cid}.");
        let field = |name: &str| fields.get(format!("{prefix}{name}").as_str()).copied();
        let qci = field("qci")
            .and_then(|s| s.parse::<u16>().ok())
            .filter(|v| *v > 0 && *v <= 255);
        let five_qi = field("5qi")
            .and_then(|s| s.parse::<u16>().ok())
            .filter(|v| *v > 0 && *v <= 255);
        supported |= field("qci").is_some() || field("5qi").is_some();
        let state = field("connect_status").unwrap_or("unknown");
        let connected = matches!(
            state,
            "connected" | "ipv4_connected" | "ipv6_connected" | "ipv4_ipv6_connected"
        );
        if !connected {
            continue;
        }
        let interface = field("ipv4_dev_name")
            .filter(|s| !s.is_empty())
            .or_else(|| field("ipv6_dev_name"));
        let default = interface.map(|v| default_interfaces.contains(&v));
        let apn_key = format!("zwrt_wan.wwaniface{cid}.apn");
        bearers.push(
            json!({"id":format!("wwaniface{cid}"),"cid":cid,"apn":apns.get(apn_key.as_str()),
            "technology":technology,"interface":interface,"connected":true,"default_data":default,
            "qci":qci,"five_qi":five_qi}),
        );
    }
    let status = if !supported {
        "unsupported"
    } else if bearers
        .iter()
        .any(|b| !b["qci"].is_null() || !b["five_qi"].is_null())
    {
        "available"
    } else {
        "no_data"
    };
    json!({"status":status,"sampled_at":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
        "source":"uci:zwrt_data_tmp.wwaniface*.qci/5qi","stale":false,"error":null,"bearers":bearers})
}
pub fn get(state: &crate::handlers::AppState) -> (u16, Value) {
    let mut slot = SAMPLE.safe_lock();
    if slot
        .as_ref()
        .is_none_or(|s| s.attempted.elapsed() >= Duration::from_secs(3))
    {
        let value = match read(state) {
            Ok(value) => value,
            Err(error) => {
                let mut value = slot.as_ref().map(|s| s.value.clone()).unwrap_or_else(|| json!({"status":"error","sampled_at":null,"source":"uci:zwrt_data_tmp","bearers":[]}));
                value["stale"] = json!(true);
                value["error"] = json!(error);
                value
            }
        };
        *slot = Some(Sample {
            attempted: Instant::now(),
            value,
        });
    }
    (200, json!({"ok":true,"data":slot.as_ref().unwrap().value}))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reports_real_qci_without_inventing_5qi_or_default() {
        let sample = parse("zwrt_data_tmp.wwaniface1.connect_status='ipv4_ipv6_connected'\nzwrt_data_tmp.wwaniface1.qci='6'\nzwrt_data_tmp.wwaniface1.ipv4_dev_name='rmnet_data0'\nzwrt_data_tmp.wwaniface2.connect_status='disconnected'\nzwrt_data_tmp.wwaniface2.qci='9'", "default via 10.0.0.1 dev rmnet_data0", Some("SA"), "");
        assert_eq!(sample["bearers"].as_array().unwrap().len(), 1);
        assert_eq!(sample["bearers"][0]["qci"], 6);
        assert!(sample["bearers"][0]["five_qi"].is_null());
        assert_eq!(sample["bearers"][0]["default_data"], true);
    }
    #[test]
    fn unsupported_empty_and_invalid_are_distinct() {
        assert_eq!(parse("", "", None, "")["status"], "unsupported");
        assert_eq!(
            parse("zwrt_data_tmp.wwaniface1.qci='999'", "", None, "")["status"],
            "no_data"
        );
    }
}
