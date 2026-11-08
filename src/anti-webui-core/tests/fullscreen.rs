//! The window and what is in it on all of the screen. See `docs/design/27-fullscreen.md`.

use anti_webui_core::Ui;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, RectI, Scale, SizeI};
use anti_webui_core::platform::{CursorIcon, PlatformRequest};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{Primitive, Scene};
use anti_webui_core::tree::with_doc;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

const WINDOW: SizeI = SizeI {
    width: 600,
    height: 400,
};
const SCREEN: SizeI = SizeI {
    width: 1000,
    height: 800,
};
const RED: Color = Color::from_rgb(0xff0000);
const GREEN: Color = Color::from_rgb(0x00ff00);

fn resized(ui: &mut Ui, size: SizeI) {
    ui.handle_event(WindowEvent::Resized {
        size,
        scale: Scale(120),
    });
}

fn mount<V: IntoView>(f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    resized(&mut ui, WINDOW);
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    ui
}

/// Plays the platform: the window got this size because of that state.
fn window_went(ui: &mut Ui, on: bool) {
    ui.handle_event(WindowEvent::Fullscreen(on));
    resized(ui, if on { SCREEN } else { WINDOW });
    ui.update();
}

fn asked(ui: &mut Ui) -> Vec<bool> {
    ui.take_requests()
        .into_iter()
        .filter_map(|r| match r {
            PlatformRequest::SetFullscreen(on) => Some(on),
            _ => None,
        })
        .collect()
}

fn key(ui: &mut Ui, named: NamedKey) {
    ui.handle_event(WindowEvent::Key(KeyEvent {
        key: Key::Named(named),
        text: None,
        modifiers: Modifiers::default(),
        pressed: true,
        repeat: false,
    }));
    ui.update();
}

fn click(ui: &mut Ui, x: f32, y: f32) {
    for pressed in [true, false] {
        ui.handle_event(WindowEvent::PointerButton {
            position: PointF::new(x, y),
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
            time: 0,
        });
    }
    ui.update();
}

fn has_focus(r: NodeRef) -> bool {
    with_doc(|d| d.state.focused) == r.get()
}

/// The colors of all that was painted, with where.
fn quads(scene: &Scene) -> Vec<(Color, RectI)> {
    let mut out = Vec::new();
    for p in &scene.primitives {
        match p {
            Primitive::Quad(q) => out.push((q.background, q.rect)),
            Primitive::Layer(l) => out.extend(quads(&l.content)),
            _ => {}
        }
    }
    out
}

/// A box that is small, moved, clipped and faded by what it is in, next to another box.
fn page(target: NodeRef) -> Div {
    div()
        .w_full()
        .h_full()
        .flex()
        .flex_col()
        .child(div().h(50.0).bg(GREEN))
        .child(
            div()
                .w(120.0)
                .h(60.0)
                .overflow_hidden()
                .opacity(0.5)
                .translate(20.0, 0.0)
                .child(
                    div()
                        .node_ref(target)
                        .focusable()
                        .w(30.0)
                        .h(30.0)
                        .m(5.0)
                        .bg(RED),
                ),
        )
}

#[test]
fn the_window_says_how_it_is_whoever_changed_it() {
    let seen = Rc::new(Cell::new(None));
    let told = seen.clone();
    let mut ui = mount(move || {
        effect(move || told.set(Some(window_fullscreen())));
        div()
    });
    assert_eq!(seen.get(), Some(false));
    set_window_fullscreen(true);
    assert_eq!(asked(&mut ui), [true]);
    // Asking doesn't make it so.
    assert_eq!(seen.get(), Some(false));
    window_went(&mut ui, true);
    assert_eq!(seen.get(), Some(true));
    // What the rest of the system does counts too.
    window_went(&mut ui, false);
    assert_eq!(seen.get(), Some(false));
}

#[test]
fn the_window_alone_leaves_the_ui_as_it_is() {
    let target = node_ref();
    let mut ui = mount(move || page(target));
    window_went(&mut ui, true);
    assert_eq!(target.rect().unwrap().width, 30);
    let scene = ui.paint(Instant::now());
    assert_eq!(scene.clear, Color::WHITE);
    assert!(quads(&scene).iter().any(|q| q.0 == GREEN));
}

#[test]
fn an_element_takes_all_of_the_screen_when_the_window_does() {
    let target = node_ref();
    let mut ui = mount(move || page(target));
    target.request_fullscreen();
    assert_eq!(asked(&mut ui), [true]);
    // Not before the window is that big, as both change in the same frame.
    ui.update();
    assert!(!target.is_fullscreen());
    assert!(
        quads(&ui.paint(Instant::now()))
            .iter()
            .any(|q| q.0 == GREEN)
    );

    window_went(&mut ui, true);
    assert!(target.is_fullscreen());
    // Its size, margin and what it is in don't count.
    assert_eq!(target.rect(), Some(RectI::new(0, 0, 1000, 800)));
    let scene = ui.paint(Instant::now());
    assert_eq!(scene.clear, Color::BLACK);
    // The rest doesn't show.
    assert_eq!(quads(&scene), [(RED, RectI::new(0, 0, 1000, 800))]);
}

