//! Looks of one's own on the behavior of the standard controls: `x_with`, `styled_x` and
//! `Binding`. See `docs/design/33-customization.md`.

mod common;

use anti_webui_core::event::NamedKey;
use anti_webui_core::prelude::*;
use anti_webui_widgets::*;
use common::*;
use std::cell::Cell;
use std::rc::Rc;

fn column_of(child: impl IntoView) -> Div {
    div().flex().flex_col().items_start().p(20.0).child(child)
}

#[test]
fn a_checkbox_can_stand_for_a_value_that_comes_from_elsewhere() {
    let (a, b) = (signal(false), signal(true));
    let all = Binding::new(
        move || a.get() && b.get(),
        move |v| {
            a.set(v);
            b.set(v);
        },
    );
    let mut ui = start(move || column_of(checkbox(all, "All")));
    click_text(&mut ui, "All");
    assert!(a.get() && b.get(), "turns them all on");
    click_text(&mut ui, "All");
    assert!(!a.get() && !b.get(), "and off");
}

#[test]
fn a_switch_of_ones_own_gets_the_state_and_the_clicks() {
    let on = signal(false);
    let mut ui = start(move || {
        column_of(switch_with(on, |s| {
            div().w(80.0).h(30.0).child(text(move || {
                format!(
                    "{} {}",
                    if s.on() { "on" } else { "off" },
                    if s.pressed() { "pressed" } else { "up" }
                )
            }))
        }))
    });
    assert!(has_text("off up"));
    let (x, y) = middle_of("off up");
    pointer_at(&mut ui, x, y);
    pointer(&mut ui, x, y, true);
    assert!(has_text("off pressed"));
    pointer(&mut ui, x, y, false);
    assert!(on.get());
    assert!(has_text("on up"));
    key(&mut ui, NamedKey::Tab);
    key(&mut ui, NamedKey::Space);
    assert!(!on.get(), "the keyboard flips it too");
}

#[test]
fn a_bigger_thumb_stays_inside_the_track() {
    let value = signal(100.0f32);
    let mut ui = start(move || {
        column_of(
            styled_slider(
                value,
                0.0,
                100.0,
                SliderParts::new().thumb(|t, _| t.w(30.0).h(30.0).bg(rgb(0xff0000))),
            )
            .w(200.0),
        )
    });
    let scene = settle(&mut ui);
    let thumb = colored(&scene, rgb(0xff0000))[0].rect;
    assert_eq!(thumb.width, 30);
    assert_eq!(thumb.x + thumb.width, 20 + 200, "at the far end");
    value.set(0.0);
    let scene = settle(&mut ui);
    assert_eq!(colored(&scene, rgb(0xff0000))[0].rect.x, 20);
}

#[test]
fn the_thumbs_of_a_range_are_stops_and_do_not_pass_each_other() {
    let (start_v, end_v) = (signal(20.0f32), signal(60.0f32));
    let mut ui = start(move || column_of(range_slider(start_v, end_v, 0.0, 100.0).w(200.0)));
    key(&mut ui, NamedKey::Tab);
    key(&mut ui, NamedKey::ArrowRight);
    assert_eq!(
        (start_v.get(), end_v.get()),
        (25.0, 60.0),
        "the first thumb"
    );
    key(&mut ui, NamedKey::End);
    assert_eq!(start_v.get(), 60.0, "stops at the other");
    key(&mut ui, NamedKey::Tab);
    key(&mut ui, NamedKey::PageUp);
    assert!(end_v.get() > 60.0, "the second thumb");
    key(&mut ui, NamedKey::Home);
    assert_eq!(end_v.get(), 60.0, "stops at the other");

    // The pointer takes the nearest thumb.
    start_v.set(20.0);
    end_v.set(60.0);
    ui.update();
    pointer_at(&mut ui, 20.0 + 180.0, 30.0);
    pointer(&mut ui, 20.0 + 180.0, 30.0, true);
    pointer(&mut ui, 20.0 + 180.0, 30.0, false);
    assert_eq!((start_v.get(), end_v.get()), (20.0, 90.0));
    pointer_at(&mut ui, 20.0 + 10.0, 30.0);
    pointer(&mut ui, 20.0 + 10.0, 30.0, true);
    pointer(&mut ui, 20.0 + 10.0, 30.0, false);
    assert_eq!((start_v.get(), end_v.get()), (5.0, 90.0));
}

