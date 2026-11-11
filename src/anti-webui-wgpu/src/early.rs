//! The GPU made on another thread while the first window is made (Windows). Making the DX12
//! device takes about 190 ms; making the window and having it activated about 50 ms of that. See
//! "GPU を先に作る" in `docs/design/39-platform-windows.md`.

use crate::{Gpu, adapters, instance_desc};
use raw_window_handle::{RawDisplayHandle, WindowsDisplayHandle};
use std::sync::Mutex;
use std::thread::JoinHandle;

static EARLY: Mutex<Option<JoinHandle<Option<Gpu>>>> = Mutex::new(None);

pub(crate) fn start() {
    // A backend the user chose goes the usual way, with all its tries.
    if std::env::var_os("WGPU_BACKEND").is_some() {
        return;
    }
    let mut early = EARLY.lock().unwrap_or_else(|e| e.into_inner());
    if early.is_some() {
        return;
    }
    *early = std::thread::Builder::new()
        .name("anti-webui gpu".into())
        .spawn(|| {
            let display = RawDisplayHandle::Windows(WindowsDisplayHandle::new());
            let desc = instance_desc(
                display,
                wgpu::Backends::DX12,
                false,
                adapters::preferred(),
            );
            let instance = wgpu::Instance::new(desc);
            let adapter =
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter: false,
                    apply_limit_buckets: false,
                }))
                .ok()?;
            Gpu::open(instance, adapter).ok()
        })
        .ok();
}

/// The GPU made ahead, once; waits for it if it isn't made yet.
pub(crate) fn take() -> Option<Gpu> {
    let handle = EARLY.lock().unwrap_or_else(|e| e.into_inner()).take()?;
    handle.join().ok().flatten()
}
