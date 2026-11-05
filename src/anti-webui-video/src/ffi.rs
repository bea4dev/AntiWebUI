//! The parts of FFmpeg this uses, found at run time. Nothing here comes from the headers of FFmpeg
//! other than what its interface is: the names and the types of functions, where fields are, and
//! the values of constants. `tools/offsets.c` is how the numbers were found out. See "ライセンス" in
//! `docs/design/24-video.md`.

use anti_webui_core::video::{VideoError, VideoLibraries, VideoSettings};
use libloading::Library;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

pub const AVERROR_EOF: c_int = -0x20464F45; // FFERRTAG('E', 'O', 'F', ' ')
pub const AVERROR_EAGAIN: c_int = -libc::EAGAIN;
pub const AV_NOPTS_VALUE: i64 = i64::MIN;
pub const AV_TIME_BASE: f64 = 1_000_000.0;
pub const AVMEDIA_TYPE_VIDEO: c_int = 0;
pub const AVMEDIA_TYPE_AUDIO: c_int = 1;
pub const AV_PKT_DATA_DISPLAYMATRIX: c_int = 5;
pub const AV_HWDEVICE_TYPE_VAAPI: c_int = 3;
pub const AV_HWFRAME_MAP_READ: c_int = 1;
pub const AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX: c_int = 1;
pub const AVSEEK_FLAG_BACKWARD: c_int = 1;
pub const SWS_BILINEAR: c_int = 2;
const AV_LOG_QUIET: c_int = -8;
pub const AVCOL_SPC_BT709: c_int = 1;
pub const AVCOL_SPC_BT470BG: c_int = 5;
pub const AVCOL_SPC_SMPTE170M: c_int = 6;
pub const AVCOL_SPC_BT2020_NCL: c_int = 9;
pub const AVCOL_SPC_BT2020_CL: c_int = 10;
pub const AVCOL_RANGE_JPEG: c_int = 2;
pub const AVCOL_TRC_SMPTE2084: c_int = 16;
pub const AVCOL_TRC_ARIB_STD_B67: c_int = 18;

pub type Opaque = c_void;
pub type GetFormat = unsafe extern "C" fn(*mut Opaque, *const c_int) -> c_int;
pub type Interrupt = unsafe extern "C" fn(*mut c_void) -> c_int;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Rational {
    pub num: c_int,
    pub den: c_int,
}

impl Rational {
    pub fn as_f64(self) -> f64 {
        if self.den == 0 {
            0.0
        } else {
            self.num as f64 / self.den as f64
        }
    }
}

/// Which FFmpeg a row of [`LAYOUTS`] is for: the major versions of its libraries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Majors {
    pub avcodec: u32,
    pub avformat: u32,
    pub avutil: u32,
    pub swresample: u32,
    pub swscale: u32,
}

/// Where the fields this reads and writes are. These stay the same within a major version.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub name: &'static str,
    pub majors: Majors,
    pub format_streams: usize,
    pub format_start_time: usize,
    pub format_duration: usize,
    pub format_interrupt: usize,
    pub stream_codecpar: usize,
    pub stream_time_base: usize,
    pub par_codec_id: usize,
    pub par_side_data: usize,
    pub par_nb_side_data: usize,
    pub par_width: usize,
    pub par_height: usize,
    pub codec_get_format: usize,
    pub codec_hw_device: usize,
    pub frame_data: usize,
    pub frame_linesize: usize,
    pub frame_extended_data: usize,
    pub frame_width: usize,
    pub frame_height: usize,
    pub frame_nb_samples: usize,
    pub frame_format: usize,
    pub frame_sample_rate: usize,
    pub frame_color_range: usize,
    pub frame_color_trc: usize,
    pub frame_colorspace: usize,
    pub frame_best_effort: usize,
    pub frame_ch_layout: usize,
    pub frame_duration: usize,
    pub packet_size: usize,
    pub packet_stream_index: usize,
    pub side_data_data: usize,
    pub side_data_size: usize,
}

