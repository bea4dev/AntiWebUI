//! Portals, timers and reports of scrolling. See `docs/design/20-md3e-components.md`.

use anti_webui_core::Ui;
use anti_webui_core::event::{
    Key, KeyEvent, Modifiers, NamedKey, PointerButton, ScrollPhase, ScrollSource, WindowEvent,
};
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::Primitive;
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

fn start(view: impl FnOnce() -> Div + 'static) -> Ui {
    let mut ui = Ui::new(view);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(400, 300),
        scale: Scale::ONE,
    });
    ui.update();
    ui
}

fn click(ui: &mut Ui, x: f32, y: f32) {
    let position = PointF::new(x, y);
    ui.handle_event(WindowEvent::PointerMoved {
        position,
        modifiers: Modifiers::default(),
    });
    for pressed in [true, false] {
        ui.handle_event(WindowEvent::PointerButton {
            position,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
            time: 0,
        });
    }
    ui.update();
}

fn tab(ui: &mut Ui) {
    ui.handle_event(WindowEvent::Key(KeyEvent {
        key: Key::Named(NamedKey::Tab),
        text: None,
        modifiers: Modifiers::default(),
        pressed: true,
        repeat: false,
    }));
    ui.update();
}

fn counter() -> (Rc<Cell<u32>>, impl Fn(&PointerEvent) + 'static) {
    let count = Rc::new(Cell::new(0));
    let c = count.clone();
    (count, move |_: &PointerEvent| c.set(c.get() + 1))
}

#[test]
fn a_portal_leaves_what_clips_it_and_is_painted_above_the_window() {
    let color = rgb(0x123456);
    let mut ui = start(move || {
        div().size_full().bg(rgb(0xffffff)).child(
            div().w(10.0).h(10.0).overflow_hidden().child(
                portal().child(
                    div()
                        .absolute()
                        .left(100.0)
                        .top(50.0)
                        .w(80.0)
                        .h(40.0)
                        .bg(color),
                ),
            ),
        )
    });
    let scene = ui.paint(Instant::now());
    let last = scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Quad(q) => Some(q),
            _ => None,
        })
        .next_back()
        .expect("what the portal holds");
    assert_eq!(last.background, color.snapped());
    assert_eq!(
        (last.rect.x, last.rect.y, last.rect.width, last.rect.height),
        (100, 50, 80, 40)
    );
}

#[test]
fn the_pointer_goes_through_what_a_portal_does_not_cover() {
    let (below, on_below) = counter();
    let (above, on_above) = counter();
    let mut ui = start(move || {
        // Next to it rather than around it: what goes to a portal also bubbles to its ancestors.
        div()
            .size_full()
            .child(div().size_full().on_click(on_below))
            .child(
                portal().child(
                    div()
                        .absolute()
                        .left(0.0)
                        .top(0.0)
                        .w(50.0)
                        .h(50.0)
                        .on_click(on_above),
                ),
            )
    });
    click(&mut ui, 20.0, 20.0);
    assert_eq!((below.get(), above.get()), (0, 1));
    click(&mut ui, 200.0, 200.0);
    assert_eq!((below.get(), above.get()), (1, 1));
}

#[test]
fn a_modal_portal_keeps_tab_inside_and_gives_the_focus_back() {
    let open = signal(false);
    let focused = signal(0);
    let item = move |i: i32| {
        div()
            .w(10.0)
            .h(10.0)
            .focusable()
            .on_focus_change(move |on| {
                if on {
                    focused.set(i);
                }
            })
    };
    let mut ui = start(move || {
        div().size_full().child(item(1)).child(item(2)).child(show(
            move || open.get(),
            move || portal().modal().child(item(3)).child(item(4)),
        ))
    });
    tab(&mut ui);
    assert_eq!(focused.get(), 1);
    open.set(true);
    ui.update();
    tab(&mut ui);
    assert_eq!(focused.get(), 3);
    tab(&mut ui);
    assert_eq!(focused.get(), 4);
    tab(&mut ui);
    assert_eq!(focused.get(), 3, "it goes around");
    open.set(false);
    ui.update();
    tab(&mut ui);
    assert_eq!(focused.get(), 2, "from where it was before");
}

#[test]
fn a_timer_runs_once_when_it_is_due_and_can_be_cancelled() {
    let ran = Rc::new(Cell::new(0));
    let mut ui = start(div);
    let r = ran.clone();
    set_timeout(Duration::from_millis(100), move || r.set(r.get() + 1));
    let r = ran.clone();
    set_timeout(Duration::from_millis(100), move || r.set(r.get() + 10)).cancel();
    let now = Instant::now();
    let due = ui.next_deadline(now).expect("a deadline");
    assert!(due > now && due <= now + Duration::from_millis(100));
    ui.tick(now);
    assert_eq!(ran.get(), 0);
    ui.tick(now + Duration::from_millis(200));
    assert_eq!(ran.get(), 1);
    ui.tick(now + Duration::from_millis(400));
    assert_eq!(ran.get(), 1);
    assert!(ui.next_deadline(now).is_none());
}

#[test]
fn a_timer_goes_with_its_owner() {
    let ran = Rc::new(Cell::new(false));
    let r = ran.clone();
    let mut ui = start(div);
    let ((), owner) = create_root(move || {
        set_timeout(Duration::ZERO, move || r.set(true));
    });
    owner.dispose();
    ui.tick(Instant::now() + Duration::from_secs(1));
    assert!(!ran.get());
}

#[test]
fn what_scrolled_is_reported_however_it_happened() {
    let at = signal(PointF::default());
    let scroller = node_ref();
    let mut ui = start(move || {
        div()
            .w(100.0)
            .h(100.0)
            .overflow_y_scroll()
            .node_ref(scroller)
            .on_scrolled(move |p| at.set(p))
            .child(div().w(100.0).h(1000.0))
    });
    ui.handle_event(WindowEvent::Scroll {
        position: PointF::new(50.0, 50.0),
        delta_x: 0.0,
        delta_y: 30.0,
        modifiers: Modifiers::default(),
        source: ScrollSource::Unknown,
        phase: ScrollPhase::Single,
        time: 0,
    });
    ui.update();
    assert_eq!(at.get().y, 30.0);
    assert_eq!(scroller.scroll_offset().y, 30.0);
    scroller.scroll_to(0.0, 500.0);
    ui.update();
    assert_eq!(at.get().y, 500.0);
    scroller.scroll_to(0.0, 5000.0);
    ui.update();
    assert_eq!(at.get().y, 900.0, "as far as it goes");
}

#[test]
fn a_new_size_is_reported_before_anything_is_painted() {
    let size = signal((0.0f32, 0.0f32));
    let wide = signal(false);
    let mut ui = start(move || {
        div().child(
            div()
                .h(10.0)
                .dyn_style(move |s| s.w(if wide.get() { 200.0 } else { 100.0 }))
                .on_resized(move |w, h| size.set((w, h))),
        )
    });
    assert_eq!(size.get(), (100.0, 10.0));
    wide.set(true);
    ui.update();
    assert_eq!(size.get(), (200.0, 10.0));
}
