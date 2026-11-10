//! A video being played: a thread that reads the file, one that decodes pictures, and one that
//! decodes sound and sends it out, whose progress is what everything goes by. The UI thread only
//! picks from what is there. See `docs/design/24-video.md`.

use crate::audio::Output;
use crate::cadence::Cadence;
use crate::ffi::*;
#[cfg(unix)]
use anti_webui_core::video::DmabufPlane;
use anti_webui_core::video::{
    MemoryPlane, PlaneFormat, VideoColor, VideoError, VideoEvent, VideoFrame, VideoPixels,
    VideoRequest, VideoStream, VideoTiming, VideoTransfer, YuvMatrix,
};
use std::collections::VecDeque;
use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Pictures decoded ahead. As in Chromium.
const MAX_FRAMES: usize = 4;
/// What the renderer can hold on to besides those, for the pool of a GPU decoder, which can't grow.
const EXTRA_HW_FRAMES: i64 = MAX_FRAMES as i64 + 4;
/// What may be read ahead, in bytes, of each kind.
const MAX_PACKET_BYTES: usize = 16 << 20;
/// ...and in any case, since the other kind may be far away in the file.
const LIMIT_PACKET_BYTES: usize = 256 << 20;
/// Threads for decoding on the CPU. What they take grows with their number. See the measurements in
/// the design document.
const MAX_THREADS: i64 = 4;
/// What a seek leaves a clock off by before it counts as having jumped, in seconds.
const CLOCK_JUMP: f64 = 0.05;
/// How far sound can be written ahead of the clock, in seconds. Devices hold less than this.
const MAX_AUDIO_AHEAD: f64 = 0.5;
/// Sound is written in pieces this long, in seconds, to react to being told something else.
const AUDIO_PIECE: f64 = 0.02;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static VAAPI_PIX_FMT: AtomicI32 = AtomicI32::new(-1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Where the video is. Follows the sound while there is any, and otherwise the time.
#[derive(Clone, Copy, Debug)]
struct Clock {
    base: f64,
    at: Instant,
    running: bool,
    /// It has been told where to start.
    started: bool,
}

impl Clock {
    fn new() -> Self {
        Self {
            base: 0.0,
            at: Instant::now(),
            running: false,
            started: false,
        }
    }

    fn at(&self, now: Instant) -> f64 {
        if self.running {
            self.base + now.saturating_duration_since(self.at).as_secs_f64()
        } else {
            self.base
        }
    }

    fn set(&mut self, media: f64, now: Instant) {
        self.base = media;
        self.at = now;
    }

    fn run(&mut self, running: bool, now: Instant) {
        self.base = self.at(now);
        self.at = now;
        self.running = running;
    }

    /// The sound says where it is. It comes in steps, so this only leans toward it, unless the
    /// clock is far ahead of what can be heard. Being far behind is no reason to jump: a device that
    /// says nothing is left to be heard may just not know.
    fn follow(&mut self, media: f64, now: Instant) {
        let off = media - self.at(now);
        if off > CLOCK_JUMP {
            return;
        }
        let to = if off < -CLOCK_JUMP {
            media
        } else {
            self.at(now) + off * 0.1
        };
        self.set(to, now);
    }
}

struct Timed {
    pts: f64,
    frame: Arc<VideoFrame>,
}

/// Something read for a decoder.
enum Item {
    Packet(Packet),
    /// What comes after this is from somewhere else. What ends before `target` isn't to show.
    Flush {
        target: f64,
    },
    End,
}

struct State {
    stop: bool,
    playing: bool,
    looped: bool,
    muted: bool,
    volume: f32,
    /// Where to go, for the thread that reads.
    seek: Option<f64>,
    memory_only: bool,
    clock: Clock,
    /// Goes up with each seek. What was decoded for an earlier one isn't to show.
    generation: u64,
    /// After this, frames are from the place that was sought.
    fresh: bool,
    frames: VecDeque<Timed>,
    /// How long a frame takes, as far as is known.
    frame_duration: f64,
    video: VecDeque<Item>,
    audio: VecDeque<Item>,
    video_bytes: usize,
    audio_bytes: usize,
    has_video: bool,
    /// There is sound and it goes somewhere.
    has_audio: bool,
    /// The picture thread is through with what it was given.
    video_done: bool,
    audio_done: bool,
    /// When the last picture ends.
    last_end: f64,
    ended: bool,
    events: Vec<VideoEvent>,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    /// For stopping FFmpeg while it waits for something to read.
    stop: AtomicBool,
    waker: Arc<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("no panics in here")
    }

    fn wait<'a>(&self, state: MutexGuard<'a, State>, timeout: Duration) -> MutexGuard<'a, State> {
        self.changed
            .wait_timeout(state, timeout)
            .expect("no panics in here")
            .0
    }

    fn fail(&self, error: VideoError) {
        self.lock().events.push(VideoEvent::Failed(error));
        (self.waker)();
    }
}

/// What the UI thread holds.
pub struct FfmpegStream {
    shared: Arc<Shared>,
    current: Option<Timed>,
    cadence: Cadence,
    /// What refreshes are counted from, when there is no telling where they are.
    epoch: Instant,
    /// What refreshes are counted from, when there is.
    grid: Option<Instant>,
}

impl FfmpegStream {
    pub fn open(request: VideoRequest) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                stop: false,
                playing: request.playing,
                looped: request.looped,
                muted: request.muted,
                volume: request.volume,
                seek: None,
                memory_only: request.settings.software_only,
                clock: Clock::new(),
                generation: 0,
                fresh: true,
                frames: VecDeque::new(),
                frame_duration: 0.0,
                video: VecDeque::new(),
                audio: VecDeque::new(),
                video_bytes: 0,
                audio_bytes: 0,
                has_video: false,
                has_audio: false,
                video_done: false,
                audio_done: false,
                last_end: 0.0,
                ended: false,
                events: Vec::new(),
            }),
            changed: Condvar::new(),
            stop: AtomicBool::new(false),
            waker: request.waker.clone(),
        });
        let thread = shared.clone();
        std::thread::Builder::new()
            .name("video reader".into())
            .spawn(move || read_thread(thread, request))
            .expect("threads can be made");
        Self {
            shared,
            current: None,
            cadence: Cadence::default(),
            epoch: Instant::now(),
            grid: None,
        }
    }

    /// Which refresh a frame made at `now` shows at, and when that is, counted the same way for the
    /// whole of the stream. The grid may wander along with the screen.
    ///
    /// A frame made during a refresh shows at the end of the next one: the compositor takes in what
    /// is there soon after a refresh starts.
    fn tick(&self, now: Instant, timing: Option<VideoTiming>) -> Option<(i64, Instant)> {
        let timing = timing?;
        let r = timing.refresh.as_secs_f64();
        if r <= 0.0 {
            return None;
        }
        let Some(vsync) = timing.vsync else {
            let tick = (now.saturating_duration_since(self.epoch).as_secs_f64() / r).round() as i64;
            return Some((tick, now + timing.refresh));
        };
        let since_base = self.grid.map_or(0.0, |base| seconds_between(base, vsync));
        let ahead = (seconds_between(vsync, now) / r).floor() + 2.0;
        let at = add_seconds(vsync, ahead * r);
        Some(((since_base / r).round() as i64 + ahead as i64, at))
    }

    /// When to make the frame for a refresh: in the middle of the refresh two before it. A frame made
    /// right when the compositor takes things in goes either way, by how fast the GPU is that time.
    fn make_at(&self, tick: i64, now: Instant, timing: VideoTiming) -> Option<Instant> {
        let r = timing.refresh.as_secs_f64();
        let (current, at) = self.tick(now, Some(timing))?;
        Some(add_seconds(at, ((tick - current) as f64 - 1.5) * r))
    }

    /// How far ahead of the clock a frame being made now should be.
    fn lag(&self, now: Instant, timing: Option<VideoTiming>) -> f64 {
        match self.tick(now, timing) {
            Some((_, at)) => at.saturating_duration_since(now).as_secs_f64(),
            None => 0.008,
        }
    }
}

