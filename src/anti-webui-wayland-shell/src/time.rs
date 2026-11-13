//! The time of day, for clocks in bars: it moves on at the turn of each second or minute, not
//! every frame.

use anti_webui_core::animation::set_timeout;
use anti_webui_core::reactive::{Signal, create_root, signal};
use anti_webui_core::window::current_window;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CString, c_char};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How often a [`local_time`] moves on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Every {
    Second,
    Minute,
}

impl Every {
    fn seconds(self) -> u64 {
        match self {
            Every::Second => 1,
            Every::Minute => 60,
        }
    }
}

/// A moment in the local time zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    /// 1 to 12.
    pub month: u8,
    /// 1 to 31.
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// 0 is Sunday.
    pub weekday: u8,
    /// Seconds since 1970 in UTC.
    pub unix: i64,
}

unsafe extern "C" {
    fn tzset();
}

impl LocalTime {
    pub fn now() -> Self {
        let unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        Self::at(unix)
    }

    pub fn at(unix: i64) -> Self {
        let tm = local_tm(unix);
        Self {
            year: tm.tm_year + 1900,
            month: (tm.tm_mon + 1) as u8,
            day: tm.tm_mday as u8,
            hour: tm.tm_hour as u8,
            minute: tm.tm_min as u8,
            second: tm.tm_sec as u8,
            weekday: tm.tm_wday as u8,
            unix,
        }
    }

    /// As C's `strftime`, like `%H:%M` or `%Y-%m-%d %a`.
    pub fn format(&self, format: &str) -> String {
        let Ok(format) = CString::new(format) else {
            return String::new();
        };
        let tm = local_tm(self.unix);
        let mut buf = vec![0u8; 256];
        // SAFETY: `buf` is as long as said, and `format` and `tm` are valid for the call.
        let n = unsafe {
            libc::strftime(
                buf.as_mut_ptr() as *mut c_char,
                buf.len(),
                format.as_ptr(),
                &tm,
            )
        };
        buf.truncate(n);
        String::from_utf8_lossy(&buf).into_owned()
    }
}

fn local_tm(unix: i64) -> libc::tm {
    static ZONE: std::sync::Once = std::sync::Once::new();
    // SAFETY: reads `TZ` once; `localtime_r` writes only to `tm`.
    unsafe {
        ZONE.call_once(|| tzset());
        let t = unix as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    }
}

thread_local! {
    /// A clock per window and pace: timers belong to the window they were set in.
    static CLOCKS: RefCell<HashMap<(u64, Every), Signal<LocalTime>>> = RefCell::default();
}

/// The time now, moving on at the turn of every second or minute. Reading it tracks it. Read it in
/// a view: it keeps time with the window it is read in.
pub fn local_time(every: Every) -> LocalTime {
    let window = current_window().id().get();
    let found = CLOCKS.with(|c| c.borrow().get(&(window, every)).copied());
    let clock = found.unwrap_or_else(|| {
        let (clock, _) = create_root(|| {
            let clock = signal(LocalTime::now());
            // The first reader may recompute or disappear before the first tick.
            tick(clock, every);
            clock
        });
        CLOCKS.with(|c| c.borrow_mut().insert((window, every), clock));
        clock
    });
    clock.get()
}

/// Until the turn of the next second or minute, and then again.
fn tick(clock: Signal<LocalTime>, every: Every) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let step = every.seconds();
    let into =
        Duration::from_secs(now.as_secs() % step) + Duration::from_nanos(now.subsec_nanos() as u64);
    // A little after the turn, so that the time read is past it.
    let wait = Duration::from_secs(step).saturating_sub(into) + Duration::from_millis(5);
    set_timeout(wait, move || {
        if clock.is_alive() {
            clock.set(LocalTime::now());
            tick(clock, every);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use anti_webui_core::Ui;
    use anti_webui_core::reactive::memo;
    use anti_webui_core::view::div;
    use std::time::Instant;

    // Simulate an old cached reading without sleeping or changing the system clock.
    fn stale_clock(every: Every) {
        let window = current_window().id().get();
        let clock = CLOCKS.with(|c| c.borrow()[&(window, every)]);
        clock.update_untracked(|time| *time = LocalTime::at(0));
    }

    #[test]
    fn clock_keeps_updating_after_its_first_reader_recomputes() {
        let mut ui = Ui::new(div);
        let ((setting, displayed), owner) = create_root(|| {
            let setting = signal(false);
            let displayed = memo(move || {
                setting.get();
                local_time(Every::Minute).unix
            });
            (setting, displayed)
        });

        for value in [true, false] {
            let deadline = ui.next_deadline(Instant::now()).unwrap();
            stale_clock(Every::Minute);
            setting.set(value);
            assert_eq!(displayed.get(), 0);

            let before = LocalTime::now().unix;
            ui.tick(deadline);
            assert!((before..=LocalTime::now().unix).contains(&displayed.get()));
        }
        owner.dispose();
    }

    #[test]
    fn clock_keeps_updating_after_its_first_reader_is_replaced() {
        let mut ui = Ui::new(div);
        let (_, first) = create_root(|| local_time(Every::Second));
        let deadline = ui.next_deadline(Instant::now()).unwrap();
        first.dispose();
        stale_clock(Every::Second);
        let (displayed, replacement) = create_root(|| memo(|| local_time(Every::Second).unix));
        assert_eq!(displayed.get(), 0);

        let before = LocalTime::now().unix;
        ui.tick(deadline);
        assert!((before..=LocalTime::now().unix).contains(&displayed.get()));
        assert!(ui.next_deadline(Instant::now()).is_some());
        replacement.dispose();
    }

    #[test]
    fn formats_as_strftime() {
        let t = LocalTime::at(0);
        assert_eq!(t.format("%Y").len(), 4);
        assert_eq!(t.format("%%"), "%");
        assert_eq!(t.format(""), "");
    }
}
