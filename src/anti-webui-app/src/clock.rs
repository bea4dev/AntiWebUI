//! The clock frames are timed in: `CLOCK_MONOTONIC` on Linux, which compositors and fences use,
//! `CLOCK_UPTIME_RAW` on macOS, which `Instant` and Core Animation use, and the performance
//! counter on Windows, which `Instant` uses and the Windows backend times vertical blanks with.

#[cfg(target_os = "linux")]
pub(crate) use anti_webui_wgpu::linux_sync::{clock_ns, monotonic_ns};

#[cfg(target_os = "macos")]
pub(crate) fn monotonic_ns() -> i64 {
    clock_ns(libc::CLOCK_UPTIME_RAW).unwrap_or(0)
}

/// The current time of a clock in nanoseconds.
#[cfg(target_os = "macos")]
pub(crate) fn clock_ns(clock: u32) -> std::io::Result<i64> {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: valid pointer to a timespec.
    if unsafe { libc::clock_gettime(clock as libc::clockid_t, &mut ts) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(ts.tv_sec * 1_000_000_000 + ts.tv_nsec)
}

/// The performance counter in nanoseconds.
#[cfg(windows)]
pub(crate) fn monotonic_ns() -> i64 {
    let mut count = 0i64;
    let mut freq = 0i64;
    // SAFETY: valid pointers; neither fails on Windows XP and later.
    unsafe {
        QueryPerformanceCounter(&mut count);
        QueryPerformanceFrequency(&mut freq);
    }
    if freq <= 0 {
        return 0;
    }
    (count as i128 * 1_000_000_000 / freq as i128) as i64
}

/// There is only the one clock: the Windows backend says `0` for it.
#[cfg(windows)]
pub(crate) fn clock_ns(_clock: u32) -> std::io::Result<i64> {
    Ok(monotonic_ns())
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryPerformanceCounter(count: *mut i64) -> i32;
    fn QueryPerformanceFrequency(freq: *mut i64) -> i32;
}
