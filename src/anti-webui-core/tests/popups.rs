//! How popups come and go, seen from the platform's side of the core.

use anti_webui_core::Ui;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, PointI, RectI, Scale, SizeI};
use anti_webui_core::platform::{PlatformRequest, PopupId, PopupParams};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::Primitive;
use anti_webui_core::tree::with_doc;
use std::time::Instant;

const M: Modifiers = Modifiers {
    shift: false,
    ctrl: false,
    alt: false,
    logo: false,
};

fn mount<V: IntoView>(scale: u32, f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(600, 400),
        scale: Scale(scale),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    ui
}

fn press(ui: &mut Ui, x: f32, y: f32, pressed: bool) {
    ui.handle_event(WindowEvent::PointerButton {
        position: PointF::new(x, y),
        button: PointerButton::Primary,
        pressed,
        modifiers: M,
        time: 0,
    });
    ui.update();
}

fn click(ui: &mut Ui, x: f32, y: f32) {
    press(ui, x, y, true);
    press(ui, x, y, false);
}

fn key(ui: &mut Ui, named: NamedKey) {
    ui.handle_event(WindowEvent::Key(KeyEvent {
        key: Key::Named(named),
        text: None,
        modifiers: M,
        pressed: true,
        repeat: false,
    }));
    ui.update();
}

fn shows(reqs: &[PlatformRequest]) -> Vec<PopupParams> {
    reqs.iter()
        .filter_map(|r| match r {
            PlatformRequest::ShowPopup(p) => Some(p.clone()),
            _ => None,
        })
        .collect()
}

fn closes(reqs: &[PlatformRequest]) -> Vec<PopupId> {
    reqs.iter()
        .filter_map(|r| match r {
            PlatformRequest::ClosePopup(p) => Some(*p),
            _ => None,
        })
        .collect()
}

/// Plays the platform: the popup goes where `origin` says, at the size it asked for.
fn configure(ui: &mut Ui, p: &PopupParams, origin: (i32, i32)) {
    ui.handle_event(WindowEvent::PopupConfigured {
        id: p.id,
        origin: PointI::new(origin.0, origin.1),
        size: p.wanted,
    });
    ui.update();
}

fn opened(ui: &mut Ui, origin: (i32, i32)) -> PopupParams {
    ui.update();
    let p = shows(&ui.take_requests()).pop().expect("a popup to show");
    configure(ui, &p, origin);
    p
}

fn item(clicks: Signal<u32>, label: &'static str) -> Div {
    div()
        .w(100.0)
        .h(20.0)
        .bg(rgb(0xffffff))
        .on_click(move |_| clicks.update(|c| *c += 1))
        .child(text(label))
}

#[test]
fn a_popup_waits_for_its_size_and_place() {
    let mut ui = mount(180, || {
        div().w(100.0).h(50.0).child(
            popup()
                .gap(4)
                .child(div().w(101.0).h(40.0).bg(rgb(0xff0000))),
        )
    });
    let reqs = ui.take_requests();
    let p = &shows(&reqs)[0];
    assert_eq!(p.parent, None);
    assert_eq!(p.kind, PopupKind::Menu);
    assert_eq!(p.gap, 4);
    assert_eq!(p.anchor, RectI::new(0, 0, 150, 75));
    // 152 and 60 physical pixels at 1.5, in whole logical pixels.
    assert_eq!(p.wanted, SizeI::new(102, 40));
    // Nothing sticks out, so its surface is just the box.
    assert_eq!(p.geometry, RectI::new(0, 0, 102, 40));
    assert_eq!(p.logical, p.wanted);
    assert_eq!(p.buffer, SizeI::new(153, 60));

    let now = Instant::now();
    assert!(ui.paint_frame(now).popups.is_empty(), "not placed yet");
    configure(&mut ui, p, (0, 96));
    let frame = ui.paint_frame(now);
    assert_eq!(frame.popups.len(), 1);
    let scene = &frame.popups[0].1;
    assert_eq!((scene.width, scene.height), (153, 60));
    assert!(scene.clear.is_transparent());
    assert!(
        !frame.window.primitives.iter().any(|p| matches!(
            p,
            Primitive::Quad(q) if q.background == rgb(0xff0000)
        )),
        "it isn't painted in the window"
    );
}