/// The versions of FFmpeg this knows. Only what has been played with goes here: a wrong number
/// doesn't fail, it crashes.
pub const LAYOUTS: &[Layout] = &[Layout {
    name: "9",
    majors: Majors {
        avcodec: 63,
        avformat: 63,
        avutil: 61,
        swresample: 7,
        swscale: 10,
    },
    format_streams: 48,
    format_start_time: 96,
    format_duration: 104,
    format_interrupt: 216,
    stream_codecpar: 16,
    stream_time_base: 32,
    par_codec_id: 4,
    par_side_data: 32,
    par_nb_side_data: 40,
    par_width: 72,
    par_height: 76,
    codec_get_format: 192,
    codec_hw_device: 560,
    frame_data: 0,
    frame_linesize: 64,
    frame_extended_data: 96,
    frame_width: 104,
    frame_height: 108,
    frame_nb_samples: 112,
    frame_format: 116,
    frame_sample_rate: 180,
    frame_color_range: 280,
    frame_color_trc: 288,
    frame_colorspace: 292,
    frame_best_effort: 304,
    frame_ch_layout: 384,
    frame_duration: 408,
    packet_size: 32,
    packet_stream_index: 36,
    side_data_data: 0,
    side_data_size: 8,
}];

macro_rules! functions {
    ($($lib:ident { $($name:ident: fn($($arg:ty),*) $(-> $ret:ty)?;)* })*) => {
        /// What is called, with the libraries it comes from.
        #[allow(non_snake_case)]
        pub struct Functions {
            $($(pub $name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*)*
        }

        impl Functions {
            fn load(libs: &Libraries) -> Result<Self, VideoError> {
                Ok(Self {
                    $($($name: libs.$lib.function(stringify!($name))?,)*)*
                })
            }
        }
    };
}

functions! {
    avutil {
        av_frame_alloc: fn() -> *mut Opaque;
        av_frame_free: fn(*mut *mut Opaque);
        av_frame_unref: fn(*mut Opaque);
        av_buffer_ref: fn(*const Opaque) -> *mut Opaque;
        av_buffer_unref: fn(*mut *mut Opaque);
        av_hwdevice_ctx_create: fn(*mut *mut Opaque, c_int, *const c_char, *mut Opaque, c_int) -> c_int;
        av_hwframe_map: fn(*mut Opaque, *const Opaque, c_int) -> c_int;
        av_hwframe_transfer_data: fn(*mut Opaque, *const Opaque, c_int) -> c_int;
        av_get_pix_fmt: fn(*const c_char) -> c_int;
        av_get_pix_fmt_name: fn(c_int) -> *const c_char;
        av_opt_set_int: fn(*mut Opaque, *const c_char, i64, c_int) -> c_int;
        av_display_rotation_get: fn(*const i32) -> f64;
        av_strerror: fn(c_int, *mut c_char, usize) -> c_int;
        av_log_set_level: fn(c_int);
        av_channel_layout_default: fn(*mut ChannelLayout, c_int);
        av_channel_layout_uninit: fn(*mut ChannelLayout);
        av_get_sample_fmt: fn(*const c_char) -> c_int;
    }
    avcodec {
        avcodec_alloc_context3: fn(*const Opaque) -> *mut Opaque;
        avcodec_free_context: fn(*mut *mut Opaque);
        avcodec_parameters_to_context: fn(*mut Opaque, *const Opaque) -> c_int;
        avcodec_open2: fn(*mut Opaque, *const Opaque, *mut *mut Opaque) -> c_int;
        avcodec_send_packet: fn(*mut Opaque, *const Opaque) -> c_int;
        avcodec_receive_frame: fn(*mut Opaque, *mut Opaque) -> c_int;
        avcodec_flush_buffers: fn(*mut Opaque);
        avcodec_find_decoder: fn(c_int) -> *const Opaque;
        avcodec_find_decoder_by_name: fn(*const c_char) -> *const Opaque;
        avcodec_get_name: fn(c_int) -> *const c_char;
        avcodec_get_hw_config: fn(*const Opaque, c_int) -> *const HwConfig;
        av_packet_alloc: fn() -> *mut Opaque;
        av_packet_free: fn(*mut *mut Opaque);
        av_packet_side_data_get: fn(*const Opaque, c_int, c_int) -> *const Opaque;
    }
    avformat {
        avformat_alloc_context: fn() -> *mut Opaque;
        avformat_open_input: fn(*mut *mut Opaque, *const c_char, *const Opaque, *mut *mut Opaque) -> c_int;
        avformat_find_stream_info: fn(*mut Opaque, *mut *mut Opaque) -> c_int;
        avformat_close_input: fn(*mut *mut Opaque);
        av_find_best_stream: fn(*mut Opaque, c_int, c_int, c_int, *mut *const Opaque, c_int) -> c_int;
        av_read_frame: fn(*mut Opaque, *mut Opaque) -> c_int;
        av_seek_frame: fn(*mut Opaque, c_int, i64, c_int) -> c_int;
        avformat_network_init: fn() -> c_int;
    }
    swresample {
        swr_alloc_set_opts2: fn(*mut *mut Opaque, *const ChannelLayout, c_int, c_int, *const ChannelLayout, c_int, c_int, c_int, *mut Opaque) -> c_int;
        swr_init: fn(*mut Opaque) -> c_int;
        swr_free: fn(*mut *mut Opaque);
        swr_convert: fn(*mut Opaque, *const *mut u8, c_int, *const *const u8, c_int) -> c_int;
    }
    swscale {
        sws_getContext: fn(c_int, c_int, c_int, c_int, c_int, c_int, c_int, *mut Opaque, *mut Opaque, *const f64) -> *mut Opaque;
        sws_scale: fn(*mut Opaque, *const *const u8, *const c_int, c_int, c_int, *const *mut u8, *const c_int) -> c_int;
        sws_freeContext: fn(*mut Opaque);
    }
}

/// The same in all versions this knows.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ChannelLayout {
    order: c_int,
    pub nb_channels: c_int,
    mask: u64,
    opaque: *mut c_void,
}

