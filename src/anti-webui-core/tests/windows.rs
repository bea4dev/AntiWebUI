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