#[test]
fn the_surface_has_room_for_shadows_in_whole_logical_pixels() {
    let mut ui = mount(216, || {
        div().child(
            popup().child(div().w(100.0).h(50.0).bg(rgb(0xffffff)).shadow(BoxShadow {
                offset_y: 8.0,
                blur: 10.0,
                color: rgba(0x00000080),
                ..Default::default()
            })),
        )
    });
    let p = shows(&ui.take_requests()).remove(0);
    let s = Scale(216);
    // 15 logical pixels of blur + 1 physical pixel, minus the offset above.
    let extent = (s.pxf(10.0) * 1.5 + 1.0).ceil() as i32;
    assert_eq!(p.geometry.x, s.logical_ceil(extent));
    assert_eq!(p.geometry.y, s.logical_ceil(extent - s.px(8.0)));
    assert_eq!(p.geometry.width, 100);
    assert_eq!(p.geometry.height, 50);
    assert_eq!(p.logical.width, 100 + 2 * s.logical_ceil(extent));
    assert_eq!(
        p.logical.height,
        p.geometry.y + 50 + s.logical_ceil(extent + s.px(8.0))
    );
    assert_eq!(p.buffer.width, s.px(p.logical.width as f32));

    configure(&mut ui, &p, (0, 0));
    let frame = ui.paint_frame(Instant::now());
    let scene = &frame.popups[0].1;
    let shadow = scene
        .primitives
        .iter()
        .find_map(|p| match p {
            Primitive::Shadow(s) => Some(s),
            _ => None,
        })
        .unwrap();
    let bounds = shadow.bounds();
    assert!(bounds.x >= 0 && bounds.y >= 0, "{bounds:?}");
    assert!(bounds.right() <= scene.width as i32);
    assert!(bounds.bottom() <= scene.height as i32);
}

#[test]
fn input_goes_by_window_coordinates_and_events_bubble_to_where_it_was_declared() {
    let inner = signal(0);
    let outer = signal(0);
    let rect = signal(RectI::default());
    let mut ui = mount(120, move || {
        div()
            .w(100.0)
            .h(50.0)
            .on_click(move |_| outer.update(|c| *c += 1))
            .child(
                popup().child(
                    div()
                        .on_pointer_down(move |e| rect.set(e.rect))
                        .child(item(inner, "x")),
                ),
            )
    });
    opened(&mut ui, (300, 300));
    // On the popup: the window below doesn't see it.
    click(&mut ui, 310.0, 310.0);
    assert_eq!(inner.get(), 1);
    assert_eq!(outer.get(), 1, "bubbled up from the popup");
    assert_eq!(rect.get(), RectI::new(300, 300, 100, 20));
    // A popup moves without anything else changing.
    let p = PopupId(with_doc(|d| anti_webui_core::tree::popup_id(d.popups[0]).0));
    ui.handle_event(WindowEvent::PopupConfigured {
        id: p,
        origin: PointI::new(10, 200),
        size: SizeI::new(100, 20),
    });
    ui.update();
    click(&mut ui, 15.0, 205.0);
    assert_eq!(inner.get(), 2);
    assert_eq!(rect.get(), RectI::new(10, 200, 100, 20));
}

#[test]
fn pressing_outside_closes_a_menu_and_does_nothing_else() {
    let open = signal(true);
    let dismissed = signal(0);
    let background = signal(0);
    let hit = signal(0);
    let mut ui = mount(120, move || {
        div()
            .size_full()
            .on_click(move |_| background.update(|c| *c += 1))
            .child(show(
                move || open.get(),
                move || {
                    popup()
                        .on_dismiss(move || {
                            dismissed.update(|c| *c += 1);
                            open.set(false);
                        })
                        .child(item(hit, "x"))
                },
            ))
    });
    let p = opened(&mut ui, (100, 100));
    click(&mut ui, 110.0, 110.0);
    assert_eq!(hit.get(), 1);
    assert_eq!(dismissed.get(), 0, "pressing inside keeps it open");

    click(&mut ui, 300.0, 300.0);
    assert_eq!(dismissed.get(), 1);
    assert_eq!(
        background.get(),
        1,
        "the click on the popup bubbled; none from outside"
    );
    assert_eq!(closes(&ui.take_requests()), [p.id]);
    assert!(ui.paint_frame(Instant::now()).popups.is_empty());
    // The next press is an ordinary one.
    click(&mut ui, 300.0, 300.0);
    assert_eq!(background.get(), 2);
}

