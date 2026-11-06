//! Which Vulkan drivers the loader reads: only those of the GPU the compositor uses, so that a
//! sleeping dGPU isn't woken just to be listed. See "読み込む Vulkan ドライバ" in
//! `docs/design/06-platform-wayland.md`.

use std::path::PathBuf;

/// What says the user chose the drivers, or the GPU to draw on, themselves.
const CHOSEN: &[&str] = &[
    "VK_LOADER_DRIVERS_SELECT",
    "VK_LOADER_DRIVERS_DISABLE",
    "VK_DRIVER_FILES",
    "VK_ICD_FILENAMES",
    "VK_ADD_DRIVER_FILES",
    "DRI_PRIME",
    "__NV_PRIME_RENDER_OFFLOAD",
];

/// What the names of the Vulkan drivers' manifests for a kernel driver have in them. `None` leaves
/// all drivers: one it doesn't know, or NVIDIA's, which is awake for the compositor anyway.
fn manifest_names(kernel_driver: &str) -> Option<&'static [&'static str]> {
    match kernel_driver {
        "amdgpu" | "radeon" => Some(&["radeon", "amd_icd"]),
        "i915" | "xe" => Some(&["intel"]),
        "nouveau" => Some(&["nouveau"]),
        _ => None,
    }
}

/// The major and minor numbers of a `dev_t`, as glibc puts them together.
fn major_minor(dev: u64) -> (u64, u64) {
    (
        ((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff),
        (dev & 0xff) | ((dev >> 12) & !0xff),
    )
}

/// The kernel driver of a device, from sysfs, which doesn't wake it.
fn kernel_driver(dev: u64) -> Option<String> {
    let (major, minor) = major_minor(dev);
    let link = std::fs::read_link(format!("/sys/dev/char/{major}:{minor}/device/driver")).ok()?;
    Some(link.file_name()?.to_string_lossy().into_owned())
}

/// Where the loader looks for driver manifests.
fn manifest_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let var = |name: &str, default: Option<PathBuf>| -> Vec<PathBuf> {
        match std::env::var_os(name).filter(|v| !v.is_empty()) {
            Some(v) => std::env::split_paths(&v).collect(),
            None => default.into_iter().collect(),
        }
    };
    let mut dirs = var("XDG_CONFIG_HOME", home.as_ref().map(|h| h.join(".config")));
    dirs.extend(var("XDG_CONFIG_DIRS", Some("/etc/xdg".into())));
    dirs.push("/etc".into());
    dirs.extend(var(
        "XDG_DATA_HOME",
        home.as_ref().map(|h| h.join(".local/share")),
    ));
    match std::env::var_os("XDG_DATA_DIRS").filter(|v| !v.is_empty()) {
        Some(v) => dirs.extend(std::env::split_paths(&v)),
        None => dirs.extend(["/usr/local/share".into(), "/usr/share".into()]),
    }
    dirs.into_iter().map(|d| d.join("vulkan/icd.d")).collect()
}

/// The file names of the manifests in `dirs` whose names have one of `names` in them.
fn manifests(names: &[&str], dirs: &[PathBuf]) -> Vec<String> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.ends_with(".json") && names.iter().any(|k| n.contains(k)) && !out.contains(&n) {
                out.push(n);
            }
        }
    }
    out.sort();
    out
}

/// Has the Vulkan loader read only the drivers of the GPU with this `dev_t` (the compositor's main
/// device), unless the user chose drivers or a GPU themselves or `ANTIWEBUI_VULKAN_DRIVERS=all`.
/// Returns the drivers' manifests if it did.
///
/// # Safety
///
/// It sets an environment variable, so no other thread may read or set any meanwhile. That holds
/// before the app starts threads of its own.
pub unsafe fn limit_vulkan_drivers(main_device: u64) -> Option<String> {
    if std::env::var("ANTIWEBUI_VULKAN_DRIVERS").is_ok_and(|v| v == "all")
        || CHOSEN.iter().any(|v| std::env::var_os(v).is_some())
    {
        return None;
    }
    let names = manifest_names(&kernel_driver(main_device)?)?;
    let found = manifests(names, &manifest_dirs());
    // Without any, it would have none to draw with.
    if found.is_empty() {
        return None;
    }
    let list = found.join(",");
    // SAFETY: the caller sees to it that nothing else uses the environment meanwhile.
    unsafe { std::env::set_var("VK_LOADER_DRIVERS_SELECT", &list) };
    Some(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_gpu_gets_its_own_drivers_and_nvidia_all() {
        assert_eq!(manifest_names("amdgpu"), Some(&["radeon", "amd_icd"][..]));
        assert_eq!(manifest_names("xe"), Some(&["intel"][..]));
        assert_eq!(manifest_names("nvidia"), None);
        assert_eq!(manifest_names("virtio-pci"), None);
    }

    #[test]
    fn only_the_manifests_of_the_driver_are_found() {
        let dir = std::env::temp_dir().join(format!("antiwebui-icd-{}", std::process::id()));
        let icd = dir.join("vulkan/icd.d");
        std::fs::create_dir_all(&icd).unwrap();
        for n in [
            "radeon_icd.json",
            "nvidia_icd.json",
            "lvp_icd.x86_64.json",
            "amd_icd64.json",
            "radeon.txt",
        ] {
            std::fs::write(icd.join(n), "{}").unwrap();
        }
        let found = manifests(&["radeon", "amd_icd"], &[icd.clone(), icd]);
        assert_eq!(found, ["amd_icd64.json", "radeon_icd.json"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_device_number_is_read_as_the_kernel_writes_it() {
        // 226:128 is /dev/dri/renderD128; a big minor number goes partly above the major.
        assert_eq!(major_minor((226 << 8) | 128), (226, 128));
        let big = (226u64 << 8) | 0x34 | (0x12 << 20);
        assert_eq!(major_minor(big), (226, 0x1234));
    }
}
