//! `video`: moving pictures in a box. This part knows nothing of how they are decoded: that is up to
//! a [`VideoBackend`], which `anti-webui-video` is one of. See `docs/design/24-video.md`.

use crate::reactive::{Signal, signal};
use crate::tree::{Document, NodeId, with_doc};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Why a video can't be shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoError {
    /// There is nothing to decode with: no FFmpeg, one this doesn't know, one the settings don't
    /// allow, or a build without the feature `video`.
    Unavailable(String),
    /// Decoding works, but not for what this is.
    UnsupportedCodec(String),
    /// It can't be opened or read.
    Source(String),
    /// What is in it is broken.
    Decode(String),
}

impl std::fmt::Display for VideoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VideoError::Unavailable(e) => write!(f, "video is unavailable: {e}"),
            VideoError::UnsupportedCodec(e) => write!(f, "unsupported codec: {e}"),
            VideoError::Source(e) => write!(f, "can't read the video: {e}"),
            VideoError::Decode(e) => write!(f, "can't decode the video: {e}"),
        }
    }
}

impl std::error::Error for VideoError {}

/// Which FFmpeg may be used. See "ライセンス" in `docs/design/24-video.md`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VideoLibraries {
    /// Whatever is there.
    #[default]
    Any,
    /// Only one that says it is under the LGPL. Distributions tend to build theirs under the GPL,
    /// so this usually means bringing your own.
    LgplOnly,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct VideoSettings {
    pub libraries: VideoLibraries,
    /// Where to look for FFmpeg before the system. By default `ffmpeg/` next to the executable.
    pub library_dir: Option<PathBuf>,
    /// Never decode with the GPU.
    pub software_only: bool,
}

/// How the numbers of a frame become colors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[doc(hidden)]
pub enum YuvMatrix {
    Bt601,
    #[default]
    Bt709,
    Bt2020,
}

/// What the numbers of a frame mean as light.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[doc(hidden)]
pub enum VideoTransfer {
    /// As it is on any other screen.
    #[default]
    Sdr,
    Pq,
    Hlg,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[doc(hidden)]
pub struct VideoColor {
    pub matrix: YuvMatrix,
    /// Black is 0 rather than 16.
    pub full_range: bool,
    pub transfer: VideoTransfer,
}

/// What the texels of a plane are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[doc(hidden)]
pub enum PlaneFormat {
    R8,
    Rg8,
    /// In the high bits.
    R16,
    /// In the high bits.
    Rg16,
}

impl PlaneFormat {
    pub fn texel_bytes(self) -> u32 {
        match self {
            PlaneFormat::R8 => 1,
            PlaneFormat::Rg8 | PlaneFormat::R16 => 2,
            PlaneFormat::Rg16 => 4,
        }
    }
}

/// A plane in memory.
#[derive(Clone, Copy, Debug)]
#[doc(hidden)]
pub struct MemoryPlane {
    pub format: PlaneFormat,
    pub size: (u32, u32),
    pub stride: u32,
    data: *const u8,
}

// SAFETY: it only points at what the frame holds on to, which nothing writes to any more.
unsafe impl Send for MemoryPlane {}
// SAFETY: as above.
unsafe impl Sync for MemoryPlane {}

impl MemoryPlane {
    /// # Safety
    ///
    /// `data` must have `size.1` lines, `stride` apart, of `size.0` texels that stay as they are for
    /// as long as the frame this goes into is alive, which the frame's `keep` has to see to.
    pub unsafe fn new(format: PlaneFormat, size: (u32, u32), stride: u32, data: *const u8) -> Self {
        Self {
            format,
            size,
            stride,
            data,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        // The last line needn't have all of what the stride leaves room for.
        let row = self.size.0 * self.format.texel_bytes();
        let len = self.stride * self.size.1.saturating_sub(1) + row;
        // SAFETY: see `new`.
        unsafe { std::slice::from_raw_parts(self.data, len as usize) }
    }
}

/// A plane in a dmabuf, as the GPU decoder left it.
#[cfg(unix)]
#[derive(Clone, Copy, Debug)]
#[doc(hidden)]
pub struct DmabufPlane {
    pub format: PlaneFormat,
    pub size: (u32, u32),
    /// Stays open for as long as the frame is alive.
    pub fd: std::os::fd::RawFd,
    pub offset: u64,
    pub stride: u64,
    pub modifier: u64,
}

#[derive(Debug)]
#[doc(hidden)]
pub enum VideoPixels {
    /// One plane of luma and one or two of chroma.
    Memory(Vec<MemoryPlane>),
    #[cfg(unix)]
    Dmabuf(Vec<DmabufPlane>),
}

/// A picture of a video, for as long as something still shows it.
#[doc(hidden)]
pub struct VideoFrame {
    /// Tells it from all other frames.
    pub id: u64,
    /// The stream it is from. Pictures of a stream are about the same.
    pub stream: u64,
    /// While this is alive the stream is.
    pub alive: std::sync::Weak<()>,
    /// As it is stored, before rotating.
    pub size: (u32, u32),
    pub pixels: VideoPixels,
    pub color: VideoColor,
    /// Degrees clockwise, in steps of 90.
    pub rotation: u16,
    /// Holds on to what `pixels` is made of.
    pub keep: Box<dyn Any + Send + Sync>,
}

impl std::fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoFrame")
            .field("id", &self.id)
            .field("stream", &self.stream)
            .field("size", &self.size)
            .field("pixels", &self.pixels)
            .finish_non_exhaustive()
    }
}

