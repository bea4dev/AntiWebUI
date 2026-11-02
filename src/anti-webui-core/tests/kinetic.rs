//! Touchpad scrolling on real documents: gestures, inertia and the settings for them.

use anti_webui_core::Ui;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, RectI, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::settings::Settings;
use anti_webui_core::tree::with_doc;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

const FRAME: Duration = Duration::from_millis(16);

fn mount_at<V: IntoView>(scale: u32, f: impl FnOnce() -> V) -> Ui {
    update_settings(|s| *s = Settings::default());
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(600, 400),
        scale: Scale(scale),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    ui
}

fn mount<V: IntoView>(f: impl FnOnce() -> V) -> Ui {
    mount_at(120, f)
}

fn center(r: RectI) -> PointF {
    PointF::new(
        r.x as f32 + r.width as f32 / 2.0,
        r.y as f32 + r.height as f32 / 2.0,
    )
}

fn scroll_y(r: NodeRef) -> i32 {
    with_doc(|d| d.get(r.get().unwrap()).unwrap().scroll.y)
}

fn scroll_x(r: NodeRef) -> i32 {
    with_doc(|d| d.get(r.get().unwrap()).unwrap().scroll.x)
}

fn send(
    ui: &mut Ui,
    p: PointF,
    delta: (f32, f32),
    source: ScrollSource,
    phase: ScrollPhase,
    time: u32,
) {
    ui.handle_event(WindowEvent::Scroll {
        position: p,
        delta_x: delta.0,
        delta_y: delta.1,
        modifiers: Modifiers::default(),
        source,
        phase,
        time,
    });
    ui.update();
}

fn finger(ui: &mut Ui, p: PointF, dy: f32, phase: ScrollPhase, time: u32) {
    send(ui, p, (0.0, dy), ScrollSource::Touchpad, phase, time);
}

/// Fingers moving `dy` every 10 milliseconds, then leaving.
fn flick(ui: &mut Ui, p: PointF, dy: f32) {
    finger(ui, p, dy, ScrollPhase::Began, 1000);
    finger(ui, p, dy, ScrollPhase::Moved, 1010);
    finger(ui, p, dy, ScrollPhase::Moved, 1020);
    finger(ui, p, 0.0, ScrollPhase::Lifted { fling: true }, 1025);
}

/// Runs frames until inertia ends. Returns how many it took.
fn run_fling(ui: &mut Ui, frame: Duration) -> u32 {
    let start = Instant::now();
    for i in 1..2000 {
        ui.animate(start + frame * i);
        ui.update();
        if !ui.animating() {
            return i;
        }
    }
    panic!("inertia did not end");
}

fn rows(n: usize) -> Vec<Div> {
    (0..n)
        .map(|i| div().h(30.0).child(text(format!("row {i}"))))
        .collect()
}

/// A plain scroller filling the window.
fn scroller(n: usize) -> (Ui, NodeRef, PointF) {
    let view = node_ref();
    let ui = mount(move || {
        div()
            .h(pct(100.0))
            .overflow_y_scroll()
            .node_ref(view)
            .children(rows(n))
    });
    let p = center(view.rect().unwrap());
    (ui, view, p)
}

/// A short scroller at the top of a long one, and a point on it.
fn nested() -> (Ui, NodeRef, NodeRef, PointF) {
    let outer = node_ref();
    let inner = node_ref();
    let ui = mount(move || {
        div()
            .h(pct(100.0))
            .overflow_y_scroll()
            .node_ref(outer)
            .child(
                div()
                    .h(120.0)
                    .overflow_y_scroll()
                    .node_ref(inner)
                    .children(rows(6)),
            )
            .children(rows(60))
    });
    let p = center(inner.rect().unwrap());
    (ui, outer, inner, p)
}

// ---------------------------------------------------------------------------------------------
// Fractions of a pixel

#[test]
fn slow_motion_adds_up() {
    let (mut ui, view, p) = scroller(100);
    finger(&mut ui, p, 0.3, ScrollPhase::Began, 0);
    for i in 1..10 {
        finger(&mut ui, p, 0.3, ScrollPhase::Moved, i);
    }
    assert_eq!(scroll_y(view), 3);
    for i in 10..20 {
        finger(&mut ui, p, -0.3, ScrollPhase::Moved, i);
    }
    assert_eq!(scroll_y(view), 0);
}

#[test]
fn slow_wheels_add_up_too() {
    let (mut ui, view, p) = scroller(100);
    for _ in 0..10 {
        send(
            &mut ui,
            p,
            (0.0, 0.4),
            ScrollSource::Unknown,
            ScrollPhase::Single,
            0,
        );
    }
    assert_eq!(scroll_y(view), 4);
}

#[test]
fn a_fraction_of_a_pixel_does_not_leak_to_the_outer_scroller() {
    let (mut ui, outer, inner, p) = nested();
    send(
        &mut ui,
        p,
        (0.0, 0.2),
        ScrollSource::Unknown,
        ScrollPhase::Single,
        0,
    );
    finger(&mut ui, p, 0.2, ScrollPhase::Began, 0);
    finger(&mut ui, p, 0.2, ScrollPhase::Moved, 1);
    assert_eq!(scroll_y(outer), 0);
    assert_eq!(scroll_y(inner), 1);
}