impl ChannelLayout {
    pub fn empty() -> Self {
        Self {
            order: 0,
            nb_channels: 0,
            mask: 0,
            opaque: std::ptr::null_mut(),
        }
    }
}

/// The same in all versions this knows.
#[repr(C)]
pub struct HwConfig {
    pub pix_fmt: c_int,
    pub methods: c_int,
    pub device_type: c_int,
}

/// Mapped frames in dmabufs. The same in all versions this knows.
#[repr(C)]
pub struct DrmObject {
    pub fd: c_int,
    pub size: usize,
    pub format_modifier: u64,
}

#[repr(C)]
pub struct DrmPlane {
    pub object_index: c_int,
    pub offset: isize,
    pub pitch: isize,
}

#[repr(C)]
pub struct DrmLayer {
    pub format: u32,
    pub nb_planes: c_int,
    pub planes: [DrmPlane; 4],
}

#[repr(C)]
pub struct DrmDescriptor {
    pub nb_objects: c_int,
    pub objects: [DrmObject; 4],
    pub nb_layers: c_int,
    pub layers: [DrmLayer; 4],
}

/// The libraries themselves, which must stay loaded.
struct Libraries {
    avutil: Lib,
    avcodec: Lib,
    avformat: Lib,
    swresample: Lib,
    swscale: Lib,
}

struct Lib {
    name: String,
    library: Library,
}

impl Lib {
    fn function<T: Copy>(&self, name: &str) -> Result<T, VideoError> {
        // SAFETY: what comes back has the type that `functions!` says FFmpeg gives it.
        unsafe { self.library.get::<T>(name.as_bytes()) }
            .map(|s| *s)
            .map_err(|e| VideoError::Unavailable(format!("{} has no {name}: {e}", self.name)))
    }
}

/// An FFmpeg that can be used.
pub struct Ffmpeg {
    pub f: Functions,
    pub layout: &'static Layout,
    /// What it says about itself.
    pub description: String,
    pub vaapi_pix_fmt: c_int,
    pub drm_prime_pix_fmt: c_int,
    pub nv12_pix_fmt: c_int,
    pub p010_pix_fmt: c_int,
    _libraries: Libraries,
}

// SAFETY: it only holds function pointers and numbers, and FFmpeg's functions can be called from
// any thread for different objects.
unsafe impl Send for Ffmpeg {}
// SAFETY: as above.
unsafe impl Sync for Ffmpeg {}

/// Whether what a library says about its license is allowed. `license` is what `avcodec_license()`
/// and friends return, like `LGPL version 2.1 or later`.
pub fn license_allowed(libraries: VideoLibraries, license: &str) -> bool {
    match libraries {
        VideoLibraries::Any => true,
        VideoLibraries::LgplOnly => license.starts_with("LGPL"),
    }
}

fn file_name(stem: &str, major: u32) -> String {
    format!("lib{stem}.so.{major}")
}

/// Opens a library of a known version, from `dir` if given, or from where the system looks.
fn open(dir: Option<&Path>, stem: &str, major: u32) -> Option<Lib> {
    let name = file_name(stem, major);
    let path = match dir {
        Some(dir) => dir.join(&name),
        None => PathBuf::from(&name),
    };
    // SAFETY: FFmpeg's libraries have no constructors that care about when they are loaded.
    let library = unsafe { Library::new(&path) }.ok()?;
    Some(Lib { name, library })
}