/// What a stream has to say since the last time.
#[derive(Clone, Debug, PartialEq)]
#[doc(hidden)]
pub enum VideoEvent {
    /// How big it is, after rotating, in pixels, and how long, in seconds if that is known.
    Opened {
        size: (u32, u32),
        duration: Option<f64>,
    },
    /// The last frame has been on the screen for as long as it should.
    Ended,
    Failed(VideoError),
}

/// What it takes to open a stream.
#[derive(Clone)]
#[doc(hidden)]
pub struct VideoRequest {
    pub source: String,
    pub settings: VideoSettings,
    /// The GPU that draws, to decode on, if that can be shared with the decoder.
    pub render_node: Option<PathBuf>,
    /// Called from anywhere when there is news: see [`VideoStream::poll`].
    pub waker: Arc<dyn Fn() + Send + Sync>,
    pub playing: bool,
    pub looped: bool,
    pub muted: bool,
    pub volume: f32,
}

/// What is known about the refreshes of the screen, for showing frames evenly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[doc(hidden)]
pub struct VideoTiming {
    /// How long a refresh takes.
    pub refresh: Duration,
    /// When there was one, if that is known.
    pub vsync: Option<Instant>,
}

/// A video being played, as the UI thread sees it. All of this returns at once.
#[doc(hidden)]
pub trait VideoStream {
    fn poll(&mut self) -> Vec<VideoEvent>;
    /// What should be on the screen at the frame that starts at `now`.
    fn frame(&mut self, now: Instant, timing: Option<VideoTiming>) -> Option<Arc<VideoFrame>>;
    /// When [`VideoStream::frame`] says something else, if it will.
    fn next_change(&self, now: Instant, timing: Option<VideoTiming>) -> Option<Instant>;
    /// Where it is, in seconds.
    fn current_time(&self, now: Instant) -> f64;
    fn set_playing(&mut self, playing: bool);
    fn seek(&mut self, seconds: f64);
    fn set_looped(&mut self, looped: bool);
    fn set_muted(&mut self, muted: bool);
    fn set_volume(&mut self, volume: f32);
    /// The GPU can't read what the decoder leaves behind: frames are to come in memory from now on.
    fn use_memory(&mut self);
    /// How many frames are waiting to be shown, for tests.
    fn queued(&self) -> usize {
        0
    }
}

/// What makes streams.
#[doc(hidden)]
pub trait VideoBackend {
    fn open(&self, request: VideoRequest) -> Box<dyn VideoStream>;
    /// Whether there is something to decode with, and which.
    fn availability(&self, settings: &VideoSettings) -> Result<String, VideoError>;
    /// Whether a type, like `video/mp4; codecs=avc1`, looks like it can be played.
    fn can_play(&self, mime: &str, settings: &VideoSettings) -> bool;
}

thread_local! {
    static BACKEND: RefCell<Option<Rc<dyn VideoBackend>>> = const { RefCell::new(None) };
}

/// Says what decodes videos on this thread. `App` does this when it was built with the feature
/// `video`.
#[doc(hidden)]
pub fn set_video_backend(backend: Rc<dyn VideoBackend>) {
    BACKEND.with(|b| *b.borrow_mut() = Some(backend));
}

fn backend() -> Option<Rc<dyn VideoBackend>> {
    BACKEND.with(|b| b.borrow().clone())
}

const NO_BACKEND: &str = "built without the feature `video`";

