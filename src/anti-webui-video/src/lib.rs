//! Plays videos for AntiWebUI with an FFmpeg found at run time. This crate holds nothing of FFmpeg
//! and needs nothing of it to build. See `docs/design/24-video.md`.

mod audio;
pub mod cadence;
mod ffi;
mod player;

use anti_webui_core::video::{
    VideoBackend, VideoError, VideoRequest, VideoSettings, VideoStream, set_video_backend,
};
use std::rc::Rc;

/// Decodes with FFmpeg.
pub struct FfmpegBackend;

impl VideoBackend for FfmpegBackend {
    fn open(&self, request: VideoRequest) -> Box<dyn VideoStream> {
        Box::new(player::FfmpegStream::open(request))
    }

    fn availability(&self, settings: &VideoSettings) -> Result<String, VideoError> {
        ffi::ffmpeg(settings).map(|f| f.description.clone())
    }

    fn can_play(&self, mime: &str, settings: &VideoSettings) -> bool {
        let Ok(ffmpeg) = ffi::ffmpeg(settings) else {
            return false;
        };
        let has = |name: &std::ffi::CStr| {
            // SAFETY: a valid string.
            !unsafe { (ffmpeg.f.avcodec_find_decoder_by_name)(name.as_ptr()) }.is_null()
        };
        can_play(mime, has)
    }
}

/// Which decoders it takes to play a type like `video/mp4; codecs="avc1.42E01E, mp4a.40.2"`, and
/// whether `has` knows those.
fn can_play(mime: &str, has: impl Fn(&std::ffi::CStr) -> bool) -> bool {
    let mut parts = mime.split(';');
    let container = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let defaults: &[&str] = match container.as_str() {
        "video/mp4" | "audio/mp4" | "video/quicktime" => &["avc1", "mp4a"],
        "video/webm" | "audio/webm" => &["vp8", "vorbis"],
        "video/ogg" | "audio/ogg" => &["vorbis"],
        "video/x-matroska" => &["avc1"],
        _ => return false,
    };
    let codecs: Vec<String> = parts
        .find_map(|p| {
            let p = p.trim();
            let value = p
                .strip_prefix("codecs=")
                .or_else(|| p.strip_prefix("codecs ="))?;
            Some(
                value
                    .trim_matches('"')
                    .split(',')
                    .map(|c| c.trim().to_ascii_lowercase())
                    .collect(),
            )
        })
        .unwrap_or_else(|| defaults.iter().map(|s| (*s).to_owned()).collect());
    codecs.iter().all(|codec| {
        let family = codec.split('.').next().unwrap_or("");
        match family {
            "avc1" | "avc3" => has(c"h264"),
            "hev1" | "hvc1" => has(c"hevc"),
            "vp8" | "vp08" => has(c"vp8"),
            "vp9" | "vp09" => has(c"vp9"),
            "av01" | "av1" => has(c"libdav1d") || has(c"av1"),
            "mp4a" | "aac" => has(c"aac"),
            "mp3" => has(c"mp3float") || has(c"mp3"),
            "opus" => has(c"opus") || has(c"libopus"),
            "vorbis" => has(c"vorbis") || has(c"libvorbis"),
            "flac" => has(c"flac"),
            _ => false,
        }
    })
}

/// Makes this what plays videos on this thread.
pub fn install() {
    set_video_backend(Rc::new(FfmpegBackend));
}

#[cfg(test)]
mod tests {
    use super::*;
    use anti_webui_core::video::VideoLibraries;

    #[test]
    fn types() {
        let all = |_: &std::ffi::CStr| true;
        let only = |wanted: &'static [&'static str]| {
            move |name: &std::ffi::CStr| wanted.iter().any(|w| name.to_bytes() == w.as_bytes())
        };
        assert!(can_play("video/mp4", all));
        assert!(can_play("video/webm; codecs=\"vp9, opus\"", all));
        assert!(can_play(
            "VIDEO/MP4; codecs=\"avc1.42E01E, mp4a.40.2\"",
            all
        ));
        assert!(!can_play("video/nonsense", all));
        assert!(!can_play("video/mp4; codecs=\"nonsense\"", all));
        assert!(can_play("video/webm; codecs=vp9", only(&["vp9"])));
        assert!(!can_play("video/mp4; codecs=avc1", only(&["vp9"])));
        assert!(can_play("video/mp4; codecs=av01", only(&["libdav1d"])));
        // Without codecs, what is usual in it has to be there.
        assert!(!can_play("video/mp4", only(&["h264"])));
        assert!(can_play("video/mp4", only(&["h264", "aac"])));
    }

    #[test]
    fn licenses() {
        let gpl = "GPL version 3 or later";
        let lgpl = "LGPL version 2.1 or later";
        assert!(ffi::license_allowed(VideoLibraries::Any, gpl));
        assert!(ffi::license_allowed(VideoLibraries::Any, lgpl));
        assert!(!ffi::license_allowed(VideoLibraries::LgplOnly, gpl));
        assert!(ffi::license_allowed(VideoLibraries::LgplOnly, lgpl));
        assert!(!ffi::license_allowed(
            VideoLibraries::LgplOnly,
            "nonfree and unredistributable"
        ));
    }
}