#[test]
fn fractions_are_dropped_at_the_ends() {
    let (mut ui, view, p) = scroller(100);
    finger(&mut ui, p, 0.4, ScrollPhase::Began, 0);
    finger(&mut ui, p, -5.0, ScrollPhase::Moved, 1);
    // Held against the top, so what was left over must not count towards the next motion.
    finger(&mut ui, p, 0.3, ScrollPhase::Moved, 2);
    assert_eq!(scroll_y(view), 0);
    finger(&mut ui, p, 0.3, ScrollPhase::Moved, 3);
    assert_eq!(scroll_y(view), 1);
}

// ---------------------------------------------------------------------------------------------
// Gestures stay on what they started on

#[test]
fn a_gesture_stays_on_its_scroller_when_another_comes_under_the_pointer() {
    let outer = node_ref();
    let inner = node_ref();
    let mut ui = mount(move || {
        div()
            .h(pct(100.0))
            .overflow_y_scroll()
            .node_ref(outer)
            .children(rows(5))
            .child(
                div()
                    .h(200.0)
                    .overflow_y_scroll()
                    .node_ref(inner)
                    .children(rows(40)),
            )
            .children(rows(40))
    });
    let p = PointF::new(100.0, 20.0);
    finger(&mut ui, p, 10.0, ScrollPhase::Began, 0);
    for i in 1..20 {
        finger(&mut ui, p, 10.0, ScrollPhase::Moved, i * 10);
    }
    assert_eq!(scroll_y(outer), 200);
    assert_eq!(scroll_y(inner), 0);
    // It is under the pointer now, so the next gesture goes there.
    finger(&mut ui, p, 0.0, ScrollPhase::Lifted { fling: false }, 500);
    finger(&mut ui, p, 10.0, ScrollPhase::Began, 600);
    assert_eq!(scroll_y(outer), 200);
    assert_eq!(scroll_y(inner), 10);
}

#[test]
fn inertia_stays_on_its_scroller_when_another_comes_under_the_pointer() {
    let outer = node_ref();
    let inner = node_ref();
    let mut ui = mount(move || {
        div()
            .h(pct(100.0))
            .overflow_y_scroll()
            .node_ref(outer)
            .children(rows(5))
            .child(
                div()
                    .h(200.0)
                    .overflow_y_scroll()
                    .node_ref(inner)
                    .children(rows(40)),
            )
            .children(rows(400))
    });
    flick(&mut ui, PointF::new(100.0, 20.0), 30.0);
    run_fling(&mut ui, FRAME);
    assert!(scroll_y(outer) > 200, "{}", scroll_y(outer));
    assert_eq!(scroll_y(inner), 0);
}

#[test]
fn reaching_the_end_does_not_pass_the_gesture_on() {
    let (mut ui, outer, inner, p) = nested();
    finger(&mut ui, p, 100.0, ScrollPhase::Began, 0);
    for i in 1..10 {
        finger(&mut ui, p, 100.0, ScrollPhase::Moved, i * 10);
    }
    assert_eq!(scroll_y(inner), 60);
    assert_eq!(scroll_y(outer), 0);
    // A new one goes outward, because the inner scroller can't go further that way.
    finger(&mut ui, p, 0.0, ScrollPhase::Lifted { fling: false }, 200);
    finger(&mut ui, p, 50.0, ScrollPhase::Began, 300);
    assert_eq!(scroll_y(inner), 60);
    assert_eq!(scroll_y(outer), 50);
    // And keeps going there, even going back.
    finger(&mut ui, p, -20.0, ScrollPhase::Moved, 310);
    assert_eq!(scroll_y(inner), 60);
    assert_eq!(scroll_y(outer), 30);
}

#[test]
fn wheels_choose_for_every_event() {
    let (mut ui, outer, inner, p) = nested();
    send(
        &mut ui,
        p,
        (0.0, 200.0),
        ScrollSource::Unknown,
        ScrollPhase::Single,
        0,
    );
    assert_eq!((scroll_y(inner), scroll_y(outer)), (60, 0));
    send(
        &mut ui,
        p,
        (0.0, 50.0),
        ScrollSource::Unknown,
        ScrollPhase::Single,
        0,
    );
    assert_eq!((scroll_y(inner), scroll_y(outer)), (60, 50));
}

#[test]
fn a_wheel_is_not_a_gesture_for_what_follows() {
    let (mut ui, outer, inner, p) = nested();
    send(
        &mut ui,
        p,
        (0.0, 10.0),
        ScrollSource::Unknown,
        ScrollPhase::Single,
        0,
    );
    assert_eq!((scroll_y(inner), scroll_y(outer)), (10, 0));
    // Fingers somewhere else start from there, not from where the wheel was.
    let elsewhere = PointF::new(p.x, 300.0);
    finger(&mut ui, elsewhere, 10.0, ScrollPhase::Moved, 10);
    assert_eq!((scroll_y(inner), scroll_y(outer)), (10, 10));
}