#[test]
fn an_element_doesnt_take_what_clips_or_fades_it() {
    let target = node_ref();
    let mut ui = mount(move || page(target));
    target.request_fullscreen();
    window_went(&mut ui, true);
    let scene = ui.paint(Instant::now());
    assert!(
        scene
            .primitives
            .iter()
            .all(|p| !matches!(p, Primitive::Layer(_))),
        "{:?}",
        scene.primitives
    );
    let quad = quads(&scene)[0];
    assert_eq!(quad.0.a, 1.0);
}

#[test]
fn only_what_is_on_all_of_the_screen_gets_the_pointer() {
    let target = node_ref();
    let clicks = Rc::new(Cell::new((0, 0)));
    let (inside, outside) = (clicks.clone(), clicks.clone());
    let mut ui = mount(move || {
        div()
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(100.0)
                    .on_click(move |_| outside.set((outside.get().0 + 1, outside.get().1))),
            )
            .child(
                div()
                    .h(100.0)
                    .node_ref(target)
                    .on_click(move |_| inside.set((inside.get().0, inside.get().1 + 1))),
            )
    });
    click(&mut ui, 10.0, 10.0);
    assert_eq!(clicks.get(), (1, 0));
    target.request_fullscreen();
    window_went(&mut ui, true);
    click(&mut ui, 10.0, 10.0);
    assert_eq!(clicks.get(), (1, 1));
}

#[test]
fn escape_brings_it_all_back_in_one_go() {
    let target = node_ref();
    let mut ui = mount(move || page(target));
    let before = target.rect();
    target.request_fullscreen();
    window_went(&mut ui, true);
    asked(&mut ui);

    key(&mut ui, NamedKey::Escape);
    assert_eq!(asked(&mut ui), [false]);
    // It stays until the window has changed.
    assert!(target.is_fullscreen());
    assert_eq!(ui.paint(Instant::now()).clear, Color::BLACK);

    window_went(&mut ui, false);
    assert!(!target.is_fullscreen());
    assert_eq!(target.rect(), before);
    let scene = ui.paint(Instant::now());
    assert_eq!(scene.clear, Color::WHITE);
    assert!(quads(&scene).iter().any(|q| q.0 == GREEN));
}

#[test]
fn what_stays_behind_keeps_its_place_and_isnt_laid_out_again() {
    let target = node_ref();
    let list = node_ref();
    let sizes = Rc::new(Cell::new(0));
    let told = sizes.clone();
    let mut ui = mount(move || {
        div().w_full().h_full().flex().flex_col().child(
            div()
                .node_ref(list)
                .h(100.0)
                .overflow_scroll()
                .on_resized(move |_, _| told.set(told.get() + 1))
                // The one that goes takes up room in what scrolls.
                .child(div().h(500.0))
                .child(div().node_ref(target).h(500.0)),
        )
    });
    list.scroll_to_after_layout(0, 800);
    ui.update();
    let sized = sizes.get();
    assert_eq!(list.scroll_offset().y, 800.0);
    target.request_fullscreen();
    window_went(&mut ui, true);
    window_went(&mut ui, false);
    ui.update();
    assert_eq!(list.scroll_offset().y, 800.0);
    assert_eq!(sizes.get(), sized);
}

#[test]
fn what_changed_behind_shows_afterwards() {
    let target = node_ref();
    let label = node_ref();
    let wide = signal(50.0f32);
    let mut ui = mount(move || {
        div()
            .child(
                div()
                    .node_ref(label)
                    .h(10.0)
                    .dyn_style(move |s| s.w(wide.get())),
            )
            .child(div().node_ref(target).w(10.0).h(10.0))
    });
    target.request_fullscreen();
    window_went(&mut ui, true);
    wide.set(70.0);
    ui.update();
    window_went(&mut ui, false);
    assert_eq!(label.rect().unwrap().width, 70);
    assert_eq!(target.rect().unwrap().y, 10);
}

#[test]
fn a_window_already_on_all_of_the_screen_stays_there() {
    let target = node_ref();
    let mut ui = mount(move || page(target));
    window_went(&mut ui, true);
    target.request_fullscreen();
    ui.update();
    assert!(target.is_fullscreen());
    assert_eq!(target.rect(), Some(RectI::new(0, 0, 1000, 800)));
    exit_fullscreen();
    ui.update();
    assert!(!target.is_fullscreen());
    assert_eq!(asked(&mut ui), Vec::<bool>::new());
    assert_eq!(target.rect().unwrap().width, 30);
}

#[test]
fn the_system_taking_the_window_back_ends_it() {
    let target = node_ref();
    let mut ui = mount(move || page(target));
    target.request_fullscreen();
    window_went(&mut ui, true);
    asked(&mut ui);
    window_went(&mut ui, false);
    assert!(!target.is_fullscreen());
    assert_eq!(asked(&mut ui), Vec::<bool>::new());
    assert_eq!(target.rect().unwrap().width, 30);
}

