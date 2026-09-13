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
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub revision: u64,
    pub shortcut_enabled: bool,
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
            brightness: 128,
            idle_seconds: 120,
            home_page: "menu".into(),
            menu_items: ["overview", "wireguard", "profiles", "devices"]
                .map(String::from)
                .to_vec(),
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
        if self.menu_items.is_empty() || self.menu_items.len() > 4 {
            return Err("Choose at least one menu item");
        }
        let mut seen = std::collections::HashSet::new();
        for item in &self.menu_items {
            if !["overview", "wireguard", "profiles", "devices"].contains(&item.as_str())
                || !seen.insert(item)
            {
                return Err("Invalid or duplicate menu item");
            }
        }
        if self.home_page != "menu" && !self.menu_items.contains(&self.home_page) {
            return Err("The startup page must be visible in the menu");
        }
        Ok(())
    }
    pub fn projection(&self) -> String {
        let page = |s: &str| match s {
            "wireguard" => 0,
            "devices" => 1,
            "profiles" => 3,
            "overview" => 4,
            _ => 2,
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
        json!({"expected_revision":1,"shortcut_enabled":true,"brightness":160,"idle_seconds":90,"home_page":"overview","menu_items":["overview","wireguard"]})
    }
    #[test]
    fn validation_and_conflicts() {
        let old = Settings::default();
        let b = body();
        let next = prepare(&old, &serde_json::to_vec(&b).unwrap()).unwrap();
        assert_eq!(next.revision, 2);
        assert!(next.shortcut_enabled);
        assert_eq!(next.projection(), "C 2 90 160 4 17\n");
        for (key, value) in [
            ("brightness", json!(0)),
            ("brightness", json!(256)),
            ("idle_seconds", json!(29)),
            ("idle_seconds", json!(601)),
            ("menu_items", json!([])),
            ("menu_items", json!(["overview", "overview"])),
            ("home_page", json!("devices")),
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
    }
}