#[test]
fn a_gesture_without_a_beginning_still_works() {
    let (mut ui, view, p) = scroller(100);
    finger(&mut ui, p, 20.0, ScrollPhase::Moved, 0);
    assert_eq!(scroll_y(view), 20);
}

#[test]
fn shift_scrolls_sideways_for_the_whole_gesture() {
    let view = node_ref();
    let mut ui = mount(move || {
        div()
            .w(pct(100.0))
            .h(pct(100.0))
            .overflow_scroll()
            .flex()
            .node_ref(view)
            .children(
                (0..100)
                    .map(|_| div().w(60.0).h(30.0).flex_shrink(0.0))
                    .collect::<Vec<_>>(),
            )
    });
    let p = center(view.rect().unwrap());
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };
    for (dy, phase, time) in [
        (30.0, ScrollPhase::Began, 1000),
        (30.0, ScrollPhase::Moved, 1010),
        (30.0, ScrollPhase::Moved, 1020),
        (0.0, ScrollPhase::Lifted { fling: true }, 1025),
    ] {
        ui.handle_event(WindowEvent::Scroll {
            position: p,
            delta_x: 0.0,
            delta_y: dy,
            modifiers: shift,
            source: ScrollSource::Touchpad,
            phase,
            time,
        });
    }
    ui.update();
    assert_eq!(scroll_x(view), 90);
    // Letting go of Shift doesn't send what is left of the motion the other way.
    ui.handle_event(WindowEvent::Key(KeyEvent {
        key: Key::Named(NamedKey::Shift),
        text: None,
        modifiers: Modifiers::default(),
        pressed: false,
        repeat: false,
    }));
    assert!(ui.animating());
    run_fling(&mut ui, FRAME);
    assert!(scroll_x(view) > 90, "{}", scroll_x(view));
}

// ---------------------------------------------------------------------------------------------
// Inertia

#[test]
fn scrolling_goes_on_after_the_fingers_leave() {
    let (mut ui, view, p) = scroller(1000);
    flick(&mut ui, p, 30.0);
    assert_eq!(scroll_y(view), 90);
    assert!(ui.animating());
    // This is what starts the frames: nothing else has changed since the last one.
    let _ = ui.paint(Instant::now());
    assert!(ui.needs_redraw());

    let start = Instant::now();
    let mut last = 90;
    let mut last_step = i32::MAX / 2;
    for i in 1..200 {
        ui.animate(start + FRAME * i);
        ui.update();
        let y = scroll_y(view);
        assert!(y >= last, "went backwards");
        // One pixel of slack for rounding.
        assert!(y - last <= last_step + 1, "sped up at frame {i}");
        last_step = y - last;
        last = y;
        if !ui.animating() {
            break;
        }
    }
    assert!(!ui.animating());
    // 3000 px/s of content motion comes from 1200 px/s of axis motion: 272 pixels.
    assert!((last - 90 - 272).abs() < 2, "{}", last - 90);
    let _ = ui.paint(start);
    assert!(!ui.needs_redraw(), "must go idle when it ends");
}

#[test]
fn inertia_goes_the_same_way_at_any_frame_rate() {
    let mut ends = Vec::new();
    for ms in [4, 16, 33, 100] {
        let (mut ui, view, p) = scroller(1000);
        flick(&mut ui, p, 30.0);
        run_fling(&mut ui, Duration::from_millis(ms));
        ends.push(scroll_y(view));
    }
    let (min, max) = (ends.iter().min().unwrap(), ends.iter().max().unwrap());
    assert!(max - min <= 1, "{ends:?}");
}

#[test]
fn inertia_stops_at_the_end_without_passing_on() {
    let (mut ui, outer, inner, p) = nested();
    flick(&mut ui, p, 15.0);
    assert_eq!(scroll_y(inner), 45);
    let frames = run_fling(&mut ui, FRAME);
    assert_eq!(scroll_y(inner), 60);
    assert_eq!(scroll_y(outer), 0);
    // Stops right there instead of running out the curve with nothing to show for it.
    assert!(frames < 10, "{frames} frames");
}

#[test]
fn stopping_before_lifting_has_no_inertia() {
    let (mut ui, _, p) = scroller(1000);
    finger(&mut ui, p, 30.0, ScrollPhase::Began, 1000);
    finger(&mut ui, p, 30.0, ScrollPhase::Moved, 1010);
    finger(&mut ui, p, 30.0, ScrollPhase::Moved, 1020);
    finger(&mut ui, p, 0.0, ScrollPhase::Lifted { fling: true }, 1221);
    assert!(!ui.animating());
}

#[test]
fn slow_motion_has_no_inertia() {
    let (mut ui, _, p) = scroller(1000);
    flick(&mut ui, p, 0.1);
    assert!(!ui.animating());
}

#[test]
fn nothing_to_scroll_means_no_frames() {
    let mut ui = mount(|| div().h(pct(100.0)).children(rows(3)));
    flick(&mut ui, PointF::new(100.0, 20.0), 30.0);
    if ui.animating() {
        ui.animate(Instant::now() + FRAME);
    }
    assert!(!ui.animating());
}