/// Whether videos can be played at all, and with what.
pub fn video_availability() -> Result<String, VideoError> {
    let settings = crate::settings::settings().video;
    match backend() {
        Some(b) => b.availability(&settings),
        None => Err(VideoError::Unavailable(NO_BACKEND.into())),
    }
}

/// Like `canPlayType`: whether a type, like `video/mp4; codecs=avc1`, looks like it can be played.
pub fn can_play_video(mime: &str) -> bool {
    let settings = crate::settings::settings().video;
    backend().is_some_and(|b| b.can_play(mime, &settings))
}

/// How to play a [`video`](crate::view::video) and how far it has come. It can be copied into
/// closures like a signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoPlayer {
    pub(crate) playing: Signal<bool>,
    pub(crate) ended: Signal<bool>,
    pub(crate) time: Signal<f64>,
    pub(crate) duration: Signal<f64>,
    pub(crate) volume: Signal<f32>,
    pub(crate) muted: Signal<bool>,
    /// Where to go, and how many times that has been asked.
    pub(crate) seek: Signal<(f64, u64)>,
}

/// Something to give a [`video`](crate::view::video) so that it can be told what to do.
pub fn video_player() -> VideoPlayer {
    VideoPlayer {
        playing: signal(false),
        ended: signal(false),
        time: signal(0.0),
        duration: signal(f64::NAN),
        volume: signal(1.0),
        muted: signal(false),
        seek: signal((0.0, 0)),
    }
}

impl VideoPlayer {
    pub fn play(&self) {
        self.playing.set(true);
    }

    pub fn pause(&self) {
        self.playing.set(false);
    }

    /// Goes to a time, in seconds. What shows stays until there is something to show from there.
    pub fn seek(&self, seconds: f64) {
        self.seek.update(|s| *s = (seconds.max(0.0), s.1 + 1));
        self.time.set(seconds.max(0.0));
        self.ended.set(false);
    }

    pub fn set_volume(&self, volume: f32) {
        self.volume.set(volume.clamp(0.0, 1.0));
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.set(muted);
    }

    /// In seconds, four times a second while it plays.
    pub fn current_time(&self) -> f64 {
        self.time.get()
    }

    /// In seconds. `NaN` until it is known, and for what has no end.
    pub fn duration(&self) -> f64 {
        self.duration.get()
    }

    pub fn paused(&self) -> bool {
        !self.playing.get()
    }

    pub fn ended(&self) -> bool {
        self.ended.get()
    }

    pub fn volume(&self) -> f32 {
        self.volume.get()
    }

    pub fn muted(&self) -> bool {
        self.muted.get()
    }
}

/// Told how big a video is, in logical pixels, and how long, in seconds, or `NaN`.
#[doc(hidden)]
pub type LoadListener = Rc<dyn Fn(f32, f32, f64)>;
/// Told why a video can't be shown.
#[doc(hidden)]
pub type VideoErrorListener = Rc<dyn Fn(&VideoError)>;

/// What a `video` shows.
#[derive(Default)]
#[doc(hidden)]
pub struct VideoData {
    pub source: Option<String>,
    /// In pixels, once that is known.
    pub natural: Option<(f32, f32)>,
    pub player: Option<VideoPlayer>,
    pub looped: bool,
    pub on_load: Option<LoadListener>,
    pub on_ended: Option<Rc<dyn Fn()>>,
    pub on_error: Option<VideoErrorListener>,
}

struct Playing {
    stream: Box<dyn VideoStream>,
    /// The last thing it was asked to show, to tell whether that changed.
    shown: Option<u64>,
    /// It said it is done, or that it can't be.
    finished: bool,
    /// When [`VideoPlayer::current_time`] was last told.
    told: Instant,
}

/// How often a player hears of the time.
const TIME_UPDATE: Duration = Duration::from_millis(250);

/// The videos of a document.
#[derive(Default)]
#[doc(hidden)]
pub struct VideoStore {
    streams: HashMap<NodeId, Playing>,
    /// What to open at the next chance. The window comes after the tree, and what decodes wants to
    /// know the GPU that draws.
    pending: Vec<(NodeId, String)>,
    waker: Option<Arc<dyn Fn() + Send + Sync>>,
    timing: Option<VideoTiming>,
    render_node: Option<PathBuf>,
    memory_only: bool,
}

impl VideoStore {
    pub(crate) fn set_waker(&mut self, waker: Arc<dyn Fn() + Send + Sync>) {
        self.waker = Some(waker);
    }

    pub(crate) fn set_timing(&mut self, timing: Option<VideoTiming>) {
        self.timing = timing;
    }

