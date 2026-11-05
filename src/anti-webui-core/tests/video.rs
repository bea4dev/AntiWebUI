//! How a `video` behaves, with something standing in for what decodes. See
//! `docs/design/24-video.md`.

use anti_webui_core::Ui;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{RectI, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{Primitive, Scene, VideoDraw};
use anti_webui_core::video::{
    MemoryPlane, PlaneFormat, VideoBackend, VideoColor, VideoEvent, VideoFrame, VideoPixels,
    VideoRequest, VideoStream, VideoTiming, set_video_backend,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Default)]
struct Log {
    opened: Vec<VideoRequest>,
    calls: Vec<String>,
    /// Streams alive.
    alive: usize,
    events: Vec<VideoEvent>,
    /// What each stream is showing.
    frame: Option<u64>,
    next_change: Option<Instant>,
    time: f64,
}

type Shared = Rc<RefCell<Log>>;

struct Fake(Shared);

struct FakeStream(Shared);

impl Drop for FakeStream {
    fn drop(&mut self) {
        self.0.borrow_mut().alive -= 1;
    }
}

fn picture(id: u64) -> Arc<VideoFrame> {
    static PIXELS: [u8; 16] = [128; 16];
    // SAFETY: the pixels are static.
    let plane = unsafe { MemoryPlane::new(PlaneFormat::R8, (4, 4), 4, PIXELS.as_ptr()) };
    Arc::new(VideoFrame {
        id,
        stream: 1,
        alive: Arc::downgrade(&Arc::new(())),
        size: (4, 4),
        pixels: VideoPixels::Memory(vec![plane, plane]),
        color: VideoColor::default(),
        rotation: 0,
        keep: Box::new(()),
    })
}

impl VideoStream for FakeStream {
    fn poll(&mut self) -> Vec<VideoEvent> {
        std::mem::take(&mut self.0.borrow_mut().events)
    }
    fn frame(&mut self, _: Instant, _: Option<VideoTiming>) -> Option<Arc<VideoFrame>> {
        self.0.borrow().frame.map(picture)
    }
    fn next_change(&self, _: Instant, _: Option<VideoTiming>) -> Option<Instant> {
        self.0.borrow().next_change
    }
    fn current_time(&self, _: Instant) -> f64 {
        self.0.borrow().time
    }
    fn set_playing(&mut self, playing: bool) {
        self.0.borrow_mut().calls.push(format!("playing {playing}"));
    }
    fn seek(&mut self, seconds: f64) {
        self.0.borrow_mut().calls.push(format!("seek {seconds}"));
    }
    fn set_looped(&mut self, looped: bool) {
        self.0.borrow_mut().calls.push(format!("looped {looped}"));
    }
    fn set_muted(&mut self, muted: bool) {
        self.0.borrow_mut().calls.push(format!("muted {muted}"));
    }
    fn set_volume(&mut self, volume: f32) {
        self.0.borrow_mut().calls.push(format!("volume {volume}"));
    }
    fn use_memory(&mut self) {
        self.0.borrow_mut().calls.push("memory".into());
    }
}

impl VideoBackend for Fake {
    fn open(&self, request: VideoRequest) -> Box<dyn VideoStream> {
        let mut log = self.0.borrow_mut();
        log.opened.push(request);
        log.alive += 1;
        Box::new(FakeStream(self.0.clone()))
    }
    fn availability(&self, _: &VideoSettings) -> Result<String, VideoError> {
        Ok("fake".into())
    }
    fn can_play(&self, mime: &str, _: &VideoSettings) -> bool {
        mime == "video/fake"
    }
}

fn backend() -> Shared {
    let log = Shared::default();
    set_video_backend(Rc::new(Fake(log.clone())));
    log
}

fn window(ui: &mut Ui, w: i32, h: i32, scale: f64) {
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(w, h),
        scale: Scale::from_f64(scale),
    });
}

fn opened(size: (u32, u32), duration: Option<f64>) -> VideoEvent {
    VideoEvent::Opened { size, duration }
}

/// Goes through what a frame goes through, and returns what it would draw.
fn frame(ui: &mut Ui) -> Scene {
    let now = Instant::now();
    ui.animate(now);
    ui.update();
    ui.paint(now)
}