#[test]
fn text_areas_have_inertia_too() {
    let area = node_ref();
    let mut ui = mount(move || {
        div().child(
            text_area()
                .h(100.0)
                .w(300.0)
                .node_ref(area)
                .value(signal("line\n".repeat(200))),
        )
    });
    let edit_scroll =
        move || with_doc(|d| d.get(area.get().unwrap()).unwrap().edit().unwrap().scroll.y);
    let p = center(area.rect().unwrap());
    finger(&mut ui, p, 0.4, ScrollPhase::Began, 0);
    finger(&mut ui, p, 0.4, ScrollPhase::Moved, 1);
    assert_eq!(edit_scroll(), 1);
    flick(&mut ui, p, 30.0);
    run_fling(&mut ui, FRAME);
    assert!(edit_scroll() > 300, "{}", edit_scroll());
}

#[test]
fn the_scroller_going_away_ends_inertia() {
    let shown = signal(true);
    let view = node_ref();
    let mut ui = mount(move || {
        div().h(pct(100.0)).child(show(
            move || shown.get(),
            move || {
                div()
                    .h(pct(100.0))
                    .overflow_y_scroll()
                    .node_ref(view)
                    .children(rows(1000))
            },
        ))
    });
    flick(&mut ui, PointF::new(100.0, 100.0), 30.0);
    assert!(ui.animating());
    shown.set(false);
    ui.update();
    ui.animate(Instant::now() + FRAME);
    assert!(!ui.animating());
}

#[test]
fn things_that_stop_inertia() {
    let m = Modifiers::default();
    type Stop = Box<dyn Fn(&mut Ui, PointF)>;
    let stops: Vec<(&str, Stop)> = vec![
        (
            "fingers moving again",
            Box::new(|ui, p| finger(ui, p, 1.0, ScrollPhase::Began, 2000)),
        ),
        (
            "fingers moving without a beginning",
            Box::new(|ui, p| finger(ui, p, 1.0, ScrollPhase::Moved, 2000)),
        ),
        (
            "fingers resting",
            Box::new(|ui, p| finger(ui, p, 0.0, ScrollPhase::Held, 2000)),
        ),
        (
            "a wheel",
            Box::new(|ui, p| {
                send(
                    ui,
                    p,
                    (0.0, 0.0),
                    ScrollSource::Wheel,
                    ScrollPhase::Single,
                    0,
                )
            }),
        ),
        (
            "a button",
            Box::new(move |ui, p| {
                ui.handle_event(WindowEvent::PointerButton {
                    position: p,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: m,
                    time: 2000,
                })
            }),
        ),
        (
            "a key",
            Box::new(move |ui, _| {
                ui.handle_event(WindowEvent::Key(KeyEvent {
                    key: Key::Named(NamedKey::ArrowDown),
                    text: None,
                    modifiers: m,
                    pressed: true,
                    repeat: false,
                }))
            }),
        ),
        (
            "the pointer moving away",
            Box::new(move |ui, p| {
                ui.handle_event(WindowEvent::PointerMoved {
                    position: PointF::new(p.x + 9.0, p.y),
                    modifiers: m,
                })
            }),
        ),
        (
            "the pointer leaving",
            Box::new(|ui, _| ui.handle_event(WindowEvent::PointerLeft)),
        ),
    ];
    for (what, stop) in stops {
        let (mut ui, view, p) = scroller(1000);
        flick(&mut ui, p, 30.0);
        assert!(ui.animating(), "{what}");
        stop(&mut ui, p);
        assert!(!ui.animating(), "{what} did not stop it");
        let y = scroll_y(view);
        ui.animate(Instant::now() + Duration::from_millis(100));
        ui.update();
        assert_eq!(scroll_y(view), y, "{what}");
    }
}

#[test]
fn things_that_do_not_stop_inertia() {
    let m = Modifiers::default();
    let (mut ui, _, p) = scroller(1000);
    flick(&mut ui, p, 30.0);
    // The last finger leaving can move the pointer a little.
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(p.x + 7.0, p.y),
        modifiers: m,
    });
    assert!(ui.animating());
    // Letting go of a key that was held.
    ui.handle_event(WindowEvent::Key(KeyEvent {
        key: Key::Named(NamedKey::ArrowDown),
        text: None,
        modifiers: m,
        pressed: false,
        repeat: false,
    }));
    assert!(ui.animating());
}

#[test]
fn the_slop_for_pointer_motion_is_in_logical_pixels() {
    let view = node_ref();
    let mut ui = mount_at(240, move || {
        div()
            .h(pct(100.0))
            .overflow_y_scroll()
            .node_ref(view)
            .children(rows(1000))
    });
    let p = center(view.rect().unwrap());
    flick(&mut ui, p, 30.0);
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(p.x + 15.0, p.y),
        modifiers: Modifiers::default(),
    });
    assert!(ui.animating());
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(p.x + 17.0, p.y),
        modifiers: Modifiers::default(),
    });
    assert!(!ui.animating());
}

#[test]
fn selecting_with_the_button_held_has_no_inertia() {
    let (mut ui, _, p) = scroller(1000);
    ui.handle_event(WindowEvent::PointerButton {
        position: p,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::default(),
        time: 0,
    });
    flick(&mut ui, p, 30.0);
    assert!(!ui.animating());
}