/// `to` after `from` in seconds, which may be before.
fn seconds_between(from: Instant, to: Instant) -> f64 {
    match to.checked_duration_since(from) {
        Some(d) => d.as_secs_f64(),
        None => -from.saturating_duration_since(to).as_secs_f64(),
    }
}

fn add_seconds(at: Instant, seconds: f64) -> Instant {
    if seconds >= 0.0 {
        at + Duration::from_secs_f64(seconds)
    } else {
        at.checked_sub(Duration::from_secs_f64(-seconds))
            .unwrap_or(at)
    }
}

impl Drop for FfmpegStream {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.lock().stop = true;
        self.shared.changed.notify_all();
    }
}

impl VideoStream for FfmpegStream {
    fn poll(&mut self) -> Vec<VideoEvent> {
        std::mem::take(&mut self.shared.lock().events)
    }

    fn frame(&mut self, now: Instant, timing: Option<VideoTiming>) -> Option<Arc<VideoFrame>> {
        if self.grid.is_none() {
            self.grid = timing.and_then(|t| t.vsync);
        }
        let tick = self.tick(now, timing).map(|(tick, _)| tick);
        let lag = self.lag(now, timing);
        let shared = self.shared.clone();
        let mut state = shared.lock();
        // After seeking, what shows stays until there is something from the new place.
        if state.fresh
            && let Some(first) = state.frames.pop_front()
        {
            state.fresh = false;
            self.cadence.reset();
            self.current = Some(first);
            shared.changed.notify_all();
        }
        if !state.fresh && !state.frames.is_empty() {
            let media = state.clock.at(now) + lag;
            let next: Vec<f64> = state.frames.iter().map(|f| f.pts).collect();
            let by = self.cadence.advance(
                tick,
                media,
                timing.map(|t| t.refresh.as_secs_f64()),
                state.frame_duration,
                self.current.as_ref().map(|c| c.pts),
                &next,
            );
            if by > 0 {
                for _ in 1..by {
                    state.frames.pop_front();
                }
                self.current = state.frames.pop_front();
                shared.changed.notify_all();
            }
        }
        self.current.as_ref().map(|c| c.frame.clone())
    }

    fn next_change(&self, now: Instant, timing: Option<VideoTiming>) -> Option<Instant> {
        let state = self.shared.lock();
        let next = state.frames.front()?;
        if state.fresh || self.current.is_none() {
            return Some(now);
        }
        let lag = self.lag(now, timing);
        if !state.clock.running {
            return (state.clock.at(now) + lag >= next.pts).then_some(now);
        }
        let by_time = add_seconds(now, (next.pts - state.clock.at(now) - lag).max(0.0));
        let by_count = self
            .cadence
            .next_tick()
            .zip(timing)
            .and_then(|(tick, timing)| self.make_at(tick, now, timing));
        // What counting says may be held back by the clock, and that isn't worth waking up for.
        Some(match by_count {
            Some(at) if at > now || by_time <= now => at,
            _ => by_time,
        })
    }

    fn current_time(&self, now: Instant) -> f64 {
        let state = self.shared.lock();
        let time = state.clock.at(now).max(0.0);
        if state.ended { state.last_end } else { time }
    }

    fn set_playing(&mut self, playing: bool) {
        let mut state = self.shared.lock();
        if state.playing == playing {
            return;
        }
        state.playing = playing;
        // `play()` at the end starts over.
        if playing && state.ended {
            state.ended = false;
            state.seek = Some(0.0);
        }
        if state.clock.started {
            state.clock.run(playing, Instant::now());
        }
        drop(state);
        self.shared.changed.notify_all();
    }

    fn seek(&mut self, seconds: f64) {
        let mut state = self.shared.lock();
        state.seek = Some(seconds);
        state.ended = false;
        drop(state);
        self.shared.changed.notify_all();
    }

    fn set_looped(&mut self, looped: bool) {
        self.shared.lock().looped = looped;
    }

    fn set_muted(&mut self, muted: bool) {
        self.shared.lock().muted = muted;
    }

    fn set_volume(&mut self, volume: f32) {
        self.shared.lock().volume = volume;
    }

    fn use_memory(&mut self) {
        self.shared.lock().memory_only = true;
    }

    fn queued(&self) -> usize {
        self.shared.lock().frames.len()
    }
}

/// An `AVPacket`.
struct Packet {
    ffmpeg: Arc<Ffmpeg>,
    raw: *mut Opaque,
    size: usize,
}

// SAFETY: nothing else has it, and FFmpeg doesn't mind where it is freed.
unsafe impl Send for Packet {}

impl Drop for Packet {
    fn drop(&mut self) {
        // SAFETY: allocated by `av_packet_alloc`, and freed once.
        unsafe { (self.ffmpeg.f.av_packet_free)(&mut self.raw) };
    }
}

/// An `AVFormatContext` and what its streams are.
struct Input {
    ffmpeg: Arc<Ffmpeg>,
    raw: *mut Opaque,
    video: Option<StreamInfo>,
    audio: Option<StreamInfo>,
    /// Kept for as long as FFmpeg may call what asks about it.
    _stop: Arc<Shared>,
}

/// What a decoder needs to know of a stream.
#[derive(Clone, Copy)]
struct StreamInfo {
    index: c_int,
    par: *const Opaque,
    decoder: *const Opaque,
    time_base: f64,
    rotation: u16,
}

// SAFETY: these belong to the `Input`, which outlives the threads they go to, and aren't changed.
unsafe impl Send for StreamInfo {}

impl Drop for Input {
    fn drop(&mut self) {
        // SAFETY: opened by `avformat_open_input`, and closed once.
        unsafe { (self.ffmpeg.f.avformat_close_input)(&mut self.raw) };
    }
}

unsafe extern "C" fn interrupted(opaque: *mut c_void) -> c_int {
    // SAFETY: this is what `open_input` gave, which the `Input` holds on to.
    let shared = unsafe { &*opaque.cast::<Shared>() };
    c_int::from(shared.stop.load(Ordering::Relaxed))
}

