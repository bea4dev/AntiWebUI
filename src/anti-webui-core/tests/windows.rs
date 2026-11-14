//! More than one window on a thread: each has its own document, and what is shared between them
//! reaches the right one. See `docs/design/36-multi-window.md`.

use anti_webui_core::Ui;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::platform::PlatformRequest;
use anti_webui_core::prelude::*;
use anti_webui_core::window::{Command, take_commands, take_opens};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

const SIZE: SizeI = SizeI {
    width: 400,
    height: 300,
};

fn show(ui: &mut Ui) {
    ui.handle_event(WindowEvent::Resized {
        size: SIZE,
        scale: Scale(120),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
}

/// Opens what the first window asked for, as the app does.
fn open_asked() -> Ui {
    let mut opens = take_opens();
    assert_eq!(opens.len(), 1, "one window was asked for");
    let mut ui = Ui::open(opens.remove(0));
    show(&mut ui);
    ui
}

fn closes(ui: &mut Ui) -> bool {
    ui.take_requests()
        .iter()
        .any(|r| matches!(r, PlatformRequest::Close))
}

fn press(ui: &mut Ui, x: f32, y: f32) {
    for pressed in [true, false] {
        ui.handle_event(WindowEvent::PointerButton {
            position: PointF::new(x, y),
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
            time: 0,
        });
    }
}

#[test]
fn a_signal_written_in_one_window_changes_the_document_of_the_other() {
    let width = signal(40.0f32);
    let (first, second) = (node_ref(), node_ref());
    let mut a = Ui::new(move || {
        open_window(WindowOptions::new().title("B"), move || {
            div().child(
                div()
                    .node_ref(second)
                    .h(10.0)
                    .dyn_style(move |s| s.w(width.get())),
            )
        });
        div().child(
            div()
                .node_ref(first)
                .w(100.0)
                .h(10.0)
                .on_click(move |_| width.set(70.0)),
        )
    });
    show(&mut a);
    let mut b = open_asked();
    assert_eq!(first.rect().unwrap().width, 100);
    assert_eq!(second.rect().unwrap().width, 40);
    assert_ne!(a.window(), b.window());

    press(&mut a, 5.0, 5.0);
    a.update();
    assert_eq!(
        first.rect().unwrap().width,
        100,
        "A's own node is as it was"
    );
    assert!(b.needs_redraw(), "B was touched from A's handler");
    b.update();
    assert_eq!(second.rect().unwrap().width, 70);
}

#[test]
fn timers_and_cleanups_run_in_the_window_they_were_made_in() {
    let fired = Rc::new(Cell::new(false));
    let cleaned = Rc::new(Cell::new(false));
    let (f, c) = (fired.clone(), cleaned.clone());
    let mut a = Ui::new(move || {
        open_window(WindowOptions::new(), move || {
            set_timeout(Duration::from_millis(10), move || f.set(true));
            on_cleanup(move || c.set(true));
            div()
        });
        div()
    });
    show(&mut a);
    let mut b = open_asked();
    let now = Instant::now();
    assert!(
        a.next_deadline(now)
            .is_none_or(|d| d > now + Duration::from_secs(1))
    );
    assert!(b.next_deadline(now).is_some());
    a.tick(now + Duration::from_millis(50));
    assert!(!fired.get(), "A's timers are A's");
    b.tick(now + Duration::from_millis(50));
    assert!(fired.get());

    assert!(!cleaned.get());
    b.close();
    assert!(cleaned.get());
    // The first window goes on as before.
    a.update();
}

#[test]
fn a_close_request_can_be_kept_from_closing() {
    let keep = signal(true);
    let asked = Rc::new(Cell::new(0));
    let seen = asked.clone();
    let mut ui = Ui::new(move || {
        on_close_request(move |request| {
            seen.set(seen.get() + 1);
            if keep.get_untracked() {
                request.prevent();
            }
        });
        div()
    });
    show(&mut ui);
    ui.handle_event(WindowEvent::CloseRequested);
    assert_eq!(asked.get(), 1);
    assert!(!closes(&mut ui));
    keep.set(false);
    ui.handle_event(WindowEvent::CloseRequested);
    assert_eq!(asked.get(), 2);
    assert!(closes(&mut ui));
}

#[test]
fn a_modal_window_keeps_input_from_the_one_that_opened_it() {
    let clicks = Rc::new(Cell::new(0));
    let c = clicks.clone();
    let mut a = Ui::new(move || {
        div()
            .w(100.0)
            .h(100.0)
            .on_click(move |_| c.set(c.get() + 1))
    });
    show(&mut a);
    // The first window opens a dialog from a handler.
    let _dialog = {
        a.handle_event(WindowEvent::Focused(true));
        anti_webui_core::window::in_window(a.window(), || {
            open_window(WindowOptions::new().modal(true), div)
        })
    };
    let b = open_asked();
    let commands = take_commands();
    assert_eq!(commands, vec![(a.window(), Command::ModalChild(true))]);
    for (_, command) in commands {
        a.apply(command);
    }
    press(&mut a, 5.0, 5.0);
    assert_eq!(clicks.get(), 0);
    a.handle_event(WindowEvent::CloseRequested);
    assert!(!closes(&mut a), "a window under a dialog doesn't close");

    b.close();
    for (_, command) in take_commands() {
        a.apply(command);
    }
    press(&mut a, 5.0, 5.0);
    assert_eq!(clicks.get(), 1);
}

#[test]
fn a_handle_sends_to_its_window_and_closing_closes_what_it_opened() {
    let mut a = Ui::new(move || {
        open_window(WindowOptions::new(), div);
        div()
    });
    show(&mut a);
    let b = open_asked();
    let handle = anti_webui_core::window::in_window(b.window(), current_window);
    handle.set_title("New");
    assert_eq!(
        take_commands(),
        vec![(b.window(), Command::SetTitle("New".into()))]
    );
    // B is A's child: A closing closes it.
    let child = anti_webui_core::window::in_window(a.window(), current_window);
    a.close();
    assert!(!child.is_open());
    assert_eq!(take_commands(), vec![(b.window(), Command::Close)]);
    b.close();
    assert!(!handle.is_open());
    handle.set_title("Gone");
    assert!(take_commands().is_empty(), "a closed window hears nothing");
}

#[test]
fn a_node_ref_works_from_the_handlers_of_another_window() {
    let target = node_ref();
    let mut a = Ui::new(move || {
        open_window(WindowOptions::new(), move || {
            div().child(div().node_ref(target).w(20.0).h(20.0).focusable())
        });
        div().w(50.0).h(50.0).on_click(move |_| target.focus())
    });
    show(&mut a);
    let mut b = open_asked();
    press(&mut a, 5.0, 5.0);
    b.update();
    let focused = anti_webui_core::window::in_window(b.window(), || {
        anti_webui_core::tree::with_doc(|d| d.state.focused)
    });
    assert_eq!(focused, target.get());
}

#[test]
fn the_frame_and_the_state_of_the_window_follow_the_platform() {
    let seen = Rc::new(Cell::new((Decorations::Server, false, false)));
    let s = seen.clone();
    let mut ui = Ui::new(move || {
        effect(move || s.set((window_decorations(), window_maximized(), window_active())));
        div()
    });
    show(&mut ui);
    assert_eq!(seen.get(), (Decorations::Server, false, true));
    ui.handle_event(WindowEvent::Decorations(Decorations::Client));
    ui.handle_event(WindowEvent::Maximized(true));
    ui.handle_event(WindowEvent::Focused(false));
    assert_eq!(seen.get(), (Decorations::Client, true, false));
}

#[test]
fn each_window_says_what_draws_it() {
    use anti_webui_core::window::{GpuBackend, GpuInfo, GpuKind, GpuPresent};
    let seen: Rc<Cell<Vec<Option<GpuInfo>>>> = Rc::default();
    let s = seen.clone();
    let mut a = Ui::new(move || {
        open_window(WindowOptions::new(), div);
        effect(move || {
            let mut v = s.take();
            v.push(gpu_info());
            s.set(v);
        });
        div()
    });
    show(&mut a);
    let mut b = open_asked();
    assert_eq!(seen.take(), vec![None], "nothing before the first frame");

    let vulkan = GpuInfo::new(
        GpuBackend::Vulkan,
        "AMD Radeon 880M Graphics (RADV STRIX1)".into(),
        GpuKind::Integrated,
        "radv Mesa".into(),
        GpuPresent::Dmabuf {
            explicit_sync: true,
        },
    );
    a.set_gpu_info(vulkan.clone());
    assert_eq!(seen.take(), vec![Some(vulkan.clone())]);
    a.set_gpu_info(vulkan.clone());
    assert_eq!(seen.take(), vec![], "the same again changes nothing");
    assert_eq!(
        vulkan.to_string(),
        "AMD Radeon 880M Graphics (RADV STRIX1), Vulkan, dmabuf, explicit sync"
    );

    // The other window is drawn by something else, and says so to what reads it there.
    let gl = GpuInfo::new(
        GpuBackend::Gl,
        "llvmpipe".into(),
        GpuKind::Cpu,
        String::new(),
        GpuPresent::Surface,
    );
    b.set_gpu_info(gl.clone());
    assert_eq!(seen.take(), vec![], "not the first window's");
    let there = anti_webui_core::window::in_window(b.window(), gpu_info);
    assert_eq!(there, Some(gl));
    let here = anti_webui_core::window::in_window(a.window(), gpu_info);
    assert_eq!(here, Some(vulkan));
}

#[test]
fn a_thread_without_a_window_closes_what_it_opened_when_it_stops() {
    use anti_webui_core::window::{has_roots, stop_root, take_threads};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let cleaned = Arc::new(AtomicBool::new(false));
    let c = cleaned.clone();
    let thread = spawn_ui_thread(move || {
        open_window(WindowOptions::new().title("Bar"), move || {
            on_cleanup(move || c.store(true, Ordering::Relaxed));
            text("bar")
        });
    });
    // The app starts it on a thread of its own; here it runs on this one.
    let mut asked = take_threads();
    assert_eq!(asked.len(), 1);
    let id = asked[0].handle.id();
    asked.remove(0).start();
    assert!(has_roots() && thread.is_running());
    let mut bar = open_asked();
    assert!(take_commands().is_empty());

    thread.stop();
    let commands = take_commands();
    assert_eq!(commands, [(id, Command::Close)]);
    assert!(stop_root(id));
    assert!(!has_roots() && !thread.is_running());
    // What it opened is told to close, as the app does it.
    assert_eq!(take_commands(), [(bar.window(), Command::Close)]);
    bar.apply(Command::Close);
    assert!(closes(&mut bar));
    bar.close();
    assert!(cleaned.load(Ordering::Relaxed));
}

#[test]
fn a_window_can_follow_the_size_of_what_is_in_it() {
    let mut ui = Ui::new(|| div().w(123.0).h(45.0));
    ui.set_fit_content(true, true);
    show(&mut ui);
    assert_eq!(ui.content_size(), Some(SizeI::new(123, 45)));
    // Along an axis that doesn't follow, it is what the window has.
    ui.set_fit_content(false, true);
    ui.update();
    assert_eq!(ui.content_size(), Some(SizeI::new(SIZE.width, 45)));
}

#[test]
fn a_thread_without_a_window_animates_on_frames_of_its_own() {
    use anti_webui_core::window::{roots_deadline, run_roots, stop_root, take_threads};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};
    let frames = Arc::new(AtomicU32::new(0));
    let f = frames.clone();
    let _thread = spawn_ui_thread(move || {
        fn again(frames: Arc<AtomicU32>) {
            request_frame(move |_| {
                if frames.fetch_add(1, Ordering::Relaxed) < 2 {
                    again(frames);
                }
            });
        }
        again(f);
    });
    let mut asked = take_threads();
    let id = asked[0].handle.id();
    asked.remove(0).start();
    let start = Instant::now();
    // A frame is wanted now, and then one a display's frame after the last.
    assert!(roots_deadline().is_some_and(|at| at <= Instant::now()));
    run_roots(start);
    assert_eq!(frames.load(Ordering::Relaxed), 1);
    run_roots(start + Duration::from_millis(2));
    assert_eq!(
        frames.load(Ordering::Relaxed),
        1,
        "not before the next frame is due"
    );
    let next = roots_deadline().expect("another frame is wanted");
    assert!(next > start && next <= start + Duration::from_millis(9));
    run_roots(start + Duration::from_millis(9));
    run_roots(start + Duration::from_millis(18));
    assert_eq!(frames.load(Ordering::Relaxed), 3);
    // Nothing more is asked for: the thread can sleep.
    assert_eq!(roots_deadline(), None);
    assert!(stop_root(id));
}