#[test]
fn scale_does_not_change_how_far_it_feels() {
    // The same logical motion at a different scale.
    let far = |scale: u32, f: f32| {
        let view = node_ref();
        let mut ui = mount_at(scale, move || {
            div()
                .h(pct(100.0))
                .overflow_y_scroll()
                .node_ref(view)
                .children(rows(1000))
        });
        flick(&mut ui, PointF::new(100.0, 100.0), 30.0 * f);
        run_fling(&mut ui, FRAME);
        scroll_y(view) as f32 / f
    };
    let (a, b) = (far(120, 1.0), far(216, 1.8));
    assert!((a - b).abs() < 15.0, "{a} vs {b}");
}

// ---------------------------------------------------------------------------------------------
// Against Chrome 155 with its newer touchpad scaling (see docs/design/08-scrolling-and-settings.md)

/// How far Chrome moves content per unit of axis in that scaling.
const CHROME_GAIN: f32 = 2.5;

/// Axis motion scaled like in Chrome, every 8 milliseconds, then fingers leaving.
fn axis_flick(ui: &mut Ui, p: PointF, v: f32, frames: u32, time: &mut u32) {
    axis_flick_at(ui, p, v * CHROME_GAIN, frames, time);
}

/// Content motion of `speed` pixels per second, every 8 milliseconds, then fingers leaving.
fn axis_flick_at(ui: &mut Ui, p: PointF, speed: f32, frames: u32, time: &mut u32) {
    let dy = speed * 0.008;
    finger(ui, p, dy, ScrollPhase::Began, *time);
    for _ in 1..frames {
        *time += 8;
        finger(ui, p, dy, ScrollPhase::Moved, *time);
    }
    *time += 8;
    finger(ui, p, 0.0, ScrollPhase::Lifted { fling: true }, *time);
}

fn chrome_sized() -> (Ui, NodeRef, PointF) {
    let view = node_ref();
    let mut ui = Ui::new(move || {
        div()
            .h(pct(100.0))
            .overflow_y_scroll()
            .node_ref(view)
            .children(rows(5000))
    });
    update_settings(|s| *s = Settings::default());
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(800, 467),
        scale: Scale(120),
    });
    ui.update();
    let p = center(view.rect().unwrap());
    (ui, view, p)
}

#[test]
fn single_flicks_go_as_far_as_in_chrome() {
    for (v, total) in [
        (200, 107),
        (500, 297),
        (1000, 689),
        (2000, 1752),
        (3000, 2900),
        (5000, 3901),
        (8000, 5401),
        (12000, 7400),
    ] {
        let (mut ui, view, p) = chrome_sized();
        let mut time = 1000;
        axis_flick(&mut ui, p, v as f32, 25, &mut time);
        run_fling(&mut ui, FRAME);
        let y = scroll_y(view);
        assert!((y - total).abs() <= 5, "{v} px/s: {y} vs {total}");
    }
}

#[test]
fn flicks_at_our_scaling_go_further_than_at_chromes() {
    for (v, total) in [(500, 689), (1000, 1756), (2000, 3401), (12000, 13401)] {
        let (mut ui, view, p) = chrome_sized();
        let mut time = 1000;
        axis_flick_at(&mut ui, p, v as f32 * AXIS_MOTION_GAIN, 25, &mut time);
        run_fling(&mut ui, FRAME);
        let y = scroll_y(view);
        assert!((y - total).abs() <= 5, "{v} px/s: {y} vs {total}");
    }
}

#[test]
fn sideways_flicks_go_as_far_as_vertical_ones() {
    let view = node_ref();
    let mut ui = mount(move || {
        div()
            .w(pct(100.0))
            .h(pct(100.0))
            .overflow_scroll()
            .flex()
            .node_ref(view)
            .children(
                (0..200)
                    .map(|_| div().w(60.0).h(30.0).flex_shrink(0.0))
                    .collect::<Vec<_>>(),
            )
    });
    let p = center(view.rect().unwrap());
    let dx = 1000.0 * 0.008 * CHROME_GAIN;
    let source = ScrollSource::Touchpad;
    send(&mut ui, p, (dx, 0.0), source, ScrollPhase::Began, 1000);
    for i in 1..25 {
        send(
            &mut ui,
            p,
            (dx, 0.0),
            source,
            ScrollPhase::Moved,
            1000 + i * 8,
        );
    }
    let lifted = ScrollPhase::Lifted { fling: true };
    send(&mut ui, p, (0.0, 0.0), source, lifted, 1200);
    run_fling(&mut ui, FRAME);
    assert!((scroll_x(view) - 689).abs() <= 5, "{}", scroll_x(view));
}

