//! Non-exclusive evdev observer. Four short presses request the existing supervisor.
use crate::{screen, screen_settings};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::unix::fs::OpenOptionsExt,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};
static AVAILABLE: AtomicBool = AtomicBool::new(false);
static RESULT: Mutex<&str> = Mutex::new("Waiting for power key");
#[derive(Default)]
struct Gesture {
    down: Option<i64>,
    released: Option<i64>,
    first: i64,
    count: u8,
    dropped: bool,
}
impl Gesture {
    fn event(&mut self, kind: u16, code: u16, value: i32, ms: i64) -> bool {
        if kind == 0 && code == 3 {
            *self = Self {
                dropped: true,
                ..Self::default()
            };
            return false;
        }
        if self.dropped {
            if kind == 0 && code == 0 {
                self.dropped = false;
            }
            return false;
        }
        if kind != 1 || code != 116 {
            return false;
        }
        if value == 1 && self.down.is_none() {
            self.down = Some(ms);
        }
        if value != 0 {
            return false;
        }
        let Some(start) = self.down.take() else {
            self.count = 0;
            return false;
        };
        let duration = ms - start;
        let gap = self.released.map(|v| start - v);
        self.released = Some(ms);
        if !(30..800).contains(&duration) || gap.is_some_and(|g| g < 80) {
            self.count = 0;
            return false;
        }
        if self.count == 0 || gap.is_some_and(|g| g > 700) || ms - self.first > 2500 {
            self.first = start;
            self.count = 1;
        } else {
            self.count += 1;
        }
        if self.count == 4 {
            self.count = 0;
            return true;
        }
        false
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
struct InputEvent {
    time: libc::timeval,
    kind: u16,
    code: u16,
    value: i32,
}
fn open_key() -> Option<File> {
    (0..32).find_map(|i| {
        let name = fs::read_to_string(format!("/sys/class/input/event{i}/device/name")).ok()?;
        if name.trim() != "pmic_pwrkey" {
            return None;
        }
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(format!("/dev/input/event{i}"))
            .ok()
    })
}
pub fn status() -> Value {
    json!({"available":AVAILABLE.load(Ordering::Relaxed),"message":*RESULT.lock().unwrap()})
}
pub fn start() {
    std::thread::spawn(|| {
        let mut key = None;
        let mut gesture = Gesture::default();
        let mut revision = 0;
        let mut pending: Option<Instant> = None;
        let mut cooldown = Instant::now();
        loop {
            if key.is_none() {
                key = open_key();
                AVAILABLE.store(key.is_some(), Ordering::Relaxed);
                gesture = Gesture::default();
                pending = None;
                if key.is_none() {
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
            }
            let settings = screen_settings::current();
            let armed = settings.shortcut_enabled && !screen::running();
            let accept = armed && revision == settings.revision;
            if revision != settings.revision || !armed {
                revision = settings.revision;
                gesture = Gesture::default();
                pending = None;
            }
            let mut failed = false;
            // Always drain the descriptor, including while disabled or DevUI is active.
            // Enabling the shortcut must never replay old power-key events.
            for _ in 0..128 {
                let mut bytes = [0u8; std::mem::size_of::<InputEvent>()];
                match key.as_mut().unwrap().read(&mut bytes) {
                    Ok(n) if n == bytes.len() => {
                        let event = unsafe {
                            std::ptr::read_unaligned(bytes.as_ptr().cast::<InputEvent>())
                        };
                        let ms = event.time.tv_sec * 1000 + i64::from(event.time.tv_usec) / 1000;
                        if accept
                            && Instant::now() >= cooldown
                            && gesture.event(event.kind, event.code, event.value, ms)
                        {
                            // Let the factory's last short-press callback settle before takeover.
                            pending = Some(Instant::now() + Duration::from_millis(350));
                            cooldown = Instant::now() + Duration::from_secs(5);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    _ => {
                        failed = true;
                        break;
                    }
                }
            }
            if failed {
                key = None;
                AVAILABLE.store(false, Ordering::Relaxed);
                *RESULT.lock().unwrap() = "Power key disconnected";
                continue;
            }
            if pending.is_some_and(|at| Instant::now() >= at) {
                pending = None;
                if screen_settings::current().shortcut_enabled && !screen::running() {
                    let (code, _) = screen::open();
                    *RESULT.lock().unwrap() = if code == 202 {
                        "Four-press shortcut requested DevUI"
                    } else {
                        "Shortcut blocked: check screen recovery status"
                    };
                }
            } else {
                let mut result = RESULT.lock().unwrap();
                if !settings.shortcut_enabled {
                    *result = "Shortcut disabled";
                } else if *result == "Shortcut disabled" || *result == "Waiting for power key" {
                    *result = "Ready: four short presses within 2.5 seconds";
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    fn press(g: &mut Gesture, t: i64, duration: i64) -> bool {
        assert!(!g.event(1, 116, 1, t));
        assert!(!g.event(1, 116, 2, t + 10));
        g.event(1, 116, 0, t + duration)
    }
    #[test]
    fn exactly_four_and_no_release_replay() {
        let mut g = Gesture::default();
        for i in 0..3 {
            assert!(!press(&mut g, 1000 + i * 350, 100));
        }
        assert!(press(&mut g, 2050, 100));
        assert!(!g.event(1, 116, 0, 2160));
    }
    #[test]
    fn long_slow_bounce_and_dropped_events_reset() {
        for invalid in [0, 1, 2, 3] {
            let mut g = Gesture::default();
            assert!(!press(&mut g, 1000, 100));
            assert!(!press(&mut g, 1350, 100));
            match invalid {
                0 => {
                    assert!(!press(&mut g, 1700, 900));
                }
                1 => {
                    assert!(!press(&mut g, 3000, 100));
                }
                2 => {
                    assert!(!press(&mut g, 1460, 40));
                }
                _ => {
                    g.event(0, 3, 0, 1500);
                    g.event(0, 0, 0, 1501);
                }
            }
            assert!(!press(&mut g, 3500, 100));
            assert!(!press(&mut g, 3850, 100));
        }
    }
    #[test]
    fn four_slow_presses_are_not_a_shortcut() {
        let mut g = Gesture::default();
        for i in 0..4 {
            assert!(!press(&mut g, 1000 + i * 790, 600));
        }
    }
}
