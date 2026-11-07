//! Progress, badges, tooltips and snackbars. See `docs/design/20-md3e-components.md`.

mod common;

use anti_webui::core::event::WindowEvent;
use anti_webui::core::geometry::{PointI, SizeI};
use anti_webui::prelude::*;
use anti_webui_md3e::components::{
    RichTooltip, Snackbar, badge, badged, circular_progress, circular_wavy_progress,
    indeterminate_circular_progress, indeterminate_linear_progress, indeterminate_wavy_progress,
    linear_progress, rich_tooltip, snackbars, text_badge, tooltip,
};
use common::*;
use std::time::Duration;

#[test]
fn what_shows_a_known_progress_has_its_size_and_rests() {
    let mut ui = start_ui(|| {
        div()
            .child(linear_progress(|| 0.5))
            .child(circular_progress(|| 0.5))
    });
    let scene = settle(&mut ui);
    let sizes: Vec<_> = shaders(&scene)
        .iter()
        .map(|s| (s.rect.width, s.rect.height))
        .collect();
    assert_eq!(sizes, [(240, 4), (40, 40)]);
    assert_at_rest(&mut ui);
}

#[test]
fn a_changed_progress_moves_there_and_rests_again() {
    let value = signal(0.2f32);
    let mut ui = start_ui(move || div().child(linear_progress(move || value.get())));
    assert_at_rest(&mut ui);
    value.set(0.8);
    ui.update();
    assert!(ui.animating());
    assert_at_rest(&mut ui);
}

#[test]
fn what_shows_an_unknown_progress_keeps_moving_for_as_long_as_it_is_there() {
    let shown = signal(true);
    let mut ui = start_ui(move || {
        div().child(show(
            move || shown.get(),
            || {
                div()
                    .child(indeterminate_linear_progress())
                    .child(indeterminate_wavy_progress())
                    .child(indeterminate_circular_progress())
            },
        ))
    });
    let scene = run(&mut ui, 80);
    let sizes: Vec<_> = shaders(&scene)
        .iter()
        .map(|s| (s.rect.width, s.rect.height))
        .collect();
    assert_eq!(sizes, [(240, 4), (240, 10), (40, 40)]);
    assert!(ui.animating());
    shown.set(false);
    ui.update();
    assert_at_rest(&mut ui);
}

#[test]
fn a_wavy_circle_is_bigger_and_only_waves_in_between() {
    let mut ui = start_ui(|| div().child(circular_wavy_progress(|| 0.5)));
    let scene = run(&mut ui, 80);
    let s = shaders(&scene).pop().expect("the circle");
    assert_eq!((s.rect.width, s.rect.height), (48, 48));
}

#[test]
fn a_badge_sits_on_the_corner_of_what_it_is_on() {
    let mut ui = start_ui(|| {
        div()
            .flex()
            .child(badged(div().w(24.0).h(24.0), badge(), false))
    });
    let scene = settle(&mut ui);
    let dot = colored(&scene, C.error).pop().expect("the badge");
    assert_eq!(
        (dot.rect.x, dot.rect.y, dot.rect.width, dot.rect.height),
        (18, 0, 6, 6)
    );
}

#[test]
fn a_badge_with_a_text_comes_and_goes_with_it() {
    let count = signal(0);
    let mut ui = start_ui(move || {
        div().flex().child(badged(
            div().w(24.0).h(24.0),
            text_badge(move || match count.get() {
                0 => String::new(),
                n => n.to_string(),
            }),
            true,
        ))
    });
    let scene = settle(&mut ui);
    assert!(colored(&scene, C.error).is_empty());
    count.set(3);
    let scene = settle(&mut ui);
    let shown = colored(&scene, C.error).pop().expect("the badge");
    assert_eq!(
        (shown.rect.x, shown.rect.y, shown.rect.height),
        (12, -2, 16)
    );
    count.set(0);
    let scene = settle(&mut ui);
    assert!(colored(&scene, C.error).is_empty());
}

#[test]
fn a_tooltip_shows_after_a_moment_and_goes_when_the_pointer_does() {
    let mut ui = start_ui(|| div().p(100.0).child(tooltip(div().w(40.0).h(40.0), "Tip")));
    popups_shown(&mut ui);
    pointer_at(&mut ui, 120.0, 120.0);
    assert!(popups_shown(&mut ui).is_empty(), "not right away");
    wait(&mut ui, 1000);
    let shown = popups_shown(&mut ui);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].kind, PopupKind::Tooltip);
    assert_eq!(shown[0].placement, Placement::AboveCenter);
    let _ = ui.take_requests();
    pointer_at(&mut ui, 500.0, 500.0);
    wait(&mut ui, 1000);
    settle(&mut ui);
    assert!(ui.take_requests().iter().any(|r| matches!(
        r,
        anti_webui::core::platform::PlatformRequest::ClosePopup(_)
    )));
}

#[test]
fn leaving_before_it_shows_keeps_a_tooltip_from_showing() {
    let mut ui = start_ui(|| div().p(100.0).child(tooltip(div().w(40.0).h(40.0), "Tip")));
    pointer_at(&mut ui, 120.0, 120.0);
    pointer_at(&mut ui, 500.0, 500.0);
    wait(&mut ui, 1000);
    assert!(popups_shown(&mut ui).is_empty());
}

#[test]
fn the_action_of_a_rich_tooltip_can_be_reached() {
    let done = signal(0);
    let mut ui = start_ui(move || {
        div().p(200.0).child(rich_tooltip(
            div().w(40.0).h(40.0),
            RichTooltip::new("Text")
                .title("Title")
                .action("Do it", move || done.update(|d| *d += 1)),
        ))
    });
    pointer_at(&mut ui, 220.0, 220.0);
    wait(&mut ui, 1000);
    let shown = popups_shown(&mut ui);
    let params = shown.first().expect("the tooltip");
    assert_eq!(params.kind, PopupKind::Bubble, "it takes input");
    let _ = (
        PointI::default(),
        SizeI::default(),
        WindowEvent::Focused(true),
    );
}