#[test]
fn the_focus_goes_along_and_comes_back() {
    let target = node_ref();
    let other = node_ref();
    let mut ui = mount(move || {
        div()
            .child(div().node_ref(other).focusable().h(10.0))
            .child(div().node_ref(target).focusable().h(10.0))
    });
    other.focus();
    ui.update();
    target.request_fullscreen();
    window_went(&mut ui, true);
    ui.update();
    assert!(has_focus(target));
    // What can't be seen can't have it.
    other.focus();
    ui.update();
    assert!(has_focus(target));
    key(&mut ui, NamedKey::Tab);
    assert!(has_focus(target));
    exit_fullscreen();
    window_went(&mut ui, false);
    ui.update();
    assert!(has_focus(other));
}

#[test]
fn keys_go_to_what_is_on_all_of_the_screen_and_up_from_there() {
    let target = node_ref();
    let got = Rc::new(Cell::new((0, 0)));
    let (inside, around) = (got.clone(), got.clone());
    let mut ui = mount(move || {
        div()
            .on_key_down(move |_| around.set((around.get().0 + 1, around.get().1)))
            .child(
                div()
                    .node_ref(target)
                    .on_key_down(move |_| inside.set((inside.get().0, inside.get().1 + 1))),
            )
    });
    target.request_fullscreen();
    window_went(&mut ui, true);
    key(&mut ui, NamedKey::ArrowLeft);
    assert_eq!(got.get(), (1, 1));
}

#[test]
fn escape_can_be_kept_from_ending_it() {
    let target = node_ref();
    let mut ui = mount(move || {
        div().child(div().node_ref(target).on_key_down(|e| {
            if matches!(e.key, Key::Named(NamedKey::Escape)) {
                e.prevent_default();
            }
        }))
    });
    target.request_fullscreen();
    window_went(&mut ui, true);
    asked(&mut ui);
    key(&mut ui, NamedKey::Escape);
    assert_eq!(asked(&mut ui), Vec::<bool>::new());
    assert!(target.is_fullscreen());
}

#[test]
fn taking_the_element_away_ends_it() {
    let target = node_ref();
    let shown = signal(true);
    let mut ui = mount(move || {
        div().child(show(
            move || shown.get(),
            move || div().node_ref(target).w(10.0).h(10.0),
        ))
    });
    target.request_fullscreen();
    window_went(&mut ui, true);
    asked(&mut ui);
    shown.set(false);
    ui.update();
    assert_eq!(asked(&mut ui), [false]);
    assert_eq!(ui.paint(Instant::now()).clear, Color::WHITE);
}

#[test]
fn a_portal_inside_or_outside_goes_on_top() {
    let target = node_ref();
    let mut ui = mount(move || {
        div()
            .child(div().node_ref(target).bg(RED).w(10.0).h(10.0))
            .child(portal().child(div().absolute().w(40.0).h(40.0).bg(GREEN)))
    });
    target.request_fullscreen();
    window_went(&mut ui, true);
    let scene = ui.paint(Instant::now());
    let colors: Vec<Color> = quads(&scene).iter().map(|q| q.0).collect();
    assert_eq!(colors, [RED, GREEN]);
}

#[test]
fn an_element_in_a_portal_leaves_the_rest_of_the_portal_behind() {
    let target = node_ref();
    let mut ui = mount(move || {
        div().child(
            portal().child(
                div()
                    .w(40.0)
                    .h(40.0)
                    .bg(GREEN)
                    .child(div().node_ref(target).bg(RED).w(10.0).h(10.0)),
            ),
        )
    });
    target.request_fullscreen();
    window_went(&mut ui, true);
    let scene = ui.paint(Instant::now());
    assert_eq!(quads(&scene), [(RED, RectI::new(0, 0, 1000, 800))]);
    click(&mut ui, 500.0, 500.0);
    window_went(&mut ui, false);
    assert_eq!(target.rect().unwrap().width, 10);
}

#[test]
fn a_cursor_changing_by_itself_gets_there() {
    let hidden = signal(false);
    let mut ui = mount(move || {
        div().w_full().h_full().dyn_style(move |s| {
            s.cursor(if hidden.get() {
                CursorIcon::None
            } else {
                CursorIcon::Default
            })
        })
    });
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(10.0, 10.0),
        modifiers: Modifiers::default(),
    });
    ui.update();
    ui.take_requests();
    hidden.set(true);
    ui.update();
    assert!(
        ui.take_requests()
            .contains(&PlatformRequest::SetCursor(CursorIcon::None))
    );
}

#[test]
fn changing_ones_mind_before_the_window_gets_there_brings_the_window_back() {
    let target = node_ref();
    let mut ui = mount(move || page(target));
    target.request_fullscreen();
    exit_fullscreen();
    assert_eq!(asked(&mut ui), [true, false]);
    // On its way there and back it may or may not say so, and nothing shows differently.
    window_went(&mut ui, true);
    assert!(!target.is_fullscreen());
    window_went(&mut ui, false);
    assert!(!target.is_fullscreen());
    assert_eq!(target.rect().unwrap().width, 30);
}
