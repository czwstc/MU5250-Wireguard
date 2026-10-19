//! Persisted DevUI preferences. No executable content or WireGuard configuration.
use crate::storage;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{Mutex, OnceLock},
};
const DIR: &str = "/data/local/tmp/openui-devui";
const FILE: &str = "/data/local/tmp/openui-devui/settings.json";
/// Gateway entries shown on the Overview page.
const GATEWAYS: [&str; 2] = ["singbox", "wireguard"];
const HOME_PAGES: [&str; 3] = ["overview", "singbox", "wireguard"];
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub revision: u64,
    pub shortcut_enabled: bool,
    /// Long-press the stock UI's top-left corner to open DevUI.
    #[serde(default)]
    pub corner_shortcut_enabled: bool,
    pub brightness: u16,
    pub idle_seconds: u16,
    pub home_page: String,
    pub menu_items: Vec<String>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            revision: 1,
            shortcut_enabled: false,
            corner_shortcut_enabled: false,
            brightness: 128,
            idle_seconds: 120,
            home_page: "overview".into(),
            menu_items: GATEWAYS.map(String::from).to_vec(),
        }
    }
}
impl Settings {
    fn validate(&self) -> Result<(), &'static str> {
        if !(20..=255).contains(&self.brightness) {
            return Err("Brightness must be between 20 and 255");
        }
        if !(30..=600).contains(&self.idle_seconds) {
            return Err("Idle return must be between 30 and 600 seconds");
        }
        let mut seen = std::collections::HashSet::new();
        for item in &self.menu_items {
            if !GATEWAYS.contains(&item.as_str()) || !seen.insert(item) {
                return Err("Invalid or duplicate Overview entry");
            }
        }
        if !HOME_PAGES.contains(&self.home_page.as_str()) {
            return Err("Invalid startup page");
        }
        if self.home_page != "overview" && !self.menu_items.contains(&self.home_page) {
            return Err("The startup page must be shown on the Overview");
        }
        Ok(())
    }
    /// Settings saved by the earlier menu-based DevUI: the main menu and its
    /// WireGuard sub-pages become the Overview hub. The shortcut, brightness and
    /// idle settings are kept as they were.
    fn migrated(mut self) -> Self {
        let old = std::mem::take(&mut self.menu_items);
        if !old.iter().all(|i| GATEWAYS.contains(&i.as_str())) {
            self.menu_items.push("singbox".into());
            if old
                .iter()
                .any(|i| ["wireguard", "profiles", "devices"].contains(&i.as_str()))
            {
                self.menu_items.push("wireguard".into());
            }
        } else {
            self.menu_items = old;
        }
        if !HOME_PAGES.contains(&self.home_page.as_str())
            || (self.home_page != "overview" && !self.menu_items.contains(&self.home_page))
        {
            self.home_page = "overview".into();
        }
        self
    }
    /// Screen page IDs: Overview 0, sing-box 1, WireGuard 2; the mask holds the
    /// gateway entries shown on the Overview.
    pub fn projection(&self) -> String {
        let page = |s: &str| match s {
            "singbox" => 1,
            "wireguard" => 2,
            _ => 0,
        };
        let mask = self.menu_items.iter().fold(0u32, |m, s| m | (1 << page(s)));
        format!(
            "C {} {} {} {} {}\n",
            self.revision,
            self.idle_seconds,
            self.brightness,
            page(&self.home_page),
            mask
        )
    }
}
fn state() -> &'static Mutex<Settings> {
    static STATE: OnceLock<Mutex<Settings>> = OnceLock::new();
    STATE.get_or_init(|| {
        let saved = fs::read(FILE)
            .ok()
            .and_then(|b| serde_json::from_slice::<Settings>(&b).ok())
            .map(Settings::migrated)
            .filter(|s| s.validate().is_ok());
        Mutex::new(saved.unwrap_or_default())
    })
}
pub fn current() -> Settings {
    state().lock().unwrap().clone()
}
pub fn get() -> (u16, Value) {
    (200, json!({"ok":true,"data":current()}))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    expected_revision: u64,
    shortcut_enabled: bool,
    corner_shortcut_enabled: bool,
    brightness: u16,
    idle_seconds: u16,
    home_page: String,
    menu_items: Vec<String>,
}
fn prepare(old: &Settings, body: &[u8]) -> Result<Settings, (u16, &'static str)> {
    let u: Update = serde_json::from_slice(body).map_err(|_| (400, "Invalid DevUI settings"))?;
    if u.expected_revision != old.revision {
        return Err((409, "DevUI settings changed. Reload before saving."));
    }
    let s = Settings {
        revision: old
            .revision
            .checked_add(1)
            .ok_or((400, "Revision limit reached"))?,
        shortcut_enabled: u.shortcut_enabled,
        corner_shortcut_enabled: u.corner_shortcut_enabled,
        brightness: u.brightness,
        idle_seconds: u.idle_seconds,
        home_page: u.home_page,
        menu_items: u.menu_items,
    };
    s.validate().map_err(|e| (400, e))?;
    Ok(s)
}
pub fn update(body: &[u8]) -> (u16, Value) {
    let mut saved = state().lock().unwrap();
    let next = match prepare(&saved, body) {
        Ok(v) => v,
        Err((code, e)) => return (code, json!({"ok":false,"error":e})),
    };
    let persist = || -> std::io::Result<()> {
        fs::create_dir_all(DIR)?;
        fs::set_permissions(DIR, fs::Permissions::from_mode(0o700))?;
        storage::atomic_write(Path::new(FILE), &serde_json::to_vec_pretty(&next)?)
    };
    if persist().is_err() {
        return (
            500,
            json!({"ok":false,"error":"Cannot save DevUI settings"}),
        );
    }
    *saved = next;
    (200, json!({"ok":true,"data":*saved}))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn body() -> Value {
        json!({"expected_revision":1,"shortcut_enabled":true,"corner_shortcut_enabled":true,"brightness":160,"idle_seconds":90,"home_page":"singbox","menu_items":["singbox"]})
    }
    #[test]
    fn validation_and_conflicts() {
        let old = Settings::default();
        let b = body();
        let next = prepare(&old, &serde_json::to_vec(&b).unwrap()).unwrap();
        assert_eq!(next.revision, 2);
        assert!(next.shortcut_enabled && next.corner_shortcut_enabled);
        assert_eq!(next.projection(), "C 2 90 160 1 2\n");
        for (key, value) in [
            ("brightness", json!(0)),
            ("brightness", json!(256)),
            ("idle_seconds", json!(29)),
            ("idle_seconds", json!(601)),
            ("menu_items", json!(["singbox", "singbox"])),
            ("menu_items", json!(["overview"])),
            ("home_page", json!("wireguard")),
            ("home_page", json!("menu")),
            ("command", json!("reboot")),
        ] {
            let mut bad = b.clone();
            bad[key] = value;
            assert_eq!(
                prepare(&old, &serde_json::to_vec(&bad).unwrap())
                    .unwrap_err()
                    .0,
                400
            );
        }
        assert_eq!(
            prepare(&next, &serde_json::to_vec(&b).unwrap())
                .unwrap_err()
                .0,
            409
        );
        assert!(prepare(&old, b"{}").is_err());
        let mut both_hidden = b.clone();
        both_hidden["menu_items"] = json!([]);
        both_hidden["home_page"] = json!("overview");
        let hidden = prepare(&old, &serde_json::to_vec(&both_hidden).unwrap()).unwrap();
        assert_eq!(hidden.projection(), "C 2 90 160 0 0\n");
        assert_eq!(Settings::default().projection(), "C 1 120 128 0 6\n");
    }
    #[test]
    fn menu_era_settings_migrate_and_keep_the_shortcut() {
        let saved: Settings = serde_json::from_value(json!({"revision":7,"shortcut_enabled":true,"brightness":128,
            "idle_seconds":120,"home_page":"menu","menu_items":["overview","wireguard","profiles","devices"]}))
        .unwrap();
        let s = saved.migrated();
        s.validate().unwrap();
        assert!(s.shortcut_enabled && !s.corner_shortcut_enabled);
        assert_eq!((s.revision, s.home_page.as_str()), (7, "overview"));
        assert_eq!(s.menu_items, ["singbox", "wireguard"]);
        let only_overview: Settings = serde_json::from_value(
            json!({"revision":2,"shortcut_enabled":false,"brightness":64,
            "idle_seconds":60,"home_page":"overview","menu_items":["overview"]}),
        )
        .unwrap();
        let s = only_overview.migrated();
        s.validate().unwrap();
        assert_eq!(s.menu_items, ["singbox"]);
        let current = Settings::default().migrated();
        assert_eq!(current.menu_items, GATEWAYS);
    }
}