fn open_input(
    ffmpeg: &Arc<Ffmpeg>,
    shared: &Arc<Shared>,
    source: &str,
) -> Result<Input, VideoError> {
    let f = &ffmpeg.f;
    let l = ffmpeg.layout;
    let name = c_string(source)?;
    // SAFETY: what comes back is checked, and the fields are where the layout of this version says.
    unsafe {
        let mut raw = (f.avformat_alloc_context)();
        if raw.is_null() {
            return Err(VideoError::Source("out of memory".into()));
        }
        write::<Interrupt>(raw, l.format_interrupt, interrupted);
        write::<*const Shared>(
            raw,
            l.format_interrupt + size_of::<Interrupt>(),
            Arc::as_ptr(shared),
        );
        let code = (f.avformat_open_input)(
            &mut raw,
            name.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
        );
        if code < 0 {
            return Err(VideoError::Source(format!(
                "{source}: {}",
                ffmpeg.error(code)
            )));
        }
        let mut input = Input {
            ffmpeg: ffmpeg.clone(),
            raw,
            video: None,
            audio: None,
            _stop: shared.clone(),
        };
        let code = (f.avformat_find_stream_info)(raw, std::ptr::null_mut());
        if code < 0 {
            return Err(VideoError::Source(format!(
                "{source}: {}",
                ffmpeg.error(code)
            )));
        }
        input.video = stream_info(ffmpeg, raw, AVMEDIA_TYPE_VIDEO);
        input.audio = stream_info(ffmpeg, raw, AVMEDIA_TYPE_AUDIO);
        if input.video.is_none() && input.audio.is_none() {
            return Err(VideoError::UnsupportedCodec(format!(
                "{source}: no decoder for anything in it"
            )));
        }
        Ok(input)
    }
}

/// # Safety
///
/// `format` has been opened.
unsafe fn stream_info(ffmpeg: &Ffmpeg, format: *mut Opaque, kind: c_int) -> Option<StreamInfo> {
    let (f, l) = (&ffmpeg.f, ffmpeg.layout);
    let mut decoder = std::ptr::null();
    // SAFETY: see above; the layout says where the fields are.
    unsafe {
        let index = (f.av_find_best_stream)(format, kind, -1, -1, &mut decoder, 0);
        if index < 0 || decoder.is_null() {
            return None;
        }
        let streams: *const *const Opaque = read(format, l.format_streams);
        let stream = *streams.add(index as usize);
        let par: *const Opaque = read(stream, l.stream_codecpar);
        let time_base: Rational = read(stream, l.stream_time_base);
        let rotation = if kind == AVMEDIA_TYPE_VIDEO {
            rotation_of(ffmpeg, par)
        } else {
            0
        };
        Some(StreamInfo {
            index,
            par,
            decoder,
            time_base: time_base.as_f64(),
            rotation,
        })
    }
}

/// How far to turn the picture, clockwise.
///
/// # Safety
///
/// `par` is the `AVCodecParameters` of a stream.
unsafe fn rotation_of(ffmpeg: &Ffmpeg, par: *const Opaque) -> u16 {
    let (f, l) = (&ffmpeg.f, ffmpeg.layout);
    // SAFETY: see above; the layout says where the fields are.
    unsafe {
        let side: *const Opaque = read(par, l.par_side_data);
        let count: c_int = read(par, l.par_nb_side_data);
        let found = (f.av_packet_side_data_get)(side, count, AV_PKT_DATA_DISPLAYMATRIX);
        if found.is_null() {
            return 0;
        }
        let size: usize = read(found, l.side_data_size);
        let matrix: *const i32 = read(found, l.side_data_data);
        if size < 36 || matrix.is_null() {
            return 0;
        }
        // This says how far to turn it counterclockwise.
        let degrees = (f.av_display_rotation_get)(matrix);
        if degrees.is_nan() {
            return 0;
        }
        (((-degrees / 90.0).round() as i64 * 90).rem_euclid(360)) as u16
    }
}

fn read_thread(shared: Arc<Shared>, request: VideoRequest) {
    let ffmpeg = match ffmpeg(&request.settings) {
        Ok(ffmpeg) => ffmpeg,
        Err(e) => return shared.fail(e),
    };
    VAAPI_PIX_FMT.store(ffmpeg.vaapi_pix_fmt, Ordering::Relaxed);
    let input = match open_input(&ffmpeg, &shared, &request.source) {
        Ok(input) => input,
        Err(e) => return shared.fail(e),
    };
    let (f, l) = (&ffmpeg.f, ffmpeg.layout);
    // SAFETY: the input is open, and the layout says where the fields are.
    let (start, duration) = unsafe {
        let start: i64 = read(input.raw, l.format_start_time);
        let duration: i64 = read(input.raw, l.format_duration);
        (
            if start == AV_NOPTS_VALUE {
                0.0
            } else {
                start as f64 / AV_TIME_BASE
            },
            (duration != AV_NOPTS_VALUE && duration > 0).then(|| duration as f64 / AV_TIME_BASE),
        )
    };
    let size = input.video.map_or((0, 0), |v| {
        // SAFETY: as above.
        let (w, h): (c_int, c_int) =
            unsafe { (read(v.par, l.par_width), read(v.par, l.par_height)) };
        if v.rotation % 180 == 0 {
            (w as u32, h as u32)
        } else {
            (h as u32, w as u32)
        }
    });
    {
        let mut state = shared.lock();
        state.has_video = input.video.is_some();
        state.has_audio = input.audio.is_some();
        state.video_done = input.video.is_none();
        state.audio_done = input.audio.is_none();
        state.events.push(VideoEvent::Opened { size, duration });
        if input.video.is_none() {
            state.clock.started = true;
            let playing = state.playing;
            state.clock.run(playing, Instant::now());
        }
    }
    (shared.waker)();

    let mut threads = Vec::new();
    if let Some(info) = input.video {
        let (shared, ffmpeg, node) = (shared.clone(), ffmpeg.clone(), request.render_node.clone());
        threads.push(
            std::thread::Builder::new()
                .name("video decoder".into())
                .spawn(move || video_thread(shared, ffmpeg, info, node))
                .expect("threads can be made"),
        );
    }
    if let Some(info) = input.audio {
        let (shared, ffmpeg) = (shared.clone(), ffmpeg.clone());
        threads.push(
            std::thread::Builder::new()
                .name("video sound".into())
                .spawn(move || audio_thread(shared, ffmpeg, info))
                .expect("threads can be made"),
        );
    }

    let video_index = input.video.map_or(-1, |v| v.index);
    let audio_index = input.audio.map_or(-1, |a| a.index);
    let mut read_all = false;
    let mut pending: Option<Packet> = None;
    loop {
        let mut state = shared.lock();
        if state.stop {
            break;
        }
        if let Some(target) = state.seek.take() {
            drop(state);
            pending = None;
            // SAFETY: the input is open.
            let code = unsafe {
                (f.av_seek_frame)(
                    input.raw,
                    -1,
                    ((target + start) * AV_TIME_BASE) as i64,
                    AVSEEK_FLAG_BACKWARD,
                )
            };
            if code < 0 {
                shared.fail(VideoError::Source(format!(
                    "can't go to {target} s: {}",
                    ffmpeg.error(code)
                )));
            }
            read_all = false;
            let mut state = shared.lock();
            state.generation += 1;
            state.video.clear();
            state.audio.clear();
            state.video_bytes = 0;
            state.audio_bytes = 0;
            state.frames.clear();
            state.fresh = true;
            state.ended = false;
            state.video_done = !state.has_video;
            state.audio_done = !state.has_audio;
            state.last_end = 0.0;
            state.clock.started = !state.has_video;
            state.clock.set(target, Instant::now());
            state.clock.running = !state.has_video && state.playing;
            state.video.push_back(Item::Flush { target });
            state.audio.push_back(Item::Flush { target });
            drop(state);
            shared.changed.notify_all();
            continue;
        }
        if read_all {
            // At the end there is only to wait for it to be over.
            let now = Instant::now();
            // Frames may be left when nobody is looking, which doesn't make it any less over.
            let over = state.video_done
                && state.audio_done
                && state.clock.started
                && state.clock.at(now) >= state.last_end;
            if over && !state.ended {
                if state.looped {
                    state.seek = Some(0.0);
                    continue;
                }
                state.ended = true;
                state.playing = false;
                state.clock.run(false, now);
                state.events.push(VideoEvent::Ended);
                drop(state);
                (shared.waker)();
                continue;
            }
            drop(shared.wait(state, Duration::from_millis(10)));
            continue;
        }
        if let Some(packet) = pending.take() {
            let to_video = packet_stream(&packet, l) == video_index;
            let (bytes, other) = if to_video {
                (state.video_bytes, state.audio.is_empty())
            } else {
                (state.audio_bytes, state.video.is_empty())
            };
            // With the other side starving, what it needs must be further ahead.
            if bytes >= MAX_PACKET_BYTES && !(other && bytes < LIMIT_PACKET_BYTES) {
                pending = Some(packet);
                drop(shared.wait(state, Duration::from_millis(50)));
                continue;
            }
            if to_video {
                state.video_bytes += packet.size;
                state.video.push_back(Item::Packet(packet));
            } else {
                state.audio_bytes += packet.size;
                state.audio.push_back(Item::Packet(packet));
            }
            drop(state);
            shared.changed.notify_all();
            continue;
        }
        drop(state);
        // SAFETY: nothing to it.
        let raw = unsafe { (f.av_packet_alloc)() };
        if raw.is_null() {
            shared.fail(VideoError::Source("out of memory".into()));
            break;
        }
        let packet = Packet {
            ffmpeg: ffmpeg.clone(),
            raw,
            size: 0,
        };
        // SAFETY: the input is open and the packet is empty.
        let code = unsafe { (f.av_read_frame)(input.raw, packet.raw) };
        if code < 0 {
            let mut state = shared.lock();
            if state.stop {
                break;
            }
            if code != AVERROR_EOF {
                state
                    .events
                    .push(VideoEvent::Failed(VideoError::Source(ffmpeg.error(code))));
                drop(state);
                (shared.waker)();
                state = shared.lock();
            }
            state.video.push_back(Item::End);
            state.audio.push_back(Item::End);
            read_all = true;
            drop(state);
            shared.changed.notify_all();
            continue;
        }
        let index = packet_stream(&packet, l);
        if index == video_index || index == audio_index {
            // SAFETY: a packet that has been read into.
            let size: c_int = unsafe { read(packet.raw, l.packet_size) };
            let mut packet = packet;
            packet.size = size.max(0) as usize;
            pending = Some(packet);
        }
    }
    {
        // The decoders stop after this, and only then can what they read from go away.
        shared.lock().stop = true;
        shared.changed.notify_all();
    }
    for thread in threads {
        let _ = thread.join();
    }
    drop(input);
}

