//! Which GPU wgpu's DX12 backend looks at: only the low-power one, so that a sleeping dGPU isn't
//! woken just to be listed. See "使う GPU" in `docs/design/39-platform-windows.md`.

use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_GPU_PREFERENCE, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
    DXGI_GPU_PREFERENCE_MINIMUM_POWER, IDXGIAdapter1, IDXGIFactory6,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
use windows::core::{HSTRING, w};

/// What a LUID is to `wgpu::Dx12AdapterFilter`.
fn luid(l: windows::Win32::Foundation::LUID) -> u64 {
    ((l.HighPart as u32 as u64) << 32) | u64::from(l.LowPart)
}

/// The LUID of a DX12 adapter.
pub(crate) fn luid_of(adapter: &wgpu::Adapter) -> Option<u64> {
    // SAFETY: only reads the adapter's description.
    let hal = unsafe { adapter.as_hal::<wgpu::hal::api::Dx12>() }?;
    let desc = unsafe { hal.raw_adapter().GetDesc1() }.ok()?;
    Some(luid(desc.AdapterLuid))
}

/// The adapter to let wgpu see: the first by `MINIMUM_POWER`, or by `HIGH_PERFORMANCE` if the
/// user chose that for this app in Windows' graphics settings. Not the one the window's monitor
/// is on: NVIDIA's driver claims the monitor for apps whose executable has a name in its profiles
/// (`showcase.exe` is one), and that would wake the dGPU. Listing adapters doesn't wake a GPU;
/// wgpu making a device on one does.
pub(crate) fn preferred() -> wgpu::Dx12AdapterFilter {
    if std::env::var("ANTIWEBUI_ADAPTERS").is_ok_and(|v| v == "all")
        || std::env::var_os("WGPU_ADAPTER_NAME").is_some()
    {
        return wgpu::Dx12AdapterFilter::All;
    }
    let preference = if user_wants_high_performance() {
        DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE
    } else {
        DXGI_GPU_PREFERENCE_MINIMUM_POWER
    };
    match first_by(preference) {
        Some(id) => wgpu::Dx12AdapterFilter::Luids(vec![id]),
        None => wgpu::Dx12AdapterFilter::All,
    }
}

fn first_by(preference: DXGI_GPU_PREFERENCE) -> Option<u64> {
    // SAFETY: plain calls.
    let factory = unsafe { CreateDXGIFactory1::<IDXGIFactory6>() }.ok()?;
    let adapter: IDXGIAdapter1 = unsafe { factory.EnumAdapterByGpuPreference(0, preference) }.ok()?;
    let desc = unsafe { adapter.GetDesc1() }.ok()?;
    Some(luid(desc.AdapterLuid))
}

/// Whether Windows' graphics settings say "high performance" for this executable: a value named
/// by its path under `UserGpuPreferences`, like `GpuPreference=2;`.
fn user_wants_high_performance() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let name = HSTRING::from(exe.as_os_str());
    let mut buf = [0u16; 256];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: the buffer and its length in bytes go together.
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\DirectX\\UserGpuPreferences"),
            &name,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    if read.is_err() {
        return false;
    }
    let value = String::from_utf16_lossy(&buf[..(len as usize / 2).min(buf.len())]);
    value
        .split(';')
        .any(|p| p.trim().trim_end_matches('\0') == "GpuPreference=2")
}