    pub(crate) fn set_render_node(&mut self, node: Option<PathBuf>) {
        self.render_node = node;
    }

    fn close(&mut self, node: NodeId) {
        self.streams.remove(&node);
        self.pending.retain(|(id, _)| *id != node);
    }

    /// Whether any stream is yet to show something, for tests and screenshots.
    pub(crate) fn busy(&self) -> bool {
        !self.pending.is_empty()
            || self
                .streams
                .values()
                .any(|p| !p.finished && p.shown.is_none())
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.streams.is_empty() && self.pending.is_empty()
    }

    /// What a `video` shows now.
    pub(crate) fn frame(&mut self, node: NodeId, now: Instant) -> Option<Arc<VideoFrame>> {
        let timing = self.timing;
        let playing = self.streams.get_mut(&node)?;
        let frame = playing.stream.frame(now, timing)?;
        playing.shown = Some(frame.id);
        Some(frame)
    }

    /// When a frame has something else to show.
    pub(crate) fn next_frame(&self, now: Instant) -> Option<Instant> {
        self.streams
            .values()
            .filter_map(|p| p.stream.next_change(now, self.timing))
            .min()
    }

    /// The GPU can't read what the decoders leave behind.
    pub(crate) fn use_memory(&mut self) {
        self.memory_only = true;
        for p in self.streams.values_mut() {
            p.stream.use_memory();
        }
    }

    /// How many streams there are, and frames waiting to be shown, for tests.
    pub fn counts(&self) -> (usize, usize) {
        (
            self.streams.len(),
            self.streams.values().map(|p| p.stream.queued()).sum(),
        )
    }
}

/// A `video` is to show `source` now.
pub(crate) fn set_source(doc: &mut Document, id: NodeId, source: String) {
    let Some(data) = doc.nodes.get_mut(id).and_then(|n| n.video_mut()) else {
        return;
    };
    if data.source.as_deref() == Some(&source) {
        return;
    }
    data.source = Some(source.clone());
    let store = doc.videos.get_mut();
    store.close(id);
    store.pending.push((id, source));
}

/// Opens what is waiting to be.
fn open_pending(doc: &mut Document) {
    for (id, source) in std::mem::take(&mut doc.videos.get_mut().pending) {
        let Some(data) = doc.nodes.get(id).and_then(|n| n.video()) else {
            continue;
        };
        let (looped, player, on_error) = (data.looped, data.player, data.on_error.clone());
        let Some(backend) = backend() else {
            if let Some(f) = on_error {
                doc.videos_failed
                    .push((f, VideoError::Unavailable(NO_BACKEND.into())));
            }
            continue;
        };
        let store = doc.videos.get_mut();
        let waker = store
            .waker
            .clone()
            .unwrap_or_else(|| Arc::new(|| {}) as Arc<dyn Fn() + Send + Sync>);
        let mut settings = crate::reactive::untrack(|| crate::settings::settings().video);
        settings.software_only |= store.memory_only;
        let stream = backend.open(VideoRequest {
            source,
            settings,
            render_node: store.render_node.clone(),
            waker,
            playing: player.is_some_and(|p| p.playing.get_untracked()),
            looped,
            muted: player.is_some_and(|p| p.muted.get_untracked()),
            volume: player.map_or(1.0, |p| p.volume.get_untracked()),
        });
        store.streams.insert(
            id,
            Playing {
                stream,
                shown: None,
                finished: false,
                told: Instant::now(),
            },
        );
        doc.mark_shader_of(id);
    }
}

/// Every `video` is to paint again, for waiting for the first frames.
pub(crate) fn mark_all(doc: &mut Document) {
    let ids: Vec<NodeId> = doc.videos.get_mut().streams.keys().copied().collect();
    for id in ids {
        doc.mark_shader_of(id);
    }
}

/// The node is gone.
pub(crate) fn remove(doc: &mut Document, id: NodeId) {
    doc.videos.get_mut().close(id);
}

/// What the player says goes to the stream.
pub(crate) fn control(id: NodeId, f: impl FnOnce(&mut dyn VideoStream)) {
    with_doc(|d| {
        if let Some(p) = d.videos.get_mut().streams.get_mut(&id) {
            f(p.stream.as_mut());
        }
    });
}

/// Something to call once the document isn't borrowed any more.
pub(crate) type Tell = Box<dyn FnOnce()>;