fn packet_stream(packet: &Packet, l: &Layout) -> c_int {
    // SAFETY: a packet that has been read into.
    unsafe { read(packet.raw, l.packet_stream_index) }
}

/// An `AVCodecContext`.
struct Decoder {
    ffmpeg: Arc<Ffmpeg>,
    raw: *mut Opaque,
    hardware: bool,
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: allocated by `avcodec_alloc_context3`, and freed once.
        unsafe { (self.ffmpeg.f.avcodec_free_context)(&mut self.raw) };
    }
}

unsafe extern "C" fn pick_format(_: *mut Opaque, formats: *const c_int) -> c_int {
    let vaapi = VAAPI_PIX_FMT.load(Ordering::Relaxed);
    let mut last = -1;
    let mut i = 0;
    loop {
        // SAFETY: FFmpeg gives a list that ends in -1.
        let format = unsafe { *formats.add(i) };
        if format == -1 {
            // The last one is what it can do on its own.
            return last;
        }
        if format == vaapi {
            return vaapi;
        }
        last = format;
        i += 1;
    }
}

fn open_decoder(
    ffmpeg: &Arc<Ffmpeg>,
    info: &StreamInfo,
    decoder: *const Opaque,
    device: Option<*mut Opaque>,
    threads: i64,
) -> Result<Decoder, VideoError> {
    let (f, l) = (&ffmpeg.f, ffmpeg.layout);
    // SAFETY: the decoder and the parameters are valid, and the layout says where the fields are.
    unsafe {
        let raw = (f.avcodec_alloc_context3)(decoder);
        if raw.is_null() {
            return Err(VideoError::Decode("out of memory".into()));
        }
        let this = Decoder {
            ffmpeg: ffmpeg.clone(),
            raw,
            hardware: device.is_some(),
        };
        let code = (f.avcodec_parameters_to_context)(raw, info.par);
        if code < 0 {
            return Err(VideoError::Decode(ffmpeg.error(code)));
        }
        ffmpeg.set_option(raw, c"threads", threads);
        if let Some(device) = device {
            write::<*mut Opaque>(raw, l.codec_hw_device, (f.av_buffer_ref)(device));
            write::<GetFormat>(raw, l.codec_get_format, pick_format);
            ffmpeg.set_option(raw, c"extra_hw_frames", EXTRA_HW_FRAMES);
        }
        let code = (f.avcodec_open2)(raw, decoder, std::ptr::null_mut());
        if code < 0 {
            return Err(VideoError::Decode(ffmpeg.error(code)));
        }
        Ok(this)
    }
}

/// A decoder on the GPU, if there is one that can do this.
fn hardware_decoder(
    ffmpeg: &Arc<Ffmpeg>,
    info: &StreamInfo,
    node: &std::path::Path,
) -> Option<Decoder> {
    let (f, l) = (&ffmpeg.f, ffmpeg.layout);
    let node = c_string(node.to_str()?).ok()?;
    // SAFETY: what comes back is checked, and the layout says where the fields are.
    unsafe {
        let codec_id: c_int = read(info.par, l.par_codec_id);
        // What is best on the CPU may not be what drives the GPU, as with `libdav1d`.
        let decoder = (f.avcodec_find_decoder_by_name)((f.avcodec_get_name)(codec_id));
        let decoder = if decoder.is_null() {
            (f.avcodec_find_decoder)(codec_id)
        } else {
            decoder
        };
        if decoder.is_null() {
            return None;
        }
        let mut supported = false;
        for i in 0.. {
            let config = (f.avcodec_get_hw_config)(decoder, i);
            if config.is_null() {
                break;
            }
            let c = &*config;
            supported |= c.device_type == AV_HWDEVICE_TYPE_VAAPI
                && c.methods & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX != 0;
        }
        if !supported {
            return None;
        }
        let mut device = std::ptr::null_mut();
        let code = (f.av_hwdevice_ctx_create)(
            &mut device,
            AV_HWDEVICE_TYPE_VAAPI,
            node.as_ptr(),
            std::ptr::null_mut(),
            0,
        );
        if code < 0 {
            return None;
        }
        let opened = open_decoder(ffmpeg, info, decoder, Some(device), 1);
        (f.av_buffer_unref)(&mut device);
        opened.ok()
    }
}

/// The number of threads to decode a picture with.
fn thread_count() -> i64 {
    std::thread::available_parallelism()
        .map_or(1, |n| n.get() as i64)
        .min(MAX_THREADS)
}