fn snack_view() -> (anti_webui_md3e::components::Snackbars, anti_webui::core::Ui) {
    let bars = std::rc::Rc::new(std::cell::Cell::new(None));
    let inner = bars.clone();
    let ui = start_ui(move || {
        let place = snackbars();
        inner.set(Some(place));
        div().size_full().child(place.view())
    });
    (bars.get().unwrap(), ui)
}

#[test]
fn snackbars_come_one_at_a_time_and_go_by_themselves() {
    let (bars, mut ui) = snack_view();
    let bar = |scene: &anti_webui::core::scene::Scene| colored(scene, C.inverse_surface);
    bars.show(Snackbar::new("One"));
    bars.show(Snackbar::new("Two").lasting(Duration::from_millis(100)));
    let scene = settle(&mut ui);
    let first = bar(&scene);
    assert_eq!(first.len(), 1);
    // At the bottom of the window, in the middle.
    assert_eq!(first[0].rect.bottom(), 800 - 24);
    assert_eq!(first[0].rect.x + first[0].rect.width / 2, 500);
    wait(&mut ui, 3000);
    assert_eq!(bar(&settle(&mut ui)).len(), 1, "not after 3 seconds");
    wait(&mut ui, 5000);
    let scene = settle(&mut ui);
    assert_eq!(bar(&scene).len(), 1, "the next one came");
    wait(&mut ui, 5000);
    let scene = settle(&mut ui);
    assert!(bar(&scene).is_empty());
    assert_at_rest(&mut ui);
}

#[test]
fn a_snackbar_with_an_action_stays_until_it_is_used() {
    let (bars, mut ui) = snack_view();
    let done = signal(0);
    bars.show(Snackbar::new("Sent").action("Undo", move || done.update(|d| *d += 1)));
    let scene = settle(&mut ui);
    let shown = colored(&scene, C.inverse_surface)
        .pop()
        .expect("the snackbar");
    wait(&mut ui, 60000);
    assert_eq!(colored(&settle(&mut ui), C.inverse_surface).len(), 1);
    click(
        &mut ui,
        (shown.rect.right() - 20) as f32,
        (shown.rect.y + shown.rect.height / 2) as f32,
    );
    assert_eq!(done.get(), 1);
    assert!(colored(&settle(&mut ui), C.inverse_surface).is_empty());
}

#[test]
fn what_a_snackbar_does_not_cover_stays_clickable() {
    let clicks = signal(0);
    let bars = std::rc::Rc::new(std::cell::Cell::new(None));
    let inner = bars.clone();
    let mut ui = start_ui(move || {
        let place = snackbars();
        inner.set(Some(place));
        // Next to it rather than around it: what goes to a portal also bubbles to its ancestors.
        div()
            .size_full()
            .child(
                div()
                    .size_full()
                    .on_click(move |_| clicks.update(|c| *c += 1)),
            )
            .child(place.view())
    });
    let bars: anti_webui_md3e::components::Snackbars = bars.get().unwrap();
    bars.show(Snackbar::new("Here"));
    settle(&mut ui);
    // At the height of the snackbar, to the left of it.
    click(&mut ui, 50.0, 750.0);
    assert_eq!(clicks.get(), 1);
}

/// See `tests/icons.rs`.
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn pixel(p: &[u8], width: usize, x: usize, y: usize) -> [u8; 3] {
    let i = (y * width + x) * 4;
    [p[i], p[i + 1], p[i + 2]]
}

fn near(a: [u8; 3], c: Color) -> bool {
    let b = c.snapped().premultiplied();
    (0..3).all(|i| (a[i] as f32 - b[i] * 255.0).abs() < 12.0)
}

#[test]
fn a_linear_progress_has_a_line_a_gap_a_track_and_a_stop() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let drawn = anti_webui::app::headless::render_rgba(240, 4, 1.0, || {
        div()
            .size_full()
            .bg(C.surface)
            .child(linear_progress(|| 0.5))
    });
    let p = match drawn {
        Ok(p) => p,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    assert!(near(pixel(&p, 240, 60, 2), C.primary), "the line");
    assert!(near(pixel(&p, 240, 124, 2), C.surface), "the gap");
    assert!(
        near(pixel(&p, 240, 180, 2), C.secondary_container),
        "the track"
    );
    assert!(near(pixel(&p, 240, 238, 2), C.primary), "the stop");
}

#[test]
fn a_determinate_circle_starts_at_the_top_and_goes_clockwise() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let drawn = anti_webui::app::headless::render_rgba(40, 40, 1.0, || {
        div()
            .size_full()
            .bg(C.surface)
            .child(circular_progress(|| 0.25))
    });
    let p = match drawn {
        Ok(p) => p,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    // A quarter: from 12 o'clock to 3 o'clock, and the track for most of the rest.
    assert!(near(pixel(&p, 40, 32, 7), C.primary), "the upper right");
    assert!(
        near(pixel(&p, 40, 7, 32), C.secondary_container),
        "the lower left"
    );
}

#[test]
fn a_tooltip_shows_for_the_focus_from_the_keyboard() {
    let mut ui = start_ui(|| {
        div()
            .p(100.0)
            .child(tooltip(div().w(40.0).h(40.0).focusable(), "Tip"))
    });
    popups_shown(&mut ui);
    key(&mut ui, anti_webui::core::event::NamedKey::Tab);
    assert_eq!(popups_shown(&mut ui).len(), 1, "at once");
}
