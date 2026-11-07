//! The clock frames are timed in: `CLOCK_MONOTONIC` on Linux, which compositors and fences use,
//! and `CLOCK_UPTIME_RAW` on macOS, which `Instant` and Core Animation use.

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