/// Waits for something to decode. `None` when it's time to stop.
fn next_item(shared: &Shared, video: bool) -> Option<Item> {
    let mut state = shared.lock();
    loop {
        if state.stop {
            return None;
        }
        let item = if video {
            state.video.pop_front()
        } else {
            state.audio.pop_front()
        };
        if let Some(item) = item {
            if let Item::Packet(p) = &item {
                if video {
                    state.video_bytes -= p.size;
                } else {
                    state.audio_bytes -= p.size;
                }
            }
            drop(state);
            shared.changed.notify_all();
            return Some(item);
        }
        state = shared.wait(state, Duration::from_millis(100));
    }
}

/// What a frame holds on to.
struct Keep {
    ffmpeg: Arc<Ffmpeg>,
    frames: Vec<*mut Opaque>,
    /// What it was converted into.
    _buffer: Vec<u8>,
}

// SAFETY: nothing writes to these frames any more, and they can be freed from anywhere.
unsafe impl Send for Keep {}
// SAFETY: as above.
unsafe impl Sync for Keep {}

impl Drop for Keep {
    fn drop(&mut self) {
        for frame in &mut self.frames {
            // SAFETY: from `av_frame_alloc`, and freed once.
            unsafe { (self.ffmpeg.f.av_frame_free)(frame) };
        }
    }
}

fn alloc_frame(ffmpeg: &Ffmpeg) -> *mut Opaque {
    // SAFETY: nothing to it.
    unsafe { (ffmpeg.f.av_frame_alloc)() }
}

/// A conversion to what the renderer can take.
struct Scaler {
    ffmpeg: Arc<Ffmpeg>,
    raw: *mut Opaque,
    from: (c_int, c_int, c_int),
    to: c_int,
}

impl Drop for Scaler {
    fn drop(&mut self) {
        // SAFETY: from `sws_getContext`, and freed once.
        unsafe { (self.ffmpeg.f.sws_freeContext)(self.raw) };
    }
}

/// How the pixels of a format are to be taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Nv12,
    I420,
    P010,
    /// Needs converting, keeping more than 8 bits or not.
    Other {
        deep: bool,
    },
}

fn shape_of(name: &str) -> Shape {
    match name {
        "nv12" => Shape::Nv12,
        "yuv420p" | "yuvj420p" => Shape::I420,
        "p010le" => Shape::P010,
        _ => Shape::Other {
            deep: ["9", "10", "12", "14", "16"]
                .iter()
                .any(|d| name.contains(&format!("p{d}")) || name.contains(&format!("{d}le"))),
        },
    }
}

/// The color a frame says it is.
///
/// # Safety
///
/// `frame` holds a decoded picture.
unsafe fn color_of(ffmpeg: &Ffmpeg, frame: *const Opaque, name: &str, height: u32) -> VideoColor {
    let l = ffmpeg.layout;
    // SAFETY: see above; the layout says where the fields are.
    let (space, range, trc): (c_int, c_int, c_int) = unsafe {
        (
            read(frame, l.frame_colorspace),
            read(frame, l.frame_color_range),
            read(frame, l.frame_color_trc),
        )
    };
    VideoColor {
        matrix: match space {
            AVCOL_SPC_BT709 => YuvMatrix::Bt709,
            AVCOL_SPC_BT470BG | AVCOL_SPC_SMPTE170M => YuvMatrix::Bt601,
            AVCOL_SPC_BT2020_NCL | AVCOL_SPC_BT2020_CL => YuvMatrix::Bt2020,
            // As in Chromium: small is from the old days.
            _ if height < 720 => YuvMatrix::Bt601,
            _ => YuvMatrix::Bt709,
        },
        full_range: range == AVCOL_RANGE_JPEG || name.starts_with("yuvj"),
        transfer: match trc {
            AVCOL_TRC_SMPTE2084 => VideoTransfer::Pq,
            AVCOL_TRC_ARIB_STD_B67 => VideoTransfer::Hlg,
            _ => VideoTransfer::Sdr,
        },
    }
}

fn memory_frame(
    ffmpeg: &Arc<Ffmpeg>,
    frame: *mut Opaque,
    scaler: &mut Option<Scaler>,
    mut held: Vec<*mut Opaque>,
) -> Option<(VideoPixels, Keep)> {
    let (f, l) = (&ffmpeg.f, ffmpeg.layout);
    // SAFETY: `frame` holds a picture in memory, and the layout says where the fields are.
    unsafe {
        let format: c_int = read(frame, l.frame_format);
        let (w, h): (c_int, c_int) = (read(frame, l.frame_width), read(frame, l.frame_height));
        let data: [*const u8; 8] = read(frame, l.frame_data);
        let lines: [c_int; 8] = read(frame, l.frame_linesize);
        let shape = shape_of(&ffmpeg.pix_fmt_name(format));
        let (w, h) = (w as u32, h as u32);
        let half = (w.div_ceil(2), h.div_ceil(2));
        let mut buffer = Vec::new();
        let planes = match shape {
            Shape::Nv12 => vec![
                MemoryPlane::new(PlaneFormat::R8, (w, h), lines[0] as u32, data[0]),
                MemoryPlane::new(PlaneFormat::Rg8, half, lines[1] as u32, data[1]),
            ],
            Shape::P010 => vec![
                MemoryPlane::new(PlaneFormat::R16, (w, h), lines[0] as u32, data[0]),
                MemoryPlane::new(PlaneFormat::Rg16, half, lines[1] as u32, data[1]),
            ],
            Shape::I420 => vec![
                MemoryPlane::new(PlaneFormat::R8, (w, h), lines[0] as u32, data[0]),
                MemoryPlane::new(PlaneFormat::R8, half, lines[1] as u32, data[1]),
                MemoryPlane::new(PlaneFormat::R8, half, lines[2] as u32, data[2]),
            ],
            Shape::Other { deep } => {
                let to = if deep {
                    ffmpeg.p010_pix_fmt
                } else {
                    ffmpeg.nv12_pix_fmt
                };
                let from = (format, w as c_int, h as c_int);
                if scaler.as_ref().is_none_or(|s| s.from != from || s.to != to) {
                    let raw = (f.sws_getContext)(
                        from.1,
                        from.2,
                        from.0,
                        from.1,
                        from.2,
                        to,
                        SWS_BILINEAR,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null(),
                    );
                    if raw.is_null() {
                        return None;
                    }
                    *scaler = Some(Scaler {
                        ffmpeg: ffmpeg.clone(),
                        raw,
                        from,
                        to,
                    });
                }
                let texel = if deep { 2 } else { 1 };
                let stride = (w as usize * texel + 63) & !63;
                let chroma_stride = ((half.0 as usize * 2 * texel) + 63) & !63;
                let luma = stride * h as usize;
                buffer = vec![0u8; luma + chroma_stride * half.1 as usize];
                let base = buffer.as_mut_ptr();
                let out = [base, base.add(luma)];
                let out_lines = [stride as c_int, chroma_stride as c_int];
                (f.sws_scale)(
                    scaler.as_ref().expect("just made").raw,
                    data.as_ptr(),
                    lines.as_ptr(),
                    0,
                    h as c_int,
                    out.as_ptr(),
                    out_lines.as_ptr(),
                );
                let (y, uv) = if deep {
                    (PlaneFormat::R16, PlaneFormat::Rg16)
                } else {
                    (PlaneFormat::R8, PlaneFormat::Rg8)
                };
                vec![
                    MemoryPlane::new(y, (w, h), stride as u32, out[0]),
                    MemoryPlane::new(uv, half, chroma_stride as u32, out[1]),
                ]
            }
        };
        if shape != (Shape::Other { deep: false }) && shape != (Shape::Other { deep: true }) {
            held.push(frame);
        } else {
            let mut frame = frame;
            (f.av_frame_free)(&mut frame);
        }
        let keep = Keep {
            ffmpeg: ffmpeg.clone(),
            frames: held,
            _buffer: buffer,
        };
        Some((VideoPixels::Memory(planes), keep))
    }
}