/// Takes in what the streams have to say.
pub(crate) fn poll(doc: &mut Document, now: Instant) -> Vec<Tell> {
    open_pending(doc);
    let mut tells: Vec<Tell> = Vec::new();
    for (f, error) in std::mem::take(&mut doc.videos_failed) {
        tells.push(Box::new(move || f(&error)));
    }
    let ids: Vec<NodeId> = doc.videos.get_mut().streams.keys().copied().collect();
    for id in ids {
        let Some(playing) = doc.videos.get_mut().streams.get_mut(&id) else {
            continue;
        };
        let events = playing.stream.poll();
        let timing = doc.videos.get_mut().timing;
        let due = doc
            .videos
            .get_mut()
            .streams
            .get(&id)
            .and_then(|p| p.stream.next_change(now, timing))
            .is_some_and(|at| at <= now);
        if due {
            doc.mark_shader_of(id);
        }
        if events.is_empty() {
            continue;
        }
        let Some(data) = doc.nodes.get(id).and_then(|n| n.video()) else {
            continue;
        };
        let (player, on_load, on_ended, on_error) = (
            data.player,
            data.on_load.clone(),
            data.on_ended.clone(),
            data.on_error.clone(),
        );
        for event in events {
            match event {
                VideoEvent::Opened { size, duration } => {
                    let natural = (size.0 as f32, size.1 as f32);
                    if let Some(data) = doc.nodes.get_mut(id).and_then(|n| n.video_mut()) {
                        data.natural = Some(natural);
                    }
                    doc.invalidate_layout(id);
                    doc.mark_shader_of(id);
                    let seconds = duration.unwrap_or(f64::NAN);
                    if let Some(player) = player {
                        tells.push(Box::new(move || player.duration.set(seconds)));
                    }
                    if let Some(f) = on_load.clone() {
                        tells.push(Box::new(move || f(natural.0, natural.1, seconds)));
                    }
                }
                VideoEvent::Ended => {
                    if let Some(p) = doc.videos.get_mut().streams.get_mut(&id) {
                        p.finished = true;
                        let time = p.stream.current_time(now);
                        if let Some(player) = player {
                            tells.push(Box::new(move || {
                                player.time.set(time);
                                player.ended.set(true);
                                player.playing.set(false);
                            }));
                        }
                    }
                    if let Some(f) = on_ended.clone() {
                        tells.push(Box::new(move || f()));
                    }
                }
                VideoEvent::Failed(error) => {
                    if let Some(p) = doc.videos.get_mut().streams.get_mut(&id) {
                        p.finished = true;
                    }
                    if let Some(f) = on_error.clone() {
                        tells.push(Box::new(move || f(&error)));
                    }
                }
            }
        }
    }
    tells
}

/// Moves on what has come to its time: repaints those that show something else now, and tells
/// players the time.
pub(crate) fn advance(doc: &mut Document, now: Instant) -> Vec<Tell> {
    let mut tells: Vec<Tell> = Vec::new();
    let timing = doc.videos.get_mut().timing;
    let viewport = crate::geometry::RectI::new(0, 0, doc.viewport.width, doc.viewport.height);
    let ids: Vec<NodeId> = doc.videos.get_mut().streams.keys().copied().collect();
    for id in ids {
        let shown = doc.nodes.get(id).is_some_and(|n| n.visible)
            && doc
                .visible_window_rect(id)
                .is_some_and(|r| r.intersects(&viewport));
        let player = doc
            .nodes
            .get(id)
            .and_then(|n| n.video())
            .and_then(|d| d.player);
        let Some(playing) = doc.videos.get_mut().streams.get_mut(&id) else {
            continue;
        };
        let due = playing
            .stream
            .next_change(now, timing)
            .is_some_and(|at| at <= now);
        if let Some(player) = player
            && now.saturating_duration_since(playing.told) >= TIME_UPDATE
        {
            playing.told = now;
            if player.playing.get_untracked() {
                let time = playing.stream.current_time(now);
                tells.push(Box::new(move || player.time.set(time)));
            }
        }
        if due && shown {
            doc.mark_shader_of(id);
        }
    }
    tells
}

/// When there is a player to tell the time, or a frame to show.
pub(crate) fn next_deadline(doc: &Document, now: Instant) -> Option<Instant> {
    let store = doc.videos.borrow();
    let frame = store.next_frame(now);
    let time = store
        .streams
        .iter()
        .filter(|(id, _)| {
            doc.nodes
                .get(**id)
                .and_then(|n| n.video())
                .and_then(|d| d.player)
                .is_some_and(|p| p.playing.get_untracked())
        })
        .map(|(_, p)| p.told + TIME_UPDATE)
        .min();
    [frame, time].into_iter().flatten().min()
}
