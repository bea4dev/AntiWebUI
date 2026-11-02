//! Kernel synchronization primitives used to share buffers with a compositor: sync files on
//! dmabufs (implicit sync) and DRM syncobj timelines (explicit sync).

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;

/// `_IOC(dir, type, nr, size)` with the generic Linux encoding.
const fn ioc(dir: u64, ty: u8, nr: u8, size: usize) -> u64 {
    (dir << 30) | ((size as u64) << 16) | ((ty as u64) << 8) | nr as u64
}
const IOC_WRITE: u64 = 1;
const IOC_READWRITE: u64 = 3;

fn ioctl<T>(fd: RawFd, request: u64, arg: &mut T) -> io::Result<()> {
    loop {
        // SAFETY: `request` encodes the size of `T`, which matches the kernel's structure.
        let r = unsafe { libc::ioctl(fd, request as _, arg as *mut T) };
        if r == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// dma-buf sync files

#[repr(C)]
struct DmaBufSyncFile {
    flags: u32,
    fd: i32,
}

const DMA_BUF_SYNC_READ: u32 = 1;
const DMA_BUF_SYNC_WRITE: u32 = 2;
const DMA_BUF_IOCTL_EXPORT_SYNC_FILE: u64 =
    ioc(IOC_READWRITE, b'b', 2, size_of::<DmaBufSyncFile>());
const DMA_BUF_IOCTL_IMPORT_SYNC_FILE: u64 = ioc(IOC_WRITE, b'b', 3, size_of::<DmaBufSyncFile>());

/// A sync file that signals when every pending access to the dmabuf is done, so that it can be
/// written again.
pub fn dmabuf_export_write_fence(dmabuf: BorrowedFd<'_>) -> io::Result<OwnedFd> {
    let mut arg = DmaBufSyncFile {
        flags: DMA_BUF_SYNC_WRITE,
        fd: -1,
    };
    ioctl(dmabuf.as_raw_fd(), DMA_BUF_IOCTL_EXPORT_SYNC_FILE, &mut arg)?;
    // SAFETY: the kernel returned a new file descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(arg.fd) })
}

/// Attaches `sync_file` to the dmabuf as a write, so readers using implicit sync wait for it.
pub fn dmabuf_import_write_fence(
    dmabuf: BorrowedFd<'_>,
    sync_file: BorrowedFd<'_>,
) -> io::Result<()> {
    let mut arg = DmaBufSyncFile {
        flags: DMA_BUF_SYNC_WRITE | DMA_BUF_SYNC_READ,
        fd: sync_file.as_raw_fd(),
    };
    ioctl(dmabuf.as_raw_fd(), DMA_BUF_IOCTL_IMPORT_SYNC_FILE, &mut arg)
}

// ---------------------------------------------------------------------------------------------
// DRM syncobj

#[repr(C)]
struct SyncobjCreate {
    handle: u32,
    flags: u32,
}

#[repr(C)]
struct SyncobjDestroy {
    handle: u32,
    pad: u32,
}

#[repr(C)]
struct SyncobjHandle {
    handle: u32,
    flags: u32,
    fd: i32,
    pad: u32,
}

#[repr(C)]
struct SyncobjTimelineWait {
    handles: u64,
    points: u64,
    timeout_nsec: i64,
    count_handles: u32,
    flags: u32,
    first_signaled: u32,
    pad: u32,
}

#[repr(C)]
struct SyncobjTransfer {
    src_handle: u32,
    dst_handle: u32,
    src_point: u64,
    dst_point: u64,
    flags: u32,
    pad: u32,
}

#[repr(C)]
struct SyncobjTimelineArray {
    handles: u64,
    points: u64,
    count_handles: u32,
    flags: u32,
}

const fn drm_iowr<T>(nr: u8) -> u64 {
    ioc(IOC_READWRITE, b'd', nr, size_of::<T>())
}
const DRM_IOCTL_SYNCOBJ_CREATE: u64 = drm_iowr::<SyncobjCreate>(0xBF);
const DRM_IOCTL_SYNCOBJ_DESTROY: u64 = drm_iowr::<SyncobjDestroy>(0xC0);
const DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD: u64 = drm_iowr::<SyncobjHandle>(0xC1);
const DRM_IOCTL_SYNCOBJ_FD_TO_HANDLE: u64 = drm_iowr::<SyncobjHandle>(0xC2);
const DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT: u64 = drm_iowr::<SyncobjTimelineWait>(0xCA);
const DRM_IOCTL_SYNCOBJ_TRANSFER: u64 = drm_iowr::<SyncobjTransfer>(0xCC);
const DRM_IOCTL_SYNCOBJ_TIMELINE_SIGNAL: u64 = drm_iowr::<SyncobjTimelineArray>(0xCD);

const DRM_SYNCOBJ_FD_TO_HANDLE_FLAGS_IMPORT_SYNC_FILE: u32 = 1;
const DRM_SYNCOBJ_WAIT_FLAGS_WAIT_FOR_SUBMIT: u32 = 2;

/// An open DRM device node, used for syncobj operations.
#[derive(Debug)]
pub struct DrmDevice {
    fd: OwnedFd,
}

impl DrmDevice {
    pub fn open(path: &Path) -> io::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(path)?;
        Ok(Self { fd: file.into() })
    }

    fn raw(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    pub fn create_syncobj(&self) -> io::Result<u32> {
        let mut arg = SyncobjCreate {
            handle: 0,
            flags: 0,
        };
        ioctl(self.raw(), DRM_IOCTL_SYNCOBJ_CREATE, &mut arg)?;
        Ok(arg.handle)
    }

    pub fn destroy_syncobj(&self, handle: u32) {
        let mut arg = SyncobjDestroy { handle, pad: 0 };
        let _ = ioctl(self.raw(), DRM_IOCTL_SYNCOBJ_DESTROY, &mut arg);
    }

    /// A file descriptor for the syncobj itself (to share the timeline).
    pub fn export_syncobj(&self, handle: u32) -> io::Result<OwnedFd> {
        let mut arg = SyncobjHandle {
            handle,
            flags: 0,
            fd: -1,
            pad: 0,
        };
        ioctl(self.raw(), DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD, &mut arg)?;
        // SAFETY: the kernel returned a new file descriptor.
        Ok(unsafe { OwnedFd::from_raw_fd(arg.fd) })
    }

    /// Makes timeline point `point` of `timeline` signal when `sync_file` does, or now if there
    /// is no sync file.
    pub fn set_point(
        &self,
        timeline: u32,
        point: u64,
        scratch: u32,
        sync_file: Option<BorrowedFd<'_>>,
    ) -> io::Result<()> {
        let Some(sync_file) = sync_file else {
            let mut arg = SyncobjTimelineArray {
                handles: &timeline as *const u32 as u64,
                points: &point as *const u64 as u64,
                count_handles: 1,
                flags: 0,
            };
            return ioctl(self.raw(), DRM_IOCTL_SYNCOBJ_TIMELINE_SIGNAL, &mut arg);
        };
        let mut import = SyncobjHandle {
            handle: scratch,
            flags: DRM_SYNCOBJ_FD_TO_HANDLE_FLAGS_IMPORT_SYNC_FILE,
            fd: sync_file.as_raw_fd(),
            pad: 0,
        };
        ioctl(self.raw(), DRM_IOCTL_SYNCOBJ_FD_TO_HANDLE, &mut import)?;
        let mut transfer = SyncobjTransfer {
            src_handle: scratch,
            dst_handle: timeline,
            src_point: 0,
            dst_point: point,
            flags: 0,
            pad: 0,
        };
        ioctl(self.raw(), DRM_IOCTL_SYNCOBJ_TRANSFER, &mut transfer)
    }

    /// Waits until `point` of `timeline` has signaled or the absolute `CLOCK_MONOTONIC`
    /// deadline passes (0 only checks). Returns whether it signaled.
    pub fn wait_point(&self, timeline: u32, point: u64, deadline_ns: i64) -> io::Result<bool> {
        let mut arg = SyncobjTimelineWait {
            handles: &timeline as *const u32 as u64,
            points: &point as *const u64 as u64,
            timeout_nsec: deadline_ns,
            count_handles: 1,
            flags: DRM_SYNCOBJ_WAIT_FLAGS_WAIT_FOR_SUBMIT,
            first_signaled: 0,
            pad: 0,
        };
        match ioctl(self.raw(), DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT, &mut arg) {
            Ok(()) => Ok(true),
            Err(e) if e.raw_os_error() == Some(libc::ETIME) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

/// The current `CLOCK_MONOTONIC` time in nanoseconds, for syncobj deadlines.
pub fn monotonic_ns() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: valid pointer to a timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec * 1_000_000_000 + ts.tv_nsec
}