/// Where there are no dmabufs, pictures on the GPU are read back to memory.
#[cfg(not(unix))]
fn dmabuf_frame(_ffmpeg: &Arc<Ffmpeg>, _frame: *mut Opaque) -> Option<(VideoPixels, Keep)> {
    None
}

/// The dmabufs of a picture on the GPU.
#[cfg(unix)]
fn dmabuf_frame(ffmpeg: &Arc<Ffmpeg>, frame: *mut Opaque) -> Option<(VideoPixels, Keep)> {
    let (f, l) = (&ffmpeg.f, ffmpeg.layout);
    // SAFETY: `frame` is a picture on the GPU, and the layout says where the fields are.
    unsafe {
        let mapped = alloc_frame(ffmpeg);
        write::<c_int>(mapped, l.frame_format, ffmpeg.drm_prime_pix_fmt);
        let code = (f.av_hwframe_map)(mapped, frame, AV_HWFRAME_MAP_READ);
        let keep = Keep {
            ffmpeg: ffmpeg.clone(),
            frames: vec![mapped],
            _buffer: Vec::new(),
        };
        if code < 0 {
            return None;
        }
        let data: [*const u8; 8] = read(mapped, l.frame_data);
        let descriptor = &*data[0].cast::<DrmDescriptor>();
        let (w, h): (c_int, c_int) = (read(frame, l.frame_width), read(frame, l.frame_height));
        let (w, h) = (w as u32, h as u32);
        let half = (w.div_ceil(2), h.div_ceil(2));
        // Two layers of one plane each is what can be read as two textures.
        if descriptor.nb_layers != 2 {
            return None;
        }
        let mut planes = Vec::with_capacity(2);
        for (i, layer) in descriptor.layers[..2].iter().enumerate() {
            if layer.nb_planes != 1 {
                return None;
            }
            // The fourcc of what each layer holds.
            let format = match &layer.format.to_le_bytes() {
                b"R8  " => PlaneFormat::R8,
                b"GR88" => PlaneFormat::Rg8,
                b"R16 " => PlaneFormat::R16,
                b"GR32" => PlaneFormat::Rg16,
                _ => return None,
            };
            let plane = &layer.planes[0];
            let object = &descriptor.objects[plane.object_index as usize];
            planes.push(DmabufPlane {
                format,
                size: if i == 0 { (w, h) } else { half },
                fd: object.fd,
                offset: plane.offset as u64,
                stride: plane.pitch as u64,
                modifier: object.format_modifier,
            });
        }
        Some((VideoPixels::Dmabuf(planes), keep))
    }
}

/// What the thread that decodes pictures keeps track of.
struct Pictures {
    shared: Arc<Shared>,
    ffmpeg: Arc<Ffmpeg>,
    info: StreamInfo,
    decoder: Decoder,
    /// Tells the frames of this stream from others.
    stream: u64,
    alive: Arc<()>,
    scaler: Option<Scaler>,
    /// What isn't to show after a seek ends before this.
    target: f64,
    generation: u64,
    last_pts: f64,
    duration: f64,
    decoded_any: bool,
    failures: u32,
    /// All that went in before the first picture came out, in case the GPU can't do this after all.
    kept: Vec<Packet>,
}

impl Pictures {
    /// Takes what is ready out of the decoder. Returns the error if there is a real one.
    fn drain(&mut self) -> Result<(), c_int> {
        loop {
            let mut frame = alloc_frame(&self.ffmpeg);
            // SAFETY: an open decoder and an empty frame.
            let code = unsafe { (self.ffmpeg.f.avcodec_receive_frame)(self.decoder.raw, frame) };
            if code < 0 {
                // SAFETY: from `av_frame_alloc`.
                unsafe { (self.ffmpeg.f.av_frame_free)(&mut frame) };
                return if code == AVERROR_EAGAIN || code == AVERROR_EOF {
                    Ok(())
                } else {
                    Err(code)
                };
            }
            self.decoded_any = true;
            self.failures = 0;
            self.kept.clear();
            self.deliver(frame);
        }
    }

    /// Gives the decoder a packet, or at the end nothing. Returns the error if there is a real one.
    fn send(&mut self, packet: *const Opaque) -> Result<(), c_int> {
        loop {
            // SAFETY: an open decoder, and a packet or nothing at the end.
            let code = unsafe { (self.ffmpeg.f.avcodec_send_packet)(self.decoder.raw, packet) };
            self.drain()?;
            if code == AVERROR_EAGAIN {
                continue;
            }
            return if code < 0 && code != AVERROR_EOF {
                Err(code)
            } else {
                Ok(())
            };
        }
    }

    /// Goes on with the CPU. Returns whether that can be done.
    fn fall_back(&mut self) -> bool {
        if !self.decoder.hardware || self.decoded_any {
            return false;
        }
        let Ok(decoder) = open_decoder(
            &self.ffmpeg,
            &self.info,
            self.info.decoder,
            None,
            thread_count(),
        ) else {
            return false;
        };
        self.decoder = decoder;
        for packet in std::mem::take(&mut self.kept) {
            if self.send(packet.raw).is_ok() && !self.decoded_any {
                self.kept.push(packet);
            }
        }
        true
    }

    /// Returns false when there is no use in going on.
    fn feed(&mut self, packet: Option<Packet>) -> bool {
        let raw = packet
            .as_ref()
            .map_or(std::ptr::null(), |p| p.raw.cast_const());
        if let Err(code) = self.send(raw) {
            if self.fall_back() {
                return self.feed(packet);
            }
            self.failures += 1;
            // A picture that can't be decoded is skipped, but not forever.
            if !self.decoded_any && self.failures > 32 {
                self.shared
                    .fail(VideoError::Decode(self.ffmpeg.error(code)));
                return false;
            }
        }
        if !self.decoded_any && self.decoder.hardware {
            self.kept.extend(packet);
        }
        true
    }

