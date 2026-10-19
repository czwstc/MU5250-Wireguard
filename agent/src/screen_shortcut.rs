//! Non-exclusive evdev observers. Four short power presses, or a two-second
//! long-press in the stock UI's top-left corner, request the existing supervisor.
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
static CORNER_AVAILABLE: AtomicBool = AtomicBool::new(false);
static CORNER_RESULT: Mutex<&str> = Mutex::new("Waiting for touchscreen");
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
    json!({"available":AVAILABLE.load(Ordering::Relaxed),"message":*RESULT.lock().unwrap(),
        "corner":{"available":CORNER_AVAILABLE.load(Ordering::Relaxed),"message":*CORNER_RESULT.lock().unwrap()}})
}

// Logical (upright) screen geometry; the panel is mounted rotated by 180°.
const W: i32 = 320;
const H: i32 = 480;
const ZONE: i32 = 64;
const SLOP: i32 = 16;
const HOLD: Duration = Duration::from_secs(2);
/// Single-finger hold in the top-left corner. Positions are logical pixels.
#[derive(Default)]
struct Corner {
    slot: i32,
    ids: [i32; 2],
    raw: (i32, i32),
    known: (bool, bool),
    down: Option<(Instant, i32, i32)>,
    spoiled: bool,
    fired: bool,
    dropped: bool,
}
impl Corner {
    fn new() -> Self {
        Self {
            ids: [-1, -1],
            ..Self::default()
        }
    }
    /// Feed one event; `map` turns raw axes into logical coordinates.
    fn event(
        &mut self,
        kind: u16,
        code: u16,
        value: i32,
        now: Instant,
        map: impl Fn(i32, i32) -> (i32, i32),
    ) {
        const SYN: u16 = 0;
        const KEY: u16 = 1;
        const ABS: u16 = 3;
        if kind == SYN && code == 3 {
            // SYN_DROPPED: state is unknown until the next report; never fire on it.
            *self = Self {
                dropped: true,
                ..Self::new()
            };
            return;
        }
        if self.dropped {
            if kind == SYN && code == 0 {
                self.dropped = false;
            }
            return;
        }
        match (kind, code) {
            (ABS, 0x2f) => self.slot = value,
            (ABS, 0x39) if (0..2).contains(&self.slot) => {
                self.ids[self.slot as usize] = value;
                if value >= 0 && self.slot == 0 {
                    self.known = (false, false);
                }
            }
            (ABS, 0x35) if self.slot == 0 => (self.raw.0, self.known.0) = (value, true),
            (ABS, 0x36) if self.slot == 0 => (self.raw.1, self.known.1) = (value, true),
            (KEY, 0x14a) if value == 0 => self.ids = [-1, -1], // BTN_TOUCH released
            (SYN, 0) => self.report(now, map),
            _ => {}
        }
    }
    fn report(&mut self, now: Instant, map: impl Fn(i32, i32) -> (i32, i32)) {
        if self.ids[0] < 0 {
            // Lifting the finger re-arms the gesture.
            self.down = None;
            self.spoiled = false;
            self.fired = false;
            return;
        }
        if self.ids[1] >= 0 {
            self.spoiled = true;
        }
        if !(self.known.0 && self.known.1) {
            return;
        }
        let (x, y) = map(self.raw.0, self.raw.1);
        match self.down {
            None => {
                self.down = Some((now, x, y));
                self.spoiled |= x >= ZONE || y >= ZONE;
            }
            Some((_, sx, sy)) => {
                if (x - sx).abs() > SLOP || (y - sy).abs() > SLOP || x >= ZONE || y >= ZONE {
                    self.spoiled = true;
                }
            }
        }
    }
    /// True once per hold, after the finger stayed still in the corner for HOLD.
    fn due(&mut self, now: Instant) -> bool {
        match self.down {
            Some((at, _, _)) if !self.spoiled && !self.fired && now.duration_since(at) >= HOLD => {
                self.fired = true;
                true
            }
            _ => false,
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}
fn abs_info(file: &File, axis: u32) -> Option<AbsInfo> {
    use std::os::fd::AsRawFd;
    // EVIOCGABS(axis) = _IOR('E', 0x40 + axis, struct input_absinfo)
    let request = (2u32 << 30)
        | ((std::mem::size_of::<AbsInfo>() as u32) << 16)
        | (u32::from(b'E') << 8)
        | (0x40 + axis);
    let mut info = AbsInfo::default();
    let rc = unsafe { libc::ioctl(file.as_raw_fd(), request as _, &mut info) };
    (rc == 0 && info.maximum > info.minimum).then_some(info)
}
/// Same scaling and 180° rotation as the screen program's touch handling.
fn logical(raw: i32, info: &AbsInfo, size: i32) -> i32 {
    let n = i64::from(raw - info.minimum) * i64::from(size - 1)
        / i64::from(info.maximum - info.minimum);
    size - 1 - n.clamp(0, i64::from(size - 1)) as i32
}
fn open_touch() -> Option<(File, AbsInfo, AbsInfo)> {
    (0..32).find_map(|i| {
        let name = fs::read_to_string(format!("/sys/class/input/event{i}/device/name")).ok()?;
        if !name.contains("sitronix") {
            return None;
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(format!("/dev/input/event{i}"))
            .ok()?;
        let x = abs_info(&file, 0x35)?;
        let y = abs_info(&file, 0x36)?;
        Some((file, x, y))
    })
}
fn display_on() -> bool {
    fs::read_to_string("/sys/class/leds/led:lcd/brightness")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .is_some_and(|v| v > 0)
}
fn start_corner() {
    std::thread::spawn(|| {
        let mut touch = None;
        let mut gesture = Corner::new();
        let mut revision = 0;
        let mut cooldown = Instant::now();
        loop {
            if touch.is_none() {
                touch = open_touch();
                CORNER_AVAILABLE.store(touch.is_some(), Ordering::Relaxed);
                gesture = Corner::new();
                if touch.is_none() {
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
            }
            let settings = screen_settings::current();
            let armed = settings.corner_shortcut_enabled && !screen::running();
            if revision != settings.revision || !armed {
                revision = settings.revision;
                gesture = Corner::new();
            }
            let (file, ax, ay) = touch.as_mut().unwrap();
            let map = |x: i32, y: i32| (logical(x, ax, W), logical(y, ay, H));
            let mut failed = false;
            // Always drain, so enabling the shortcut never replays old touches.
            for _ in 0..256 {
                let mut bytes = [0u8; std::mem::size_of::<InputEvent>()];
                match file.read(&mut bytes) {
                    Ok(n) if n == bytes.len() => {
                        let event = unsafe {
                            std::ptr::read_unaligned(bytes.as_ptr().cast::<InputEvent>())
                        };
                        if armed {
                            gesture.event(event.kind, event.code, event.value, Instant::now(), map);
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
                touch = None;
                CORNER_AVAILABLE.store(false, Ordering::Relaxed);
                *CORNER_RESULT.lock().unwrap() = "Touchscreen disconnected";
                continue;
            }
            let now = Instant::now();
            // A dark stock screen ignores the gesture: no launches from a pocket.
            if armed && gesture.due(now) && now >= cooldown && display_on() {
                cooldown = now + Duration::from_secs(5);
                let (code, _) = screen::open();
                *CORNER_RESULT.lock().unwrap() = if code == 202 {
                    "Corner long-press requested DevUI"
                } else {
                    "Shortcut blocked: check screen recovery status"
                };
            } else if !settings.corner_shortcut_enabled {
                *CORNER_RESULT.lock().unwrap() = "Shortcut disabled";
            } else {
                let mut result = CORNER_RESULT.lock().unwrap();
                if *result == "Shortcut disabled" || *result == "Waiting for touchscreen" {
                    *result = "Ready: hold the top-left corner for 2 seconds";
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
}
pub fn start() {
    start_corner();
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
    // Raw axes equal logical ones in these tests except for the 180° rotation.
    fn rotated(x: i32, y: i32) -> (i32, i32) {
        (W - 1 - x, H - 1 - y)
    }
    fn touch(c: &mut Corner, at: Instant, x: i32, y: i32) {
        c.event(3, 0x2f, 0, at, rotated);
        c.event(3, 0x39, 7, at, rotated);
        c.event(3, 0x35, W - 1 - x, at, rotated);
        c.event(3, 0x36, H - 1 - y, at, rotated);
        c.event(0, 0, 0, at, rotated);
    }
    fn lift(c: &mut Corner, at: Instant) {
        c.event(3, 0x39, -1, at, rotated);
        c.event(0, 0, 0, at, rotated);
    }
    #[test]
    fn corner_hold_fires_once_after_two_seconds() {
        let t = Instant::now();
        let mut c = Corner::new();
        touch(&mut c, t, 20, 30);
        assert!(!c.due(t + Duration::from_millis(1900)));
        assert!(c.due(t + Duration::from_millis(2000)));
        assert!(!c.due(t + Duration::from_secs(4)), "fires once per hold");
        lift(&mut c, t + Duration::from_secs(4));
        touch(&mut c, t + Duration::from_secs(5), 10, 10);
        assert!(c.due(t + Duration::from_secs(7)), "lifting re-arms");
    }
    #[test]
    fn corner_rejects_short_moved_outside_multi_and_dropped() {
        let t = Instant::now();
        let later = t + Duration::from_secs(3);
        let mut short = Corner::new();
        touch(&mut short, t, 20, 20);
        lift(&mut short, t + Duration::from_millis(500));
        assert!(!short.due(later));
        let mut outside = Corner::new();
        touch(&mut outside, t, 100, 20);
        assert!(!outside.due(later));
        let mut bottom_right = Corner::new();
        touch(&mut bottom_right, t, 300, 460);
        assert!(!bottom_right.due(later));
        let mut moved = Corner::new();
        touch(&mut moved, t, 10, 10);
        moved.event(3, 0x35, W - 1 - 40, t, rotated);
        moved.event(0, 0, 0, t, rotated);
        assert!(!moved.due(later));
        let mut multi = Corner::new();
        touch(&mut multi, t, 10, 10);
        multi.event(3, 0x2f, 1, t, rotated);
        multi.event(3, 0x39, 8, t, rotated);
        multi.event(0, 0, 0, t, rotated);
        assert!(!multi.due(later));
        let mut dropped = Corner::new();
        touch(&mut dropped, t, 10, 10);
        dropped.event(0, 3, 0, t, rotated);
        assert!(!dropped.due(later));
    }
    #[test]
    fn corner_scaling_matches_the_rotated_panel() {
        let info = AbsInfo {
            minimum: 0,
            maximum: 1079,
            ..AbsInfo::default()
        };
        // The raw bottom-right corner is the upright top-left.
        assert_eq!(logical(1079, &info, W), 0);
        assert_eq!(logical(0, &info, W), W - 1);
        assert_eq!(logical(5000, &info, H), 0);
    }
    #[test]
    fn four_slow_presses_are_not_a_shortcut() {
        let mut g = Gesture::default();
        for i in 0..4 {
            assert!(!press(&mut g, 1000 + i * 790, 600));
        }
    }
}
