//! What wakes a session, and the thread that waits for the vertical blank of the screen a window
//! is on, as `CADisplayLink` does on macOS (see "提示" in `docs/design/39-platform-windows.md`).

use anti_webui_core::platform::Shown;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput};
use windows::Win32::Graphics::Gdi::{
    DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW, GetMonitorInfoW, HMONITOR,
    MONITORINFOEXW,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{CreateEventW, SetEvent};
use windows::core::PCWSTR;

/// The time in nanoseconds of the performance counter, which `Instant` reads too.
pub fn monotonic_ns() -> i64 {
    let mut count = 0i64;
    let mut freq = 0i64;
    // SAFETY: valid pointers; neither fails on Windows XP and later.
    unsafe {
        let _ = QueryPerformanceCounter(&mut count);
        let _ = QueryPerformanceFrequency(&mut freq);
    }
    if freq <= 0 {
        return 0;
    }
    (count as i128 * 1_000_000_000 / freq as i128) as i64
}

/// A handle that may go to other threads: events are for that.
pub(crate) struct Handle(pub HANDLE);

// SAFETY: kernel objects may be used from any thread.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: ours, and closed once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct WakeInner {
    woken: AtomicBool,
    event: Handle,
}

/// What wakes a session's wait: an auto-reset event, which `MsgWaitForMultipleObjectsEx` waits for
/// along with the messages.
#[derive(Clone)]
pub(crate) struct Waker(Arc<WakeInner>);

impl Waker {
    pub fn new() -> windows::core::Result<Self> {
        // SAFETY: an unnamed auto-reset event.
        let event = unsafe { CreateEventW(None, false, false, PCWSTR::null())? };
        Ok(Self(Arc::new(WakeInner {
            woken: AtomicBool::new(false),
            event: Handle(event),
        })))
    }

    /// Wakes the session for its handlers' `woken`.
    pub fn wake(&self) {
        self.0.woken.store(true, Ordering::Release);
        self.signal();
    }

    /// Wakes the session without anything for the handlers.
    pub fn signal(&self) {
        // SAFETY: the event lives as long as this.
        unsafe {
            let _ = SetEvent(self.0.event.0);
        }
    }

    pub fn take_woken(&self) -> bool {
        self.0.woken.swap(false, Ordering::AcqRel)
    }

    pub fn is_woken(&self) -> bool {
        self.0.woken.load(Ordering::Acquire)
    }

    pub fn event(&self) -> HANDLE {
        self.0.event.0
    }
}

#[derive(Default)]
struct TickState {
    /// A window waits for the next vertical blank.
    wanted: bool,
    /// The screen of the window that waits.
    monitor: isize,
    /// Counts the vertical blanks that came.
    seq: u64,
    /// The last of them.
    shown: Option<Shown>,
    quit: bool,
}

struct TickerInner {
    state: Mutex<TickState>,
    cond: Condvar,
    wake: Waker,
}

impl TickerInner {
    fn lock(&self) -> MutexGuard<'_, TickState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A session's vertical blanks, from a thread that waits for them while a window asks.
pub(crate) struct Ticker(Arc<TickerInner>);

impl Ticker {
    pub fn new(wake: Waker) -> Self {
        let inner = Arc::new(TickerInner {
            state: Mutex::new(TickState::default()),
            cond: Condvar::new(),
            wake,
        });
        let thread = inner.clone();
        let started = std::thread::Builder::new()
            .name("anti-webui vblank".into())
            .spawn(move || run(&thread));
        if let Err(e) = started {
            eprintln!("[anti-webui] no thread for vertical blanks: {e}");
        }
        Self(inner)
    }

    /// Asks for the next vertical blank of a screen.
    pub fn want(&self, monitor: HMONITOR) {
        let mut s = self.0.lock();
        s.wanted = true;
        s.monitor = monitor.0 as isize;
        drop(s);
        self.0.cond.notify_one();
    }

    /// The count of vertical blanks so far, and the last one.
    pub fn last(&self) -> (u64, Option<Shown>) {
        let s = self.0.lock();
        (s.seq, s.shown)
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.0.lock().quit = true;
        self.0.cond.notify_one();
    }
}

fn run(inner: &TickerInner) {
    let mut outputs = Outputs::default();
    let mut last = Instant::now();
    loop {
        let monitor = {
            let mut s = inner.lock();
            while !s.wanted && !s.quit {
                s = inner.cond.wait(s).unwrap_or_else(|e| e.into_inner());
            }
            if s.quit {
                return;
            }
            s.monitor
        };
        let refresh = outputs.wait(monitor);
        // Something that returns at once, like a screen that is off, mustn't spin.
        let least = Duration::from_nanos(refresh as u64 / 2);
        let since = last.elapsed();
        if since < least {
            std::thread::sleep(least - since);
        }
        last = Instant::now();
        let shown = Shown {
            time_ns: monotonic_ns(),
            refresh_ns: refresh,
            flags: 0,
        };
        let mut s = inner.lock();
        s.seq += 1;
        s.shown = Some(shown);
        s.wanted = false;
        drop(s);
        inner.wake.signal();
    }
}

/// The DXGI output of the screen waited for, and its refresh period.
#[derive(Default)]
struct Outputs {
    monitor: isize,
    output: Option<IDXGIOutput>,
    refresh_ns: u32,
}

impl Outputs {
    /// Waits for the next vertical blank of the screen. Returns its refresh period.
    fn wait(&mut self, monitor: isize) -> u32 {
        if monitor != self.monitor || self.refresh_ns == 0 {
            self.monitor = monitor;
            let m = HMONITOR(monitor as *mut _);
            self.output = output_of(m);
            self.refresh_ns = refresh_of(m);
        }
        if let Some(output) = &self.output {
            // SAFETY: a valid output.
            if unsafe { output.WaitForVBlank() }.is_ok() {
                return self.refresh_ns;
            }
            self.output = None;
        }
        // SAFETY: plain call; fails where nothing composites.
        if unsafe { DwmFlush() }.is_err() {
            std::thread::sleep(Duration::from_nanos(self.refresh_ns as u64));
        }
        self.refresh_ns
    }
}

/// The DXGI output that shows a screen.
fn output_of(monitor: HMONITOR) -> Option<IDXGIOutput> {
    // SAFETY: COM calls on objects made here.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut a = 0;
        while let Ok(adapter) = factory.EnumAdapters1(a) {
            let mut o = 0;
            while let Ok(output) = adapter.EnumOutputs(o) {
                if output.GetDesc().is_ok_and(|d| d.Monitor == monitor) {
                    return Some(output);
                }
                o += 1;
            }
            a += 1;
        }
    }
    None
}

/// The refresh period of a screen in nanoseconds: 60 Hz if it doesn't say.
fn refresh_of(monitor: HMONITOR) -> u32 {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    let mut mode = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    // SAFETY: valid pointers of the sizes said.
    let hz = unsafe {
        let got = GetMonitorInfoW(monitor, &mut info.monitorInfo).as_bool()
            && EnumDisplaySettingsW(
                PCWSTR(info.szDevice.as_ptr()),
                ENUM_CURRENT_SETTINGS,
                &mut mode,
            )
            .as_bool();
        if got { mode.dmDisplayFrequency } else { 0 }
    };
    let hz = if hz > 1 { hz } else { 60 };
    1_000_000_000 / hz
}