#[test]
fn a_vertical_slider_with_an_inset_goes_up_from_the_bottom() {
    let value = signal(0.0f32);
    let mut ui = start(move || {
        column_of(slider_with(
            value,
            0.0,
            100.0,
            SliderOptions::new().vertical().inset(10.0),
            |s| {
                div()
                    .w(20.0)
                    .h(120.0)
                    .child(s.thumb(0, div().w(20.0).h(4.0)))
            },
        ))
    });
    // The box is from y = 20 to 140; the track from 30 to 130.
    pointer_at(&mut ui, 30.0, 130.0);
    pointer(&mut ui, 30.0, 130.0, true);
    assert_eq!(value.get(), 0.0);
    pointer_at(&mut ui, 30.0, 80.0);
    assert_eq!(value.get(), 50.0);
    pointer_at(&mut ui, 30.0, 25.0);
    assert_eq!(value.get(), 100.0);
    pointer(&mut ui, 30.0, 25.0, false);
    key(&mut ui, NamedKey::ArrowDown);
    assert_eq!(value.get(), 95.0, "the thumb took the focus");
}

#[test]
fn tabs_of_ones_own_go_with_the_keys_and_the_clicks() {
    let selected = signal(0usize);
    let mut ui = start(move || {
        column_of(tabs_with(selected, 3, |s| {
            div().flex().flex_row().children((0..3).map(move |i| {
                s.tab(
                    i,
                    div().p(8.0).child(text(move || {
                        let mark = if s.is_selected(i) && s.focus_visible() {
                            "*"
                        } else {
                            ""
                        };
                        format!("Tab{i}{mark}")
                    })),
                )
            }))
        }))
    });
    key(&mut ui, NamedKey::Tab);
    assert!(has_text("Tab0*"), "the chosen tab shows the focus");
    key(&mut ui, NamedKey::ArrowLeft);
    assert_eq!(selected.get(), 2, "around the end");
    assert!(has_text("Tab2*"));
    key(&mut ui, NamedKey::Home);
    assert_eq!(selected.get(), 0);
    click_text(&mut ui, "Tab1");
    assert_eq!(selected.get(), 1);
}

#[test]
fn a_tooltip_can_wait_before_it_goes() {
    let mut ui = start(|| {
        column_of(tooltip_with(
            button("Target"),
            TooltipOptions::new().hide_after(std::time::Duration::from_millis(200)),
            |_| || div().child("Says this"),
        ))
    });
    let (x, y) = middle_of("Target");
    pointer_at(&mut ui, x, y);
    wait(&mut ui, 600);
    assert!(has_text("Says this"));
    pointer_at(&mut ui, 900.0, 700.0);
    assert!(has_text("Says this"), "it waits");
    wait(&mut ui, 300);
    assert!(!has_text("Says this"));
}

#[test]
fn something_modal_of_ones_own_is_dismissed_by_escape_and_its_scrim() {
    let open = signal(true);
    let closed = Rc::new(Cell::new(0));
    let count = closed.clone();
    let mut ui = start(move || {
        column_of(div()).child(modal_with(
            move || open.get(),
            move || count.set(count.get() + 1),
            |m| {
                move || {
                    div()
                        .size_full()
                        .child(m.scrim(div().absolute().inset(0.0)))
                        .child(m.surface(div().absolute().left(100.0).top(100.0).child("Inside")))
                }
            },
        ))
    });
    click_text(&mut ui, "Inside");
    assert_eq!(closed.get(), 0);
    click(&mut ui, 5.0, 5.0);
    assert_eq!(closed.get(), 1);
    key(&mut ui, NamedKey::Escape);
    assert_eq!(closed.get(), 2);
}

#[test]
fn a_look_can_hold_something_modal_while_it_goes() {
    let open = signal(true);
    let mut ui = start(move || {
        column_of(div()).child(modal_with(
            move || open.get(),
            || {},
            |m| {
                let shown = follow(
                    move || if m.open() { 1.0f32 } else { 0.0 },
                    Transition::ms(200),
                );
                m.hold_while(move || shown.get() > 0.0);
                move || m.surface(div().child("Going"))
            },
        ))
    });
    run(&mut ui, 300);
    open.set(false);
    ui.update();
    assert!(has_text("Going"), "held");
    run(&mut ui, 400);
    assert!(!has_text("Going"));
}

#[test]
fn listeners_of_the_look_and_of_the_behavior_both_run() {
    let resized = Rc::new(Cell::new(0));
    let count = resized.clone();
    let on = signal(false);
    let clicks = Rc::new(Cell::new(0));
    let clicked = clicks.clone();
    let mut ui = start(move || {
        column_of(switch_with(on, move |_| {
            div()
                .w(40.0)
                .h(20.0)
                .child("S")
                .on_click(move |_| clicked.set(clicked.get() + 1))
                .on_resized(move |_, _| count.set(count.get() + 1))
                .on_resized(|_, _| {})
        }))
    });
    settle(&mut ui);
    click_text(&mut ui, "S");
    assert!(on.get());
    assert_eq!(clicks.get(), 1);
    assert!(
        resized.get() > 0,
        "a second on_resized doesn't take the place of the first"
    );
}