#[test]
fn flicks_in_a_row_go_as_far_as_in_chrome() {
    // Three flicks of 96 milliseconds with a gap in between, as measured.
    for (v, gap, total) in [
        (2000.0, 260, 6928.0),
        // The inertia is over by the next flick, so nothing adds up.
        (1000.0, 660, 1284.0),
    ] {
        let (mut ui, view, p) = chrome_sized();
        let mut time = 1000;
        for flick in 0..3 {
            axis_flick(&mut ui, p, v, 12, &mut time);
            if flick < 2 {
                let start = Instant::now();
                for i in 1..=gap / 16 {
                    ui.animate(start + FRAME * i);
                    ui.update();
                }
                time += gap;
            }
        }
        if ui.animating() {
            run_fling(&mut ui, FRAME);
        }
        let y = scroll_y(view) as f32;
        assert!((y - total).abs() < total * 0.02, "{v} px/s: {y} vs {total}");
    }
}

#[test]
fn inertia_takes_over_from_the_last_motion() {
    // The fingers left 100 milliseconds after they last moved, so it is already on its way.
    let (mut ui, view, p) = chrome_sized();
    finger(&mut ui, p, 40.0, ScrollPhase::Began, 1000);
    finger(&mut ui, p, 40.0, ScrollPhase::Moved, 1008);
    finger(&mut ui, p, 40.0, ScrollPhase::Moved, 1016);
    finger(&mut ui, p, 0.0, ScrollPhase::Lifted { fling: true }, 1116);
    let y = scroll_y(view);
    ui.animate(Instant::now());
    ui.update();
    assert!(scroll_y(view) - y > 100, "{}", scroll_y(view) - y);
}

#[test]
fn inertia_reaches_three_windows_at_any_scale() {
    let view = node_ref();
    let mut ui = Ui::new(move || {
        div()
            .h(pct(100.0))
            .overflow_y_scroll()
            .node_ref(view)
            .children(rows(5000))
    });
    update_settings(|s| *s = Settings::default());
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(800, 400),
        scale: Scale(240),
    });
    ui.update();
    let p = center(view.rect().unwrap());
    let mut time = 1000;
    axis_flick(&mut ui, p, 50000.0, 3, &mut time);
    let y = scroll_y(view);
    run_fling(&mut ui, FRAME);
    assert!(
        (scroll_y(view) - y - 1200).abs() <= 1,
        "{}",
        scroll_y(view) - y
    );
}

/// How far a flick goes after the fingers came back during inertia, moving like `motion`.
fn second_flick(prior: bool, motion: &[(u32, f32)]) -> i32 {
    let (mut ui, view, p) = chrome_sized();
    let mut time = 1000;
    if prior {
        axis_flick(&mut ui, p, 2000.0, 12, &mut time);
        ui.animate(Instant::now() + FRAME);
        ui.update();
    }
    let y = scroll_y(view);
    for (i, (dt, dy)) in motion.iter().enumerate() {
        time += dt;
        let phase = if i == 0 {
            ScrollPhase::Began
        } else {
            ScrollPhase::Moved
        };
        finger(&mut ui, p, *dy, phase, time);
    }
    time += 8;
    finger(&mut ui, p, 0.0, ScrollPhase::Lifted { fling: true }, time);
    run_fling(&mut ui, FRAME);
    scroll_y(view) - y
}

#[test]
fn fingers_moving_slowly_at_first_do_not_add_up() {
    let fast = [(8, 40.0), (8, 40.0), (8, 40.0), (8, 40.0)];
    assert!(second_flick(true, &fast) > second_flick(false, &fast) + 500);
    // Slow for a moment on the way: under 150 pixels per second.
    let hesitant = [(8, 40.0), (8, 0.5), (8, 40.0), (8, 40.0), (8, 40.0)];
    assert_eq!(
        second_flick(true, &hesitant),
        second_flick(false, &hesitant)
    );
}

#[test]
fn a_flick_too_late_does_not_add_up() {
    let (mut ui, view, p) = chrome_sized();
    let mut time = 1000;
    axis_flick(&mut ui, p, 2000.0, 12, &mut time);
    let start = Instant::now();
    for i in 1..=5 {
        ui.animate(start + FRAME * i);
        ui.update();
    }
    // The fling ended on its own before the next one.
    run_fling(&mut ui, FRAME);
    time += 2000;
    let y = scroll_y(view);
    axis_flick(&mut ui, p, 2000.0, 12, &mut time);
    run_fling(&mut ui, FRAME);
    assert!(
        (scroll_y(view) - y - 1236).abs() < 5,
        "{}",
        scroll_y(view) - y
    );
}

#[test]
fn stopping_a_fling_by_other_means_does_not_add_up() {
    let (mut ui, view, p) = chrome_sized();
    let mut time = 1000;
    axis_flick(&mut ui, p, 2000.0, 12, &mut time);
    ui.handle_event(WindowEvent::PointerButton {
        position: p,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::default(),
        time,
    });
    ui.handle_event(WindowEvent::PointerButton {
        position: p,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::default(),
        time,
    });
    let y = scroll_y(view);
    time += 8;
    axis_flick(&mut ui, p, 2000.0, 12, &mut time);
    run_fling(&mut ui, FRAME);
    assert!(
        (scroll_y(view) - y - 1236).abs() < 5,
        "{}",
        scroll_y(view) - y
    );
}

// ---------------------------------------------------------------------------------------------
// Inertia from the platform