#[test]
fn escape_closes_the_topmost_menu() {
    let closed = signal(Vec::<u32>::new());
    let mut ui = mount(120, move || {
        div().child(
            popup()
                .on_dismiss(move || closed.update(|c| c.push(1)))
                .child(
                    div().w(50.0).h(50.0).child(
                        popup()
                            .on_dismiss(move || closed.update(|c| c.push(2)))
                            .child(div().w(50.0).h(50.0)),
                    ),
                ),
        )
    });
    opened(&mut ui, (0, 0));
    opened(&mut ui, (50, 0));
    key(&mut ui, NamedKey::Escape);
    assert_eq!(closed.get(), [2]);
    key(&mut ui, NamedKey::Escape);
    assert_eq!(closed.get(), [2, 1]);
}

#[test]
fn the_platform_can_close_it_too_and_only_building_it_again_brings_it_back() {
    let round = signal(0);
    let dismissed = signal(0);
    let mut ui = mount(120, move || {
        div().child(dyn_view(move || {
            round.get();
            popup()
                .on_dismiss(move || dismissed.update(|c| *c += 1))
                .child(div().w(50.0).h(50.0))
        }))
    });
    let p = opened(&mut ui, (0, 0));
    ui.handle_event(WindowEvent::PopupDismissed(p.id));
    ui.update();
    assert_eq!(dismissed.get(), 1);
    let reqs = ui.take_requests();
    assert_eq!(closes(&reqs), [p.id]);
    assert!(shows(&reqs).is_empty());
    assert!(ui.paint_frame(Instant::now()).popups.is_empty());

    round.set(1);
    ui.update();
    let reqs = ui.take_requests();
    assert_eq!(shows(&reqs).len(), 1);
    assert_ne!(shows(&reqs)[0].id, p.id);
}

#[test]
fn only_one_chain_of_menus_is_open() {
    let a = signal(true);
    let b = signal(false);
    let dismissed = signal(0);
    let mut ui = mount(120, move || {
        div()
            .child(show(
                move || a.get(),
                move || {
                    popup()
                        .on_dismiss(move || dismissed.update(|c| *c += 1))
                        .child(div().w(50.0).h(50.0))
                },
            ))
            .child(show(
                move || b.get(),
                || popup().child(div().w(50.0).h(50.0)),
            ))
    });
    let first = opened(&mut ui, (0, 0));
    b.set(true);
    ui.update();
    let reqs = ui.take_requests();
    let close_at = reqs
        .iter()
        .position(|r| *r == PlatformRequest::ClosePopup(first.id))
        .expect("the first one closes");
    let show_at = reqs
        .iter()
        .position(|r| matches!(r, PlatformRequest::ShowPopup(_)))
        .expect("the second one opens");
    assert!(close_at < show_at, "grabs go on the topmost popup");
    assert_eq!(dismissed.get(), 1);
}

#[test]
fn what_is_attached_waits_for_its_parent_and_closes_with_it() {
    let mut ui = mount(120, || {
        div().child(
            popup().child(
                div()
                    .w(50.0)
                    .h(50.0)
                    .child(popup().child(div().w(30.0).h(30.0))),
            ),
        )
    });
    let reqs = shows(&ui.take_requests());
    assert_eq!(reqs.len(), 1, "the inner one waits");
    let parent = &reqs[0];
    configure(&mut ui, parent, (10, 10));
    let inner = shows(&ui.take_requests()).remove(0);
    assert_eq!(inner.parent, Some(parent.id));
    // Anchored in window coordinates, to what contains it on the parent's surface.
    assert_eq!(inner.anchor, RectI::new(10, 10, 50, 50));
    configure(&mut ui, &inner, (60, 10));
    assert_eq!(ui.paint_frame(Instant::now()).popups.len(), 2);

    ui.handle_event(WindowEvent::PopupDismissed(parent.id));
    ui.update();
    let reqs = closes(&ui.take_requests());
    assert!(reqs.contains(&parent.id) && reqs.contains(&inner.id));
    assert!(ui.paint_frame(Instant::now()).popups.is_empty());
}