fn videos(scene: &Scene) -> Vec<VideoDraw> {
    fn walk(scene: &Scene, out: &mut Vec<VideoDraw>) {
        for p in &scene.primitives {
            match p {
                Primitive::Video(v) => out.push(v.clone()),
                Primitive::Layer(l) => walk(&l.content, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(scene, &mut out);
    out
}

#[test]
fn has_the_size_of_the_video_once_that_is_known() {
    let log = backend();
    let mut ui = Ui::new(|| div().child(video("a.mp4")));
    window(&mut ui, 800, 600, 2.0);
    log.borrow_mut().frame = Some(1);
    assert!(videos(&frame(&mut ui)).is_empty(), "0x0 until it is known");
    assert_eq!(log.borrow().opened.len(), 1);
    log.borrow_mut().events.push(opened((320, 240), Some(10.0)));
    let scene = frame(&mut ui);
    let shown = videos(&scene);
    assert_eq!(shown.len(), 1);
    // A pixel of the video is a logical pixel, as with a picture.
    assert_eq!(shown[0].rect, RectI::new(0, 0, 640, 480));
}

#[test]
fn fits_like_a_picture() {
    let log = backend();
    let mut ui = Ui::new(|| {
        video("a.mp4")
            .w(100.0)
            .h(100.0)
            .object_fit(ObjectFit::Contain)
            .rounded(10.0)
            .opacity(0.5)
    });
    window(&mut ui, 400, 400, 1.0);
    {
        let mut l = log.borrow_mut();
        l.frame = Some(1);
        l.events.push(opened((200, 100), None));
    }
    frame(&mut ui);
    let shown = videos(&frame(&mut ui));
    assert_eq!(shown[0].rect, RectI::new(0, 25, 100, 50));
    assert_eq!(shown[0].shape.rect, RectI::new(0, 0, 100, 100));
    assert_eq!(shown[0].shape.radii, [10.0; 4]);
    assert!(
        (shown[0].opacity - 0.5).abs() < 0.01,
        "{}",
        shown[0].opacity
    );
}

#[test]
fn a_new_frame_only_touches_where_it_is() {
    let log = backend();
    let mut ui = Ui::new(|| {
        div()
            .child(text("around"))
            .child(video("a.mp4").w(100.0).h(50.0))
    });
    window(&mut ui, 400, 400, 1.0);
    {
        let mut l = log.borrow_mut();
        l.frame = Some(1);
        l.events.push(opened((100, 50), None));
    }
    frame(&mut ui);
    let before = frame(&mut ui);
    let at = videos(&before)[0].rect;
    {
        let mut l = log.borrow_mut();
        l.frame = Some(2);
        l.next_change = Some(Instant::now() - Duration::from_millis(1));
    }
    ui.tick(Instant::now());
    assert!(ui.needs_redraw());
    log.borrow_mut().next_change = None;
    let after = frame(&mut ui);
    assert_eq!(videos(&after)[0].frame.id, 2);
    let touched = anti_webui_core::damage::diff(&before, &after);
    assert_eq!(touched.rects(), &[at][..]);
}

#[test]
fn asks_to_be_woken_when_a_frame_is_due_and_not_otherwise() {
    let log = backend();
    let mut ui = Ui::new(|| video("a.mp4").w(10.0).h(10.0));
    window(&mut ui, 100, 100, 1.0);
    frame(&mut ui);
    assert_eq!(ui.next_deadline(Instant::now()), None);
    assert!(!ui.animating());
    let at = Instant::now() + Duration::from_millis(40);
    log.borrow_mut().next_change = Some(at);
    assert_eq!(ui.next_deadline(Instant::now()), Some(at));
    // It doesn't make frames come by itself.
    assert!(!ui.animating());
}

#[test]
fn does_nothing_for_what_doesnt_show() {
    let log = backend();
    let mut ui = Ui::new(|| {
        div()
            .relative()
            .w(100.0)
            .h(100.0)
            .child(video("a.mp4").w(10.0).h(10.0).absolute().top(1000.0))
    });
    window(&mut ui, 100, 100, 1.0);
    log.borrow_mut().frame = Some(1);
    frame(&mut ui);
    frame(&mut ui);
    log.borrow_mut().next_change = Some(Instant::now() - Duration::from_millis(1));
    ui.tick(Instant::now());
    assert!(!ui.needs_redraw());
}

#[test]
fn the_player_says_what_to_do() {
    let log = backend();
    let p = Rc::new(RefCell::new(None));
    let p2 = p.clone();
    let mut ui = Ui::new(move || {
        let player = video_player();
        *p2.borrow_mut() = Some(player);
        video("a.mp4").player(player).w(10.0).h(10.0)
    });
    window(&mut ui, 100, 100, 1.0);
    frame(&mut ui);
    let player = p.borrow().unwrap();
    assert!(player.paused());
    assert!(!log.borrow().opened[0].playing);
    log.borrow_mut().calls.clear();
    player.play();
    player.seek(3.5);
    player.set_volume(0.25);
    player.set_muted(true);
    player.pause();
    ui.update();
    let calls = log.borrow().calls.clone();
    assert_eq!(
        calls,
        [
            "playing true",
            "seek 3.5",
            "volume 0.25",
            "muted true",
            "playing false"
        ]
    );
    assert_eq!(player.current_time(), 3.5);
    assert_eq!(player.volume(), 0.25);
    assert!(player.muted());
    assert!(player.paused());
}

#[test]
fn the_first_of_it_says_how_to_start() {
    let log = backend();
    let mut ui = Ui::new(|| {
        video("a.mp4")
            .autoplay(true)
            .looped(true)
            .muted(true)
            .w(10.0)
            .h(10.0)
    });
    window(&mut ui, 100, 100, 1.0);
    frame(&mut ui);
    let log = log.borrow();
    let r = &log.opened[0];
    assert_eq!(r.source, "a.mp4");
    assert!(r.playing);
    assert!(r.looped);
    assert!(r.muted);
}

#[test]
fn says_what_happens() {
    let log = backend();
    let heard: Rc<RefCell<Vec<String>>> = Rc::default();
    let h = heard.clone();
    let p = Rc::new(RefCell::new(None));
    let p2 = p.clone();
    let mut ui = Ui::new(move || {
        let player = video_player();
        *p2.borrow_mut() = Some(player);
        let (a, b, c) = (h.clone(), h.clone(), h.clone());
        video("a.mp4")
            .player(player)
            .autoplay(true)
            .w(10.0)
            .h(10.0)
            .on_load(move |w, h, s| a.borrow_mut().push(format!("load {w} {h} {s}")))
            .on_ended(move || b.borrow_mut().push("ended".into()))
            .on_error(move |e| c.borrow_mut().push(format!("{e:?}")))
    });
    window(&mut ui, 100, 100, 1.0);
    frame(&mut ui);
    let player = p.borrow().unwrap();
    assert!(player.duration().is_nan());
    log.borrow_mut().events.push(opened((8, 6), Some(2.5)));
    frame(&mut ui);
    assert_eq!(player.duration(), 2.5);
    assert_eq!(*heard.borrow(), ["load 8 6 2.5"]);
    log.borrow_mut().time = 2.5;
    log.borrow_mut().events.push(VideoEvent::Ended);
    frame(&mut ui);
    assert!(player.ended());
    assert!(player.paused());
    assert_eq!(player.current_time(), 2.5);
    log.borrow_mut()
        .events
        .push(VideoEvent::Failed(VideoError::Decode("broken".into())));
    frame(&mut ui);
    assert_eq!(heard.borrow()[1], "ended");
    assert_eq!(heard.borrow()[2], "Decode(\"broken\")");
}

#[test]
fn another_source_starts_over_and_going_away_stops_it() {
    let log = backend();
    let source = Rc::new(RefCell::new(None));
    let there = Rc::new(RefCell::new(None));
    let (s2, t2) = (source.clone(), there.clone());
    let mut ui = Ui::new(move || {
        let name = signal("a.mp4".to_owned());
        let show_it = signal(true);
        *s2.borrow_mut() = Some(name);
        *t2.borrow_mut() = Some(show_it);
        show(
            move || show_it.get(),
            move || video(move || name.get()).w(10.0).h(10.0),
        )
    });
    window(&mut ui, 100, 100, 1.0);
    frame(&mut ui);
    assert_eq!(log.borrow().alive, 1);
    source.borrow().unwrap().set("b.mp4".to_owned());
    frame(&mut ui);
    {
        let log = log.borrow();
        assert_eq!(log.opened.len(), 2);
        assert_eq!(log.opened[1].source, "b.mp4");
        assert_eq!(log.alive, 1);
    }
    source.borrow().unwrap().set("b.mp4".to_owned());
    frame(&mut ui);
    assert_eq!(log.borrow().opened.len(), 2, "the same stays");
    there.borrow().unwrap().set(false);
    frame(&mut ui);
    assert_eq!(log.borrow().alive, 0);
}

#[test]
fn opening_waits_for_the_window_and_knows_the_gpu() {
    let log = backend();
    let mut ui = Ui::new(|| video("a.mp4").w(10.0).h(10.0));
    window(&mut ui, 100, 100, 1.0);
    assert!(log.borrow().opened.is_empty());
    ui.set_video_render_node(Some("/dev/dri/renderD128".into()));
    frame(&mut ui);
    assert_eq!(
        log.borrow().opened[0].render_node.as_deref(),
        Some(std::path::Path::new("/dev/dri/renderD128"))
    );
}

#[test]
fn what_cant_be_read_on_the_gpu_comes_in_memory() {
    let log = backend();
    let mut ui = Ui::new(|| video("a.mp4").w(10.0).h(10.0));
    window(&mut ui, 100, 100, 1.0);
    frame(&mut ui);
    log.borrow_mut().calls.clear();
    ui.video_use_memory();
    assert_eq!(log.borrow().calls, ["memory"]);
}

#[test]
fn without_a_backend_it_says_so_and_nothing_breaks() {
    // On a thread of its own: nothing has been set here.
    std::thread::spawn(|| {
        let heard: Rc<RefCell<Option<VideoError>>> = Rc::default();
        let h = heard.clone();
        let mut ui = Ui::new(move || {
            video("a.mp4")
                .w(10.0)
                .h(10.0)
                .on_error(move |e| *h.borrow_mut() = Some(e.clone()))
        });
        window(&mut ui, 100, 100, 1.0);
        frame(&mut ui);
        frame(&mut ui);
        assert!(matches!(*heard.borrow(), Some(VideoError::Unavailable(_))));
        assert!(matches!(
            video_availability(),
            Err(VideoError::Unavailable(_))
        ));
        assert!(!can_play_video("video/mp4"));
    })
    .join()
    .unwrap();
}

#[test]
fn says_what_can_be_played() {
    let _log = backend();
    assert_eq!(video_availability().unwrap(), "fake");
    assert!(can_play_video("video/fake"));
    assert!(!can_play_video("video/other"));
}
