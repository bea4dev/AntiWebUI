//! Sound, through ALSA, which PipeWire and PulseAudio also take in. It is found at run time, so that
//! a program that plays a video still starts where there is none. See `docs/design/24-video.md`.

use libloading::Library;
use std::ffi::{CString, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::sync::OnceLock;

const SND_PCM_STREAM_PLAYBACK: c_int = 0;
const SND_PCM_FORMAT_S16_LE: c_int = 2;
const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;
/// How much is in the device at most, in microseconds. Pausing and the volume react within this.
const LATENCY: c_uint = 100_000;

struct Alsa {
    open: unsafe extern "C" fn(*mut *mut c_void, *const c_char, c_int, c_int) -> c_int,
    set_params:
        unsafe extern "C" fn(*mut c_void, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int,
    writei: unsafe extern "C" fn(*mut c_void, *const c_void, c_ulong) -> c_long,
    delay: unsafe extern "C" fn(*mut c_void, *mut c_long) -> c_int,
    recover: unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int,
    drop: unsafe extern "C" fn(*mut c_void) -> c_int,
    prepare: unsafe extern "C" fn(*mut c_void) -> c_int,
    pause: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    close: unsafe extern "C" fn(*mut c_void) -> c_int,
    _library: Library,
}

fn alsa() -> Option<&'static Alsa> {
    static ALSA: OnceLock<Option<Alsa>> = OnceLock::new();
    ALSA.get_or_init(|| {
        // SAFETY: loading ALSA has no effects to care about, and the types below are what it says.
        unsafe {
            let library = Library::new("libasound.so.2").ok()?;
            Some(Alsa {
                open: *library.get(b"snd_pcm_open").ok()?,
                set_params: *library.get(b"snd_pcm_set_params").ok()?,
                writei: *library.get(b"snd_pcm_writei").ok()?,
                delay: *library.get(b"snd_pcm_delay").ok()?,
                recover: *library.get(b"snd_pcm_recover").ok()?,
                drop: *library.get(b"snd_pcm_drop").ok()?,
                prepare: *library.get(b"snd_pcm_prepare").ok()?,
                pause: *library.get(b"snd_pcm_pause").ok()?,
                close: *library.get(b"snd_pcm_close").ok()?,
                _library: library,
            })
        }
    })
    .as_ref()
}

/// Sound going out, as 16 bits in two channels.
pub struct Output {
    alsa: &'static Alsa,
    pcm: *mut c_void,
    pub rate: u32,
}

// SAFETY: it is only ever used from one thread at a time.
unsafe impl Send for Output {}

impl Output {
    /// `None` when there is nowhere for sound to go, which is no reason not to show the picture.
    pub fn open(rate: u32) -> Option<Self> {
        let device = std::env::var("ANTIWEBUI_AUDIO_DEVICE").unwrap_or_else(|_| "default".into());
        if device == "none" {
            return None;
        }
        let alsa = alsa()?;
        let name = CString::new(device).ok()?;
        let mut pcm = std::ptr::null_mut();
        // SAFETY: valid strings and pointers.
        unsafe {
            if (alsa.open)(&mut pcm, name.as_ptr(), SND_PCM_STREAM_PLAYBACK, 0) < 0 {
                return None;
            }
            if (alsa.set_params)(
                pcm,
                SND_PCM_FORMAT_S16_LE,
                SND_PCM_ACCESS_RW_INTERLEAVED,
                2,
                rate,
                1,
                LATENCY,
            ) < 0
            {
                (alsa.close)(pcm);
                return None;
            }
        }
        Some(Self { alsa, pcm, rate })
    }

    /// Hands over pairs of samples, and waits for there to be room. Returns whether that worked.
    pub fn write(&mut self, mut samples: &[i16]) -> bool {
        while !samples.is_empty() {
            let frames = samples.len() / 2;
            // SAFETY: the buffer has this many frames.
            let written =
                unsafe { (self.alsa.writei)(self.pcm, samples.as_ptr().cast(), frames as c_ulong) };
            if written < 0 {
                // SAFETY: an open device, and what it just said.
                if unsafe { (self.alsa.recover)(self.pcm, written as c_int, 1) } < 0 {
                    return false;
                }
                continue;
            }
            samples = &samples[written as usize * 2..];
        }
        true
    }

    /// How many seconds of what was written are yet to be heard.
    pub fn delay(&self) -> f64 {
        let mut frames: c_long = 0;
        // SAFETY: an open device and a valid pointer.
        if unsafe { (self.alsa.delay)(self.pcm, &mut frames) } < 0 {
            return 0.0;
        }
        frames.max(0) as f64 / self.rate as f64
    }

    /// Forgets what is yet to be heard.
    pub fn flush(&mut self) {
        // SAFETY: an open device.
        unsafe {
            (self.alsa.drop)(self.pcm);
            (self.alsa.prepare)(self.pcm);
        }
    }

    /// Stops or goes on. Returns whether the device can: if not, what it held is gone.
    pub fn pause(&mut self, pause: bool) -> bool {
        // SAFETY: an open device.
        let done = unsafe { (self.alsa.pause)(self.pcm, c_int::from(pause)) } >= 0;
        if !done && pause {
            self.flush();
        }
        done
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        // SAFETY: an open device, closed once.
        unsafe { (self.alsa.close)(self.pcm) };
    }
}