fn lifted_by_platform(ui: &mut Ui, p: PointF) {
    finger(ui, p, 30.0, ScrollPhase::Began, 1000);
    finger(ui, p, 30.0, ScrollPhase::Moved, 1010);
    finger(ui, p, 0.0, ScrollPhase::Lifted { fling: false }, 1020);
}

#[test]
fn platform_inertia_is_followed_not_computed() {
    let (mut ui, view, p) = scroller(1000);
    lifted_by_platform(&mut ui, p);
    assert!(!ui.animating());
    assert_eq!(scroll_y(view), 60);
    finger(&mut ui, p, 20.0, ScrollPhase::Momentum, 1030);
    finger(&mut ui, p, 0.4, ScrollPhase::Momentum, 1040);
    finger(&mut ui, p, 0.4, ScrollPhase::Momentum, 1050);
    assert_eq!(scroll_y(view), 81);
    finger(&mut ui, p, 0.0, ScrollPhase::MomentumEnded, 1060);
    finger(&mut ui, p, 20.0, ScrollPhase::Momentum, 1070);
    assert_eq!(scroll_y(view), 81);
}

#[test]
fn platform_inertia_stays_on_its_scroller() {
    let (mut ui, outer, inner, p) = nested();
    finger(&mut ui, p, 50.0, ScrollPhase::Began, 0);
    finger(&mut ui, p, 0.0, ScrollPhase::Lifted { fling: false }, 10);
    for i in 0..10 {
        finger(&mut ui, p, 50.0, ScrollPhase::Momentum, 20 + i);
    }
    assert_eq!((scroll_y(inner), scroll_y(outer)), (60, 0));
}

#[test]
fn stopped_platform_inertia_stays_stopped() {
    let (mut ui, view, p) = scroller(1000);
    lifted_by_platform(&mut ui, p);
    ui.handle_event(WindowEvent::PointerButton {
        position: p,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::default(),
        time: 1025,
    });
    ui.handle_event(WindowEvent::PointerButton {
        position: p,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::default(),
        time: 1026,
    });
    finger(&mut ui, p, 20.0, ScrollPhase::Momentum, 1030);
    assert_eq!(scroll_y(view), 60);
    // Until the next gesture.
    finger(&mut ui, p, 20.0, ScrollPhase::Began, 1040);
    assert_eq!(scroll_y(view), 80);
}

#[test]
fn inertia_without_a_gesture_is_ignored() {
    let (mut ui, view, p) = scroller(1000);
    finger(&mut ui, p, 20.0, ScrollPhase::Momentum, 0);
    assert_eq!(scroll_y(view), 0);
    // Nor while the fingers are still down.
    finger(&mut ui, p, 10.0, ScrollPhase::Began, 10);
    finger(&mut ui, p, 20.0, ScrollPhase::Momentum, 20);
    assert_eq!(scroll_y(view), 10);
}

// ---------------------------------------------------------------------------------------------
// Settings

#[test]
fn the_touchpad_factor_scales_touchpads_and_their_inertia() {
    let (mut ui, view, p) = scroller(1000);
    update_settings(|s| s.scroll.touchpad_factor = 2.0);
    finger(&mut ui, p, 10.0, ScrollPhase::Began, 0);
    assert_eq!(scroll_y(view), 20);
    finger(&mut ui, p, 0.0, ScrollPhase::Lifted { fling: false }, 10);
    finger(&mut ui, p, 10.0, ScrollPhase::Momentum, 20);
    assert_eq!(scroll_y(view), 40);
    send(
        &mut ui,
        p,
        (0.0, 10.0),
        ScrollSource::Unknown,
        ScrollPhase::Single,
        30,
    );
    assert_eq!(scroll_y(view), 50);

    // The speed inertia starts from includes it.
    let far = |factor: f32| {
        let (mut ui, view, p) = scroller(1000);
        update_settings(|s| s.scroll.touchpad_factor = factor);
        flick(&mut ui, p, 20.0);
        run_fling(&mut ui, FRAME);
        scroll_y(view)
    };
    assert!(far(2.0) as f32 > far(1.0) as f32 * 1.5);
}

#[test]
fn wheel_notches_become_the_wheel_step_in_physical_pixels() {
    let view = node_ref();
    let mut ui = mount_at(180, move || {
        div()
            .h(pct(100.0))
            .overflow_y_scroll()
            .node_ref(view)
            .children(rows(1000))
    });
    let p = center(view.rect().unwrap());
    send(
        &mut ui,
        p,
        (0.0, 1.0),
        ScrollSource::Wheel,
        ScrollPhase::Single,
        0,
    );
    assert_eq!(scroll_y(view), 72);
    update_settings(|s| s.scroll.wheel_step = 10.0);
    send(
        &mut ui,
        p,
        (0.0, 2.0),
        ScrollSource::Wheel,
        ScrollPhase::Single,
        0,
    );
    assert_eq!(scroll_y(view), 102);
}