    /// Passes a decoded picture on, when there is room for it.
    fn deliver(&mut self, frame: *mut Opaque) {
        let ffmpeg = self.ffmpeg.clone();
        let (f, l) = (&ffmpeg.f, ffmpeg.layout);
        let tb = self.info.time_base;
        // SAFETY: a frame just decoded, and the layout says where the fields are.
        let (pts, length, format, w, h) = unsafe {
            let pts: i64 = read(frame, l.frame_best_effort);
            let length: i64 = read(frame, l.frame_duration);
            let format: c_int = read(frame, l.frame_format);
            let w: c_int = read(frame, l.frame_width);
            let h: c_int = read(frame, l.frame_height);
            (pts, length, format, w as u32, h as u32)
        };
        let pts = if pts == AV_NOPTS_VALUE {
            self.last_pts + self.duration
        } else {
            pts as f64 * tb
        };
        let seen = pts - self.last_pts;
        self.duration = if length > 0 {
            length as f64 * tb
        } else if seen.is_finite() && seen > 0.0 {
            seen
        } else {
            self.duration
        };
        self.last_pts = pts;
        let end = pts + self.duration;
        let mut frame = frame;
        if end <= self.target {
            // SAFETY: from `av_frame_alloc`.
            unsafe { (f.av_frame_free)(&mut frame) };
            return;
        }
        let name = ffmpeg.pix_fmt_name(format);
        // SAFETY: a frame just decoded.
        let color = unsafe { color_of(&ffmpeg, frame, &name, h) };
        let Some((pixels, keep)) = self.pixels(frame, format) else {
            return;
        };
        let video_frame = Arc::new(VideoFrame {
            id: next_id(),
            stream: self.stream,
            alive: Arc::downgrade(&self.alive),
            size: (w, h),
            pixels,
            color,
            rotation: self.info.rotation,
            keep: Box::new(keep),
        });
        let shared = self.shared.clone();
        let mut state = shared.lock();
        loop {
            if state.stop || state.generation != self.generation {
                return;
            }
            if state.frames.len() < MAX_FRAMES {
                break;
            }
            // Nobody takes them while it doesn't show, and sound must not wait for that.
            let now = Instant::now();
            if state.clock.running
                && state
                    .frames
                    .get(1)
                    .is_some_and(|n| n.pts <= state.clock.at(now))
            {
                state.frames.pop_front();
                continue;
            }
            state = shared.wait(state, Duration::from_millis(20));
        }
        let was_empty = state.frames.is_empty();
        state.frames.push_back(Timed {
            pts,
            frame: video_frame,
        });
        state.frame_duration = self.duration;
        state.last_end = state.last_end.max(end);
        if !state.clock.started {
            let now = Instant::now();
            state.clock.started = true;
            state.clock.set(pts.max(self.target), now);
            let playing = state.playing;
            state.clock.run(playing, now);
        }
        drop(state);
        shared.changed.notify_all();
        if was_empty {
            (shared.waker)();
        }
    }

    /// Where the pixels of a decoded picture are for the renderer. Takes care of `frame`.
    fn pixels(&mut self, frame: *mut Opaque, format: c_int) -> Option<(VideoPixels, Keep)> {
        let ffmpeg = self.ffmpeg.clone();
        let f = &ffmpeg.f;
        let mut frame = frame;
        if format != ffmpeg.vaapi_pix_fmt {
            return memory_frame(&ffmpeg, frame, &mut self.scaler, Vec::new());
        }
        if !self.shared.lock().memory_only {
            let mapped = dmabuf_frame(&ffmpeg, frame);
            if mapped.is_some() {
                // The mapping holds on to what it was made from.
                // SAFETY: from `av_frame_alloc`.
                unsafe { (f.av_frame_free)(&mut frame) };
                return mapped;
            }
            self.shared.lock().memory_only = true;
        }
        let copy = alloc_frame(&ffmpeg);
        // SAFETY: a picture on the GPU, and an empty frame to copy it to.
        let code = unsafe { (f.av_hwframe_transfer_data)(copy, frame, 0) };
        // SAFETY: from `av_frame_alloc`.
        unsafe { (f.av_frame_free)(&mut frame) };
        if code < 0 {
            let mut copy = copy;
            // SAFETY: from `av_frame_alloc`.
            unsafe { (f.av_frame_free)(&mut copy) };
            return None;
        }
        memory_frame(&ffmpeg, copy, &mut self.scaler, Vec::new())
    }
}

fn video_thread(
    shared: Arc<Shared>,
    ffmpeg: Arc<Ffmpeg>,
    info: StreamInfo,
    node: Option<std::path::PathBuf>,
) {
    let software = shared.lock().memory_only;
    let hardware = node
        .filter(|_| !software)
        .and_then(|node| hardware_decoder(&ffmpeg, &info, &node));
    let decoder = match hardware {
        Some(decoder) => decoder,
        None => match open_decoder(&ffmpeg, &info, info.decoder, None, thread_count()) {
            Ok(decoder) => decoder,
            Err(e) => return shared.fail(e),
        },
    };
    let mut this = Pictures {
        shared: shared.clone(),
        ffmpeg: ffmpeg.clone(),
        info,
        decoder,
        stream: next_id(),
        alive: Arc::new(()),
        scaler: None,
        target: f64::NEG_INFINITY,
        generation: 0,
        last_pts: f64::NEG_INFINITY,
        duration: 0.0,
        decoded_any: false,
        failures: 0,
        kept: Vec::new(),
    };
    while let Some(item) = next_item(&shared, true) {
        match item {
            Item::Flush { target } => {
                // SAFETY: an open decoder.
                unsafe { (ffmpeg.f.avcodec_flush_buffers)(this.decoder.raw) };
                this.target = target;
                this.generation = shared.lock().generation;
                this.last_pts = f64::NEG_INFINITY;
                this.kept.clear();
            }
            Item::Packet(packet) => {
                if !this.feed(Some(packet)) {
                    return;
                }
            }
            Item::End => {
                if !this.feed(None) {
                    return;
                }
                shared.lock().video_done = true;
                shared.changed.notify_all();
            }
        }
    }
}

/// `SwrContext`.
struct Resampler {
    ffmpeg: Arc<Ffmpeg>,
    raw: *mut Opaque,
    /// Sample format and rate it takes.
    from: (c_int, c_int),
}

impl Drop for Resampler {
    fn drop(&mut self) {
        // SAFETY: from `swr_alloc_set_opts2`, and freed once.
        unsafe { (self.ffmpeg.f.swr_free)(&mut self.raw) };
    }
}

/// What the thread that decodes sound keeps track of.
struct Sound {
    shared: Arc<Shared>,
    ffmpeg: Arc<Ffmpeg>,
    time_base: f64,
    output: Option<Output>,
    /// There is nowhere for sound to go: the clock goes by the time.
    silent: bool,
    resampler: Option<Resampler>,
    s16: c_int,
    /// Where what was written ends, in seconds of the video.
    end: f64,
    /// Nothing went out since it was told where to start.
    fresh: bool,
    target: f64,
    generation: u64,
    samples: Vec<i16>,
    device_paused: bool,
}

impl Sound {
    /// Waits until this is to go on. Returns false if what it has is out of date.
    fn wait_to_go_on(&mut self) -> bool {
        let shared = self.shared.clone();
        let mut state = shared.lock();
        loop {
            if state.stop || state.generation != self.generation {
                return false;
            }
            if state.playing && state.clock.started {
                break;
            }
            if !self.device_paused && !state.playing {
                self.device_paused = true;
                if let Some(output) = &mut self.output {
                    output.pause(true);
                }
            }
            state = shared.wait(state, Duration::from_millis(50));
        }
        drop(state);
        if std::mem::take(&mut self.device_paused)
            && let Some(output) = &mut self.output
        {
            output.pause(false);
        }
        true
    }