fn version_of(lib: &Lib, function: &str) -> Result<u32, VideoError> {
    let version: unsafe extern "C" fn() -> u32 = lib.function(function)?;
    // SAFETY: takes nothing and returns a number.
    Ok(unsafe { version() } >> 16)
}

fn license_of(lib: &Lib, function: &str) -> Result<String, VideoError> {
    let license: unsafe extern "C" fn() -> *const c_char = lib.function(function)?;
    // SAFETY: returns a static string.
    Ok(unsafe { CStr::from_ptr(license()) }
        .to_string_lossy()
        .into_owned())
}

/// Tries to load a version from a place. `None` when it isn't there.
fn try_layout(
    dir: Option<&Path>,
    layout: &'static Layout,
    libraries: VideoLibraries,
) -> Option<Result<Ffmpeg, VideoError>> {
    let m = layout.majors;
    // In this order: each one needs those before it, which then are found loaded under their names.
    let avutil = open(dir, "avutil", m.avutil)?;
    let swresample = open(dir, "swresample", m.swresample)?;
    let swscale = open(dir, "swscale", m.swscale)?;
    let avcodec = open(dir, "avcodec", m.avcodec)?;
    let avformat = open(dir, "avformat", m.avformat)?;
    Some((|| {
        let found = Majors {
            avcodec: version_of(&avcodec, "avcodec_version")?,
            avformat: version_of(&avformat, "avformat_version")?,
            avutil: version_of(&avutil, "avutil_version")?,
            swresample: version_of(&swresample, "swresample_version")?,
            swscale: version_of(&swscale, "swscale_version")?,
        };
        if found != m {
            return Err(VideoError::Unavailable(format!(
                "the libraries of FFmpeg don't belong together: {found:?}"
            )));
        }
        let licenses = [
            license_of(&avcodec, "avcodec_license")?,
            license_of(&avformat, "avformat_license")?,
            license_of(&avutil, "avutil_license")?,
            license_of(&swresample, "swresample_license")?,
            license_of(&swscale, "swscale_license")?,
        ];
        if let Some(license) = licenses.iter().find(|l| !license_allowed(libraries, l)) {
            return Err(VideoError::Unavailable(format!(
                "this FFmpeg is under \"{license}\", and the settings only allow the LGPL"
            )));
        }
        let libs = Libraries {
            avutil,
            avcodec,
            avformat,
            swresample,
            swscale,
        };
        let f = Functions::load(&libs)?;
        let pix_fmt = |name: &CStr| {
            // SAFETY: a valid string.
            unsafe { (f.av_get_pix_fmt)(name.as_ptr()) }
        };
        let (vaapi_pix_fmt, drm_prime_pix_fmt, nv12_pix_fmt, p010_pix_fmt) = (
            pix_fmt(c"vaapi"),
            pix_fmt(c"drm_prime"),
            pix_fmt(c"nv12"),
            pix_fmt(c"p010le"),
        );
        // SAFETY: both only touch state of the library that is there from the start.
        unsafe {
            if std::env::var_os("ANTIWEBUI_FFMPEG_LOG").is_none() {
                (f.av_log_set_level)(AV_LOG_QUIET);
            }
            (f.avformat_network_init)();
        }
        Ok(Ffmpeg {
            f,
            layout,
            description: format!("FFmpeg {} ({})", layout.name, licenses[0]),
            vaapi_pix_fmt,
            drm_prime_pix_fmt,
            nv12_pix_fmt,
            p010_pix_fmt,
            _libraries: libs,
        })
    })())
}

/// Where to look first.
fn own_dir(settings: &VideoSettings) -> Option<PathBuf> {
    settings.library_dir.clone().or_else(|| {
        std::env::current_exe()
            .ok()?
            .parent()
            .map(|dir| dir.join("ffmpeg"))
    })
}

fn load(settings: &VideoSettings) -> Result<Ffmpeg, VideoError> {
    let own = own_dir(settings);
    load_from(&[own.as_deref(), None], settings.libraries)
}

/// Tries the places in turn, where `None` is where the system looks.
fn load_from(places: &[Option<&Path>], libraries: VideoLibraries) -> Result<Ffmpeg, VideoError> {
    let mut refused = None;
    for dir in places {
        for layout in LAYOUTS {
            match try_layout(*dir, layout, libraries) {
                Some(Ok(ffmpeg)) => return Ok(ffmpeg),
                Some(Err(e)) => refused = Some(e),
                None => {}
            }
        }
    }
    Err(refused.unwrap_or_else(|| {
        let known: Vec<_> = LAYOUTS.iter().map(|l| l.name).collect();
        VideoError::Unavailable(format!(
            "no FFmpeg was found of a version this can use ({})",
            known.join(", ")
        ))
    }))
}