#[test]
fn removing_it_closes_it() {
    let open = signal(true);
    let mut ui = mount(120, move || {
        div().child(show(
            move || open.get(),
            || popup().child(div().w(50.0).h(50.0)),
        ))
    });
    let p = opened(&mut ui, (0, 0));
    open.set(false);
    ui.update();
    assert_eq!(closes(&ui.take_requests()), [p.id]);
    assert!(with_doc(|d| d.popups.is_empty()));
}

#[test]
fn hiding_what_it_belongs_to_closes_it_until_that_comes_back() {
    let hidden = signal(false);
    let mut ui = mount(120, move || {
        div()
            .dyn_style(move |s| if hidden.get() { s.hidden() } else { s })
            .child(popup().child(div().w(50.0).h(50.0)))
    });
    let p = opened(&mut ui, (0, 0));
    hidden.set(true);
    ui.update();
    assert_eq!(closes(&ui.take_requests()), [p.id]);
    hidden.set(false);
    ui.update();
    assert_eq!(shows(&ui.take_requests()).len(), 1);
}

#[test]
fn content_and_anchor_changes_reach_the_platform_without_touching_the_window() {
    let tall = signal(50.0);
    let scrolled = node_ref();
    let mut ui = mount(120, move || {
        div().h(100.0).overflow_y_scroll().node_ref(scrolled).child(
            div().h(1000.0).child(
                div()
                    .h(30.0)
                    .child(popup().child(div().w(50.0).dyn_style(move |s| s.h(tall.get())))),
            ),
        )
    });
    let p = opened(&mut ui, (0, 30));
    assert_eq!(p.anchor, RectI::new(0, 0, 600, 30));
    let dirty = |ui: &mut Ui| {
        tall.update(|t| *t += 10.0);
        with_doc(|d| {
            d.mark_style_of(d.popups[0]);
        });
        ui.update();
    };
    dirty(&mut ui);
    let reqs = ui.take_requests();
    let PlatformRequest::UpdatePopup(u) = &reqs[0] else {
        panic!("{reqs:?}");
    };
    assert_eq!(u.wanted, SizeI::new(50, 60));
    // Its anchor scrolls along.
    ui.handle_event(WindowEvent::Scroll {
        position: PointF::new(10.0, 10.0),
        delta_x: 0.0,
        delta_y: 10.0,
        modifiers: M,
        source: ScrollSource::Unknown,
        phase: ScrollPhase::Single,
        time: 0,
    });
    ui.update();
    let reqs = ui.take_requests();
    // A menu is open: scrolling outside of it does nothing.
    assert!(reqs.is_empty(), "{reqs:?}");
}

#[test]
fn the_platform_can_grant_less_and_the_content_scrolls() {
    let last = signal(0);
    let mut ui = mount(120, move || {
        div().child(popup().child(
            div().max_h(300.0).overflow_y_scroll().children(
                (0..20).map(move |i| div().w(100.0).h(30.0).on_click(move |_| last.set(i))),
            ),
        ))
    });
    let p = shows(&ui.take_requests()).remove(0);
    assert_eq!(p.wanted, SizeI::new(100, 300));
    ui.handle_event(WindowEvent::PopupConfigured {
        id: p.id,
        origin: PointI::new(0, 100),
        size: SizeI::new(100, 120),
    });
    ui.update();
    let reqs = ui.take_requests();
    let PlatformRequest::UpdatePopup(u) = &reqs[0] else {
        panic!("{reqs:?}");
    };
    assert_eq!(u.wanted, p.wanted, "it still would like what it asked for");
    assert_eq!(u.geometry.height, 120);
    let frame = ui.paint_frame(Instant::now());
    assert_eq!(frame.popups[0].1.height, 120);

    // Its list scrolls as a layer of its own, in window coordinates from the outside.
    ui.handle_event(WindowEvent::Scroll {
        position: PointF::new(50.0, 150.0),
        delta_x: 0.0,
        delta_y: 90.0,
        modifiers: M,
        source: ScrollSource::Unknown,
        phase: ScrollPhase::Single,
        time: 0,
    });
    ui.update();
    click(&mut ui, 50.0, 105.0);
    assert_eq!(last.get(), 3);
    let frame = ui.paint_frame(Instant::now());
    assert!(
        frame.popups[0]
            .1
            .primitives
            .iter()
            .any(|p| matches!(p, Primitive::Layer(_)))
    );
}

