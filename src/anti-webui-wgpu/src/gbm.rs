//! Buffers from GBM, the allocator of the display driver, loaded at run time. A buffer the
//! compositor may put on the screen as it is (direct scanout) has to come from here: the NVIDIA
//! driver shows ones that Vulkan allocated and exported as garbage, though the modifier is the
//! same (gamescope #2309). wlroots, smithay and Chromium allocate with GBM too. See "バッファ" in
//! `docs/design/06-platform-wayland.md`.

use crate::dmabuf::DmabufPlane;
use std::ffi::c_void;
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::rc::Rc;

const GBM_BO_USE_SCANOUT: u32 = 1 << 0;
const GBM_BO_USE_RENDERING: u32 = 1 << 2;

type CreateDevice = unsafe extern "C" fn(fd: i32) -> *mut c_void;
type DeviceDestroy = unsafe extern "C" fn(dev: *mut c_void);
type CreateWithModifiers2 = unsafe extern "C" fn(
    dev: *mut c_void,
    width: u32,
    height: u32,
    format: u32,
    modifiers: *const u64,
    count: u32,
    flags: u32,
) -> *mut c_void;
type BoDestroy = unsafe extern "C" fn(bo: *mut c_void);
type BoGetFd = unsafe extern "C" fn(bo: *mut c_void) -> i32;
type BoGetModifier = unsafe extern "C" fn(bo: *mut c_void) -> u64;
type BoGetPlaneCount = unsafe extern "C" fn(bo: *mut c_void) -> i32;
type BoGetForPlane = unsafe extern "C" fn(bo: *mut c_void, plane: i32) -> u32;

struct Fns {
    device_destroy: DeviceDestroy,
    create_with_modifiers2: CreateWithModifiers2,
    bo_destroy: BoDestroy,
    bo_get_fd: BoGetFd,
    bo_get_modifier: BoGetModifier,
    bo_get_plane_count: BoGetPlaneCount,
    bo_get_stride_for_plane: BoGetForPlane,
    bo_get_offset: BoGetForPlane,
}

/// A GBM device on a DRM node.
pub(crate) struct Device {
    raw: *mut c_void,
    fns: Fns,
    /// What the functions come from, kept loaded while the device is.
    _lib: libloading::Library,
    /// GBM doesn't take the descriptor over.
    _node: File,
}

/// A buffer object, kept while its image is: the dmabuf and Vulkan's import hold the memory as
/// well, but this is how GBM expects to be used.
pub(crate) struct Bo {
    raw: *mut c_void,
    device: Rc<Device>,
}

/// A new buffer, ready to be imported.
pub(crate) struct Allocated {
    pub bo: Bo,
    pub fd: OwnedFd,
    pub modifier: u64,
    pub planes: Vec<DmabufPlane>,
}

impl Device {
    /// GBM on `node`, if libgbm is there and takes it.
    pub(crate) fn open(node: &Path) -> Option<Rc<Self>> {
        // SAFETY: libgbm's initializers have no requirements.
        let lib = unsafe { libloading::Library::new("libgbm.so.1") }.ok()?;
        // SAFETY: the types are those of gbm.h.
        let (create, fns) = unsafe {
            let create = *lib.get::<CreateDevice>(b"gbm_create_device\0").ok()?;
            let fns = Fns {
                device_destroy: *lib.get(b"gbm_device_destroy\0").ok()?,
                create_with_modifiers2: *lib.get(b"gbm_bo_create_with_modifiers2\0").ok()?,
                bo_destroy: *lib.get(b"gbm_bo_destroy\0").ok()?,
                bo_get_fd: *lib.get(b"gbm_bo_get_fd\0").ok()?,
                bo_get_modifier: *lib.get(b"gbm_bo_get_modifier\0").ok()?,
                bo_get_plane_count: *lib.get(b"gbm_bo_get_plane_count\0").ok()?,
                bo_get_stride_for_plane: *lib.get(b"gbm_bo_get_stride_for_plane\0").ok()?,
                bo_get_offset: *lib.get(b"gbm_bo_get_offset\0").ok()?,
            };
            (create, fns)
        };
        let node = File::options().read(true).write(true).open(node).ok()?;
        // SAFETY: an open DRM node, kept open as long as the device.
        let raw = unsafe { create(node.as_raw_fd()) };
        (!raw.is_null()).then(|| {
            Rc::new(Self {
                raw,
                fns,
                _lib: lib,
                _node: node,
            })
        })
    }

    /// A buffer of `format` with one of `modifiers`, which the display can show as it is and we
    /// can render to.
    pub(crate) fn allocate(
        self: &Rc<Self>,
        width: u32,
        height: u32,
        format: u32,
        modifiers: &[u64],
    ) -> Option<Allocated> {
        if modifiers.is_empty() {
            return None;
        }
        let f = &self.fns;
        // SAFETY: a live device and a list of `modifiers.len()` modifiers.
        let raw = unsafe {
            (f.create_with_modifiers2)(
                self.raw,
                width,
                height,
                format,
                modifiers.as_ptr(),
                modifiers.len() as u32,
                GBM_BO_USE_SCANOUT | GBM_BO_USE_RENDERING,
            )
        };
        if raw.is_null() {
            return None;
        }
        let bo = Bo {
            raw,
            device: self.clone(),
        };
        // SAFETY: a live buffer object.
        let (fd, modifier, count) = unsafe {
            (
                (f.bo_get_fd)(raw),
                (f.bo_get_modifier)(raw),
                (f.bo_get_plane_count)(raw),
            )
        };
        if fd < 0 || count < 1 || !modifiers.contains(&modifier) {
            if fd >= 0 {
                // SAFETY: a new descriptor that nothing else has.
                drop(unsafe { OwnedFd::from_raw_fd(fd) });
            }
            return None;
        }
        // SAFETY: a new descriptor that belongs to the caller.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let planes = (0..count)
            .map(|i| {
                // SAFETY: a live buffer object and a plane it has.
                unsafe {
                    DmabufPlane {
                        offset: (f.bo_get_offset)(raw, i),
                        stride: (f.bo_get_stride_for_plane)(raw, i),
                    }
                }
            })
            .collect();
        Some(Allocated {
            bo,
            fd,
            modifier,
            planes,
        })
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: every buffer object holds the device, so none is left.
        unsafe { (self.fns.device_destroy)(self.raw) };
    }
}

impl Drop for Bo {
    fn drop(&mut self) {
        // SAFETY: a live buffer object of this device.
        unsafe { (self.device.fns.bo_destroy)(self.raw) };
    }
}