    /// Turns a frame into pairs of samples, in `samples`. Returns the sample rate.
    fn convert(&mut self, frame: *const Opaque) -> Option<u32> {
        let ffmpeg = self.ffmpeg.clone();
        let (f, l) = (&ffmpeg.f, ffmpeg.layout);
        // SAFETY: a frame just decoded, and the layout says where the fields are.
        unsafe {
            let format: c_int = read(frame, l.frame_format);
            let rate: c_int = read(frame, l.frame_sample_rate);
            let count: c_int = read(frame, l.frame_nb_samples);
            let layout = frame
                .cast::<u8>()
                .add(l.frame_ch_layout)
                .cast::<ChannelLayout>();
            if rate <= 0 {
                return None;
            }
            if self
                .resampler
                .as_ref()
                .is_none_or(|r| r.from != (format, rate))
            {
                let mut stereo = ChannelLayout::empty();
                (f.av_channel_layout_default)(&mut stereo, 2);
                let mut raw = std::ptr::null_mut();
                let code = (f.swr_alloc_set_opts2)(
                    &mut raw,
                    &stereo,
                    self.s16,
                    rate,
                    layout,
                    format,
                    rate,
                    0,
                    std::ptr::null_mut(),
                );
                (f.av_channel_layout_uninit)(&mut stereo);
                if code < 0 || (f.swr_init)(raw) < 0 {
                    (f.swr_free)(&mut raw);
                    return None;
                }
                self.resampler = Some(Resampler {
                    ffmpeg: ffmpeg.clone(),
                    raw,
                    from: (format, rate),
                });
            }
            let room = count as usize + 64;
            self.samples.resize(room * 2, 0);
            let out = [self.samples.as_mut_ptr().cast::<u8>()];
            let input: *const *const u8 = read(frame, l.frame_extended_data);
            let made = (f.swr_convert)(
                self.resampler.as_ref().expect("just made").raw,
                out.as_ptr(),
                room as c_int,
                input,
                count,
            );
            self.samples.truncate(made.max(0) as usize * 2);
            Some(rate as u32)
        }
    }

    /// Sends a decoded frame out.
    fn play(&mut self, frame: *const Opaque) {
        let ffmpeg = self.ffmpeg.clone();
        let l = ffmpeg.layout;
        if self.silent {
            return;
        }
        // SAFETY: a frame just decoded, and the layout says where the field is.
        let pts: i64 = unsafe { read(frame, l.frame_best_effort) };
        let Some(rate) = self.convert(frame) else {
            return;
        };
        if self.output.as_ref().is_none_or(|o| o.rate != rate) {
            self.output = Output::open(rate);
            if self.output.is_none() {
                // Nowhere for sound to go, and no reason not to show the picture.
                self.silent = true;
                self.shared.lock().has_audio = false;
                return;
            }
        }
        let mut samples = std::mem::take(&mut self.samples);
        self.write(pts, rate, &mut samples);
        self.samples = samples;
    }

    fn write(&mut self, pts: i64, rate: u32, samples: &mut Vec<i16>) {
        let mut start = if pts == AV_NOPTS_VALUE {
            self.end
        } else {
            pts as f64 * self.time_base
        };
        if !self.wait_to_go_on() {
            return;
        }
        if std::mem::take(&mut self.fresh) {
            let at = self.shared.lock().clock.at(Instant::now()).max(self.target);
            // Line it up with the picture, which is what the clock starts from.
            let early = at - start;
            let frames = (early.abs() * rate as f64) as usize;
            if early > 0.0 {
                let gone = (frames * 2).min(samples.len());
                samples.drain(..gone);
            } else if early < 0.0 && early > -1.0 {
                samples.splice(0..0, std::iter::repeat_n(0, frames * 2));
            }
            start = at;
        } else if self.end.is_finite() && (start - self.end).abs() < 0.1 {
            // What is written follows on, which rounding in the file doesn't change.
            start = self.end;
        }
        self.end = start;
        let piece = ((rate as f64 * AUDIO_PIECE) as usize).max(1) * 2;
        for chunk in samples.chunks_mut(piece) {
            if !self.wait_to_go_on() {
                return;
            }
            let (gain, generation_ok) = {
                let state = self.shared.lock();
                (
                    if state.muted { 0.0 } else { state.volume },
                    state.generation == self.generation,
                )
            };
            if !generation_ok {
                return;
            }
            if gain < 1.0 {
                for sample in chunk.iter_mut() {
                    *sample = (*sample as f32 * gain) as i16;
                }
            }
            let Some(output) = &mut self.output else {
                return;
            };
            if !output.write(chunk) {
                self.output = None;
                self.silent = true;
                self.shared.lock().has_audio = false;
                return;
            }
            self.end += (chunk.len() / 2) as f64 / rate as f64;
            let heard = self.end - output.delay();
            let now = Instant::now();
            let shared = self.shared.clone();
            let mut state = shared.lock();
            if state.clock.running && state.generation == self.generation {
                state.clock.follow(heard, now);
            }
            // Something that takes sound faster than it can be heard doesn't get to run ahead.
            while state.clock.running
                && !state.stop
                && state.generation == self.generation
                && self.end - state.clock.at(Instant::now()) > MAX_AUDIO_AHEAD
            {
                state = shared.wait(state, Duration::from_millis(10));
            }
        }
    }
}

fn audio_thread(shared: Arc<Shared>, ffmpeg: Arc<Ffmpeg>, info: StreamInfo) {
    let f = &ffmpeg.f;
    let decoder = open_decoder(&ffmpeg, &info, info.decoder, None, 1).ok();
    if decoder.is_none() {
        let mut state = shared.lock();
        state.has_audio = false;
        state.audio_done = true;
    }
    // SAFETY: a valid string.
    let s16 = unsafe { (f.av_get_sample_fmt)(c"s16".as_ptr()) };
    let mut this = Sound {
        shared: shared.clone(),
        ffmpeg: ffmpeg.clone(),
        time_base: info.time_base,
        output: None,
        silent: decoder.is_none(),
        resampler: None,
        s16,
        end: f64::NAN,
        fresh: true,
        target: f64::NEG_INFINITY,
        generation: 0,
        samples: Vec::new(),
        device_paused: false,
    };
    let frame = alloc_frame(&ffmpeg);
    let drain = |this: &mut Sound, decoder: *mut Opaque| {
        loop {
            // SAFETY: an open decoder and a frame to fill.
            if unsafe { (f.avcodec_receive_frame)(decoder, frame) } < 0 {
                break;
            }
            this.play(frame);
            // SAFETY: a frame.
            unsafe { (f.av_frame_unref)(frame) };
        }
    };
    while let Some(item) = next_item(&shared, false) {
        // Without a decoder all that is left is to keep the line moving.
        let Some(decoder) = &decoder else {
            continue;
        };
        match item {
            Item::Flush { target } => {
                // SAFETY: an open decoder.
                unsafe { (f.avcodec_flush_buffers)(decoder.raw) };
                if let Some(output) = &mut this.output {
                    output.flush();
                }
                this.target = target;
                this.generation = shared.lock().generation;
                this.fresh = true;
                this.end = f64::NAN;
                this.device_paused = false;
            }
            Item::Packet(packet) => {
                loop {
                    // SAFETY: an open decoder and a packet.
                    let code = unsafe { (f.avcodec_send_packet)(decoder.raw, packet.raw) };
                    drain(&mut this, decoder.raw);
                    if code != AVERROR_EAGAIN {
                        break;
                    }
                }
            }
            Item::End => {
                // SAFETY: an open decoder, and nothing means the end.
                unsafe { (f.avcodec_send_packet)(decoder.raw, std::ptr::null()) };
                drain(&mut this, decoder.raw);
                shared.lock().audio_done = true;
                shared.changed.notify_all();
            }
        }
    }
    let mut frame = frame;
    // SAFETY: from `av_frame_alloc`, and freed once.
    unsafe { (f.av_frame_free)(&mut frame) };
}