#[test]
fn scrolling_stops_at_the_popup() {
    let view = node_ref();
    let mut ui = mount(120, move || {
        div().h(100.0).overflow_y_scroll().node_ref(view).child(
            div()
                .h(1000.0)
                .child(popup().kind(PopupKind::Bubble).child(div().w(50.0).h(50.0))),
        )
    });
    let reqs = ui.take_requests();
    let p = shows(&reqs).remove(0);
    configure(&mut ui, &p, (300, 300));
    let scroll = |ui: &mut Ui, x: f32, y: f32| {
        ui.handle_event(WindowEvent::Scroll {
            position: PointF::new(x, y),
            delta_x: 0.0,
            delta_y: 40.0,
            modifiers: M,
            source: ScrollSource::Unknown,
            phase: ScrollPhase::Single,
            time: 0,
        });
        ui.update();
    };
    scroll(&mut ui, 310.0, 310.0);
    let top = || with_doc(|d| d.get(view.get().unwrap()).unwrap().scroll.y);
    assert_eq!(top(), 0, "the popup has nothing to scroll");
    // A bubble doesn't take the input elsewhere.
    scroll(&mut ui, 10.0, 10.0);
    assert_eq!(top(), 40);
}

#[test]
fn tooltips_let_the_input_through_and_stay_open() {
    let below = signal(0);
    let dismissed = signal(0);
    let mut ui = mount(120, move || {
        div()
            .size_full()
            .on_click(move |_| below.update(|c| *c += 1))
            .child(
                popup()
                    .kind(PopupKind::Tooltip)
                    .on_dismiss(move || dismissed.update(|c| *c += 1))
                    .child(div().w(50.0).h(50.0)),
            )
    });
    opened(&mut ui, (10, 10));
    click(&mut ui, 20.0, 20.0);
    assert_eq!(below.get(), 1);
    assert_eq!(dismissed.get(), 0);
    assert_eq!(ui.paint_frame(Instant::now()).popups.len(), 1);
}

#[test]
fn bubbles_are_placed_in_the_window() {
    let place = |placement, left: f32, top: f32| {
        let mut ui = mount(120, move || {
            div().size_full().relative().child(
                div().absolute().left(left).top(top).w(100.0).h(40.0).child(
                    popup()
                        .kind(PopupKind::Bubble)
                        .placement(placement)
                        .gap(5)
                        .child(div().w(200.0).h(100.0)),
                ),
            )
        });
        let p = shows(&ui.take_requests()).remove(0);
        drop(ui);
        p.position.unwrap()
    };
    assert_eq!(
        place(Placement::BelowStart, 10.0, 10.0),
        PointI::new(10, 55)
    );
    assert_eq!(
        place(Placement::BelowEnd, 200.0, 10.0),
        PointI::new(100, 55)
    );
    assert_eq!(
        place(Placement::AboveStart, 10.0, 200.0),
        PointI::new(10, 95)
    );
    assert_eq!(
        place(Placement::RightStart, 10.0, 10.0),
        PointI::new(115, 10)
    );
    assert_eq!(
        place(Placement::LeftStart, 300.0, 10.0),
        PointI::new(95, 10)
    );
    // Flips where there is no room, and stays in the window.
    assert_eq!(
        place(Placement::BelowStart, 10.0, 340.0),
        PointI::new(10, 235)
    );
    assert_eq!(
        place(Placement::AboveStart, 10.0, 10.0),
        PointI::new(10, 55)
    );
    assert_eq!(
        place(Placement::RightStart, 490.0, 10.0),
        PointI::new(285, 10)
    );
    assert_eq!(
        place(Placement::BelowStart, 500.0, 10.0),
        PointI::new(400, 55)
    );
}