type Key = (VideoLibraries, Option<PathBuf>);
type Loaded = Mutex<HashMap<Key, Result<Arc<Ffmpeg>, VideoError>>>;

/// FFmpeg as these settings want it. What was loaded stays loaded.
pub fn ffmpeg(settings: &VideoSettings) -> Result<Arc<Ffmpeg>, VideoError> {
    static LOADED: OnceLock<Loaded> = OnceLock::new();
    let key = (settings.libraries, settings.library_dir.clone());
    LOADED
        .get_or_init(Default::default)
        .lock()
        .expect("no panics in here")
        .entry(key)
        .or_insert_with(|| load(settings).map(Arc::new))
        .clone()
}

impl Ffmpeg {
    /// What an error number means.
    pub fn error(&self, code: c_int) -> String {
        let mut text = [0 as c_char; 128];
        // SAFETY: the buffer is as long as it says.
        unsafe {
            (self.f.av_strerror)(code, text.as_mut_ptr(), text.len());
            CStr::from_ptr(text.as_ptr()).to_string_lossy().into_owned()
        }
    }

    pub fn pix_fmt_name(&self, format: c_int) -> String {
        // SAFETY: returns a static string or null.
        unsafe {
            let name = (self.f.av_get_pix_fmt_name)(format);
            if name.is_null() {
                String::new()
            } else {
                CStr::from_ptr(name).to_string_lossy().into_owned()
            }
        }
    }

    pub fn set_option(&self, object: *mut Opaque, name: &CStr, value: i64) {
        // SAFETY: `object` is one with options. One it doesn't have only returns an error.
        unsafe { (self.f.av_opt_set_int)(object, name.as_ptr(), value, 0) };
    }
}

/// Reads a field.
///
/// # Safety
///
/// `object` must be valid, and have a `T` at `offset`.
pub unsafe fn read<T: Copy>(object: *const Opaque, offset: usize) -> T {
    // SAFETY: see above.
    unsafe { object.cast::<u8>().add(offset).cast::<T>().read_unaligned() }
}

/// Writes a field.
///
/// # Safety
///
/// `object` must be valid, and have a `T` at `offset`.
pub unsafe fn write<T: Copy>(object: *mut Opaque, offset: usize, value: T) {
    // SAFETY: see above.
    unsafe {
        object
            .cast::<u8>()
            .add(offset)
            .cast::<T>()
            .write_unaligned(value)
    }
}

pub fn c_string(text: &str) -> Result<CString, VideoError> {
    CString::new(text).map_err(|_| VideoError::Source("a NUL in the name".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where there is no FFmpeg, there is an error, whatever the system has.
    #[test]
    fn nothing_there_says_so() {
        let empty = std::env::temp_dir().join(format!("anti-webui-empty-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        let found = load_from(&[Some(empty.as_path())], VideoLibraries::Any);
        let _ = std::fs::remove_dir_all(&empty);
        let Err(VideoError::Unavailable(why)) = found else {
            panic!("that should have found nothing");
        };
        assert!(why.contains("no FFmpeg"), "{why}");
    }

    /// Something else under the right name is the same as nothing, and not a crash.
    #[test]
    fn what_isnt_a_library_is_nothing() {
        let dir = std::env::temp_dir().join(format!("anti-webui-junk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for stem in [
            "avutil.so.61",
            "swresample.so.7",
            "swscale.so.10",
            "avcodec.so.63",
            "avformat.so.63",
        ] {
            std::fs::write(dir.join(format!("lib{stem}")), b"not a library").unwrap();
        }
        let found = load_from(&[Some(dir.as_path())], VideoLibraries::Any);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(matches!(found, Err(VideoError::Unavailable(_))));
    }

    #[test]
    fn descriptors_are_the_size_they_are_in_c() {
        assert_eq!(size_of::<DrmObject>(), 24);
        assert_eq!(size_of::<DrmPlane>(), 24);
        assert_eq!(size_of::<DrmLayer>(), 104);
        assert_eq!(size_of::<DrmDescriptor>(), 528);
        assert_eq!(size_of::<ChannelLayout>(), 24);
    }
}