#[test]
fn inertia_can_be_turned_off() {
    let (mut ui, view, p) = scroller(1000);
    update_settings(|s| s.scroll.kinetic.enabled = false);
    flick(&mut ui, p, 30.0);
    assert!(!ui.animating());
    lifted_by_platform(&mut ui, p);
    let y = scroll_y(view);
    finger(&mut ui, p, 20.0, ScrollPhase::Momentum, 1030);
    assert_eq!(scroll_y(view), y);
}

#[test]
fn kinetic_settings_shape_inertia() {
    let far = |f: &dyn Fn(&mut Settings)| {
        let (mut ui, view, p) = scroller(5000);
        update_settings(f);
        flick(&mut ui, p, 30.0);
        let frames = run_fling(&mut ui, FRAME);
        (scroll_y(view), frames)
    };
    let base = far(&|_| {});
    let fast = far(&|s| s.scroll.kinetic.velocity_factor = 2.0);
    assert!(fast.0 as f32 > base.0 as f32 * 1.5, "{fast:?} vs {base:?}");
    let slippery = far(&|s| s.scroll.kinetic.deceleration = 1300.0);
    assert!(slippery.0 > base.0, "{slippery:?} vs {base:?}");
    assert!(slippery.1 > base.1, "{slippery:?} vs {base:?}");
}

#[test]
fn settings_are_reactive() {
    let _ui = mount(div);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let s = seen.clone();
    let (_, owner) = create_root(move || {
        effect(move || s.borrow_mut().push(settings().scroll.touchpad_factor));
    });
    update_settings(|s| s.scroll.touchpad_factor = 3.0);
    assert_eq!(*seen.borrow(), [1.0, 3.0]);
    owner.dispose();
}

#[test]
fn settings_survive_whoever_read_them_first() {
    let (_, owner) = create_root(settings);
    owner.dispose();
    update_settings(|s| s.scroll.wheel_step = 5.0);
    assert_eq!(settings().scroll.wheel_step, 5.0);
    update_settings(|s| *s = Settings::default());
}

// ---------------------------------------------------------------------------------------------
// Handlers

#[test]
fn handlers_see_the_whole_gesture() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let log = seen.clone();
    let view = node_ref();
    let mut ui = mount(move || {
        div()
            .h(pct(100.0))
            .overflow_y_scroll()
            .node_ref(view)
            .on_scroll(move |e| log.borrow_mut().push((e.phase, e.source, e.delta_y)))
            .children(rows(1000))
    });
    let p = center(view.rect().unwrap());
    update_settings(|s| s.scroll.touchpad_factor = 2.0);
    flick(&mut ui, p, 15.0);
    run_fling(&mut ui, FRAME);
    let seen = seen.borrow();
    assert_eq!(seen[0], (ScrollPhase::Began, ScrollSource::Touchpad, 30.0));
    assert_eq!(seen[1], (ScrollPhase::Moved, ScrollSource::Touchpad, 30.0));
    assert_eq!(seen[2], (ScrollPhase::Moved, ScrollSource::Touchpad, 30.0));
    assert_eq!(
        seen[3],
        (
            ScrollPhase::Lifted { fling: true },
            ScrollSource::Touchpad,
            0.0
        )
    );
    let last = seen.len() - 1;
    assert!(last > 10);
    assert!(seen[4..last].iter().all(|e| e.0 == ScrollPhase::Momentum));
    assert_eq!(
        seen[last],
        (ScrollPhase::MomentumEnded, ScrollSource::Touchpad, 0.0)
    );
    // The handler sees the same motion the content makes.
    let sum: f32 = seen.iter().map(|e| e.2).sum();
    assert!((sum - scroll_y(view) as f32).abs() <= 1.0, "{sum}");
}

#[test]
fn stopping_inertia_is_reported() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let log = seen.clone();
    let view = node_ref();
    let mut ui = mount(move || {
        div()
            .h(pct(100.0))
            .overflow_y_scroll()
            .node_ref(view)
            .on_scroll(move |e| log.borrow_mut().push(e.phase))
            .children(rows(1000))
    });
    let p = center(view.rect().unwrap());
    flick(&mut ui, p, 30.0);
    finger(&mut ui, p, 0.0, ScrollPhase::Held, 2000);
    assert_eq!(*seen.borrow().last().unwrap(), ScrollPhase::MomentumEnded);
    assert!(!seen.borrow().contains(&ScrollPhase::Held));
}

#[test]
fn a_handler_taking_the_motion_gets_all_of_the_inertia() {
    // A map panning by itself: nothing scrolls, and inertia must not stop for that.
    let panned = signal(0.0_f32);
    let ended = signal(false);
    let map = node_ref();
    let mut ui = mount(move || {
        div()
            .h(pct(100.0))
            .node_ref(map)
            .on_scroll(move |e| {
                panned.update(|p| *p += e.delta_y);
                if e.phase == ScrollPhase::MomentumEnded {
                    ended.set(true);
                }
                e.stop_propagation();
            })
            .children(rows(3))
    });
    flick(&mut ui, center(map.rect().unwrap()), 30.0);
    run_fling(&mut ui, FRAME);
    assert!(ended.get());
    // 90 pixels by hand and the rest of the curve.
    assert!((panned.get() - 362.0).abs() < 2.0, "{}", panned.get());
}