#[test]
fn focus_stays_with_what_opened_it() {
    let field_in_popup = node_ref();
    let button = node_ref();
    let focused = signal(false);
    let mut ui = mount(120, move || {
        div()
            .child(
                div()
                    .w(100.0)
                    .h(30.0)
                    .focusable()
                    .node_ref(button)
                    .on_focus_change(move |f| focused.set(f)),
            )
            .child(
                popup().child(
                    div()
                        .flex()
                        .flex_col()
                        .child(div().w(100.0).h(30.0))
                        .child(text_input().w(100.0).h(30.0).node_ref(field_in_popup)),
                ),
            )
    });
    opened(&mut ui, (200, 200));
    button.focus();
    ui.update();
    assert!(focused.get());
    click(&mut ui, 210.0, 210.0);
    assert!(focused.get(), "pressing on nothing focusable in the popup");
    click(&mut ui, 210.0, 240.0);
    assert!(!focused.get(), "a field in the popup can take it");
}

#[test]
fn scroll_into_view_goes_through_nested_scrollers_on_a_popup() {
    let row = node_ref();
    let mut ui = mount(120, move || {
        div().child(
            popup().child(
                div()
                    .h(100.0)
                    .w(100.0)
                    .overflow_y_scroll()
                    .children((0..20).map(move |i| {
                        let d = div().h(30.0);
                        if i == 10 { d.node_ref(row) } else { d }
                    })),
            ),
        )
    });
    opened(&mut ui, (100, 100));
    row.scroll_into_view();
    ui.update();
    // Row 10 is at 300..330 in the content: it ends at the bottom of the 100 high view.
    assert_eq!(row.rect().unwrap(), RectI::new(100, 170, 100, 30));
    row.scroll_into_view();
    ui.update();
    assert_eq!(
        row.rect().unwrap(),
        RectI::new(100, 170, 100, 30),
        "it stays"
    );
}

#[test]
fn a_popup_stays_up_until_what_is_in_it_has_gone() {
    let open = signal(true);
    let inside = node_ref();
    let mut ui = mount(120, move || {
        div().w(100.0).h(50.0).child(
            show(
                move || open.get(),
                move || {
                    popup()
                        .on_dismiss(move || open.set(false))
                        .child(div().w(100.0).h(40.0).bg(rgb(0xff0000)).node_ref(inside))
                },
            )
            .exit(|s| s.opacity(0.0))
            .transition(Transition::ms(100).linear()),
        )
    });
    let p = opened(&mut ui, (0, 50));
    let opacity = || with_doc(|d| d.get(inside.get().unwrap()).unwrap().computed.opacity);

    let start = Instant::now();
    open.set(false);
    ui.update();
    let at = |ms: u64| start + std::time::Duration::from_millis(ms);
    ui.animate(at(50));
    ui.update();
    assert!(
        (opacity() - 0.5).abs() < 0.06,
        "goes for what is in it: {}",
        opacity()
    );
    assert!(closes(&ui.take_requests()).is_empty(), "its surface stays");
    assert_eq!(ui.paint_frame(at(50)).popups.len(), 1);

    let mut ms = 50;
    while ui.animating() {
        ms += 8;
        ui.animate(at(ms));
        ui.update();
        assert!(ms < 2000);
    }
    assert_eq!(closes(&ui.take_requests()), [p.id]);
}

#[test]
fn a_popup_closed_from_outside_has_no_surface_to_go_on() {
    let open = signal(true);
    let inside = node_ref();
    let mut ui = mount(120, move || {
        div().w(100.0).h(50.0).child(
            show(
                move || open.get(),
                move || {
                    popup()
                        .on_dismiss(move || open.set(false))
                        .child(div().w(100.0).h(40.0).node_ref(inside))
                },
            )
            .exit(|s| s.opacity(0.0))
            .transition(Transition::ms(10_000).linear()),
        )
    });
    let p = opened(&mut ui, (0, 50));
    ui.take_requests();
    // A press outside of it, which is what the platform would dismiss it for.
    press(&mut ui, 500.0, 300.0, true);
    press(&mut ui, 500.0, 300.0, false);
    assert!(!open.get_untracked());
    let start = Instant::now() + std::time::Duration::from_millis(1);
    ui.animate(start);
    ui.update();
    ui.animate(start + std::time::Duration::from_millis(8));
    ui.update();
    assert!(
        inside
            .get()
            .is_none_or(|n| with_doc(|d| d.get(n).is_none())),
        "doesn't wait for what can't be seen"
    );
    assert!(closes(&ui.take_requests()).contains(&p.id));
}
