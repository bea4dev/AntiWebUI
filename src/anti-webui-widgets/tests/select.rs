//! The pull-down list, driven like a window would drive it.

use anti_webui_core::Ui;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, PointI, Scale, SizeI};
use anti_webui_core::platform::{PlatformRequest, PopupParams};
use anti_webui_core::prelude::*;
use anti_webui_widgets::select;

const M: Modifiers = Modifiers {
    shift: false,
    ctrl: false,
    alt: false,
    logo: false,
};

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

fn key(ui: &mut Ui, k: NamedKey) {
    ui.handle_event(WindowEvent::Key(KeyEvent {
        key: Key::Named(k),
        text: None,
        modifiers: M,
        pressed: true,
        repeat: false,
    }));
    ui.update();
}

fn mount(value: Signal<u32>) -> Ui {
    let options = (0..30).map(|i| (i, format!("option {i}"))).collect();
    let mut ui = Ui::new(move || div().p(10.0).w(200.0).child(select(value, options)));
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(400, 300),
        scale: Scale::ONE,
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    ui
}

fn opened(ui: &mut Ui) -> Option<PopupParams> {
    ui.update();
    ui.take_requests().into_iter().find_map(|r| match r {
        PlatformRequest::ShowPopup(p) => Some(p),
        _ => None,
    })
}

fn closed(ui: &mut Ui) -> bool {
    ui.update();
    ui.take_requests()
        .iter()
        .any(|r| matches!(r, PlatformRequest::ClosePopup(_)))
}

/// Plays the platform: below the anchor, at the size it asked for.
fn place(ui: &mut Ui, p: &PopupParams) {
    ui.handle_event(WindowEvent::PopupConfigured {
        id: p.id,
        origin: PointI::new(
            p.anchor.x - p.geometry.x,
            p.anchor.bottom() + p.gap - p.geometry.y,
        ),
        size: p.wanted,
    });
    ui.update();
}

#[test]
fn pressing_opens_it_and_picking_an_option_closes_it() {
    let value = signal(0u32);
    let mut ui = mount(value);
    ui.take_requests();
    press(&mut ui, 50.0, 20.0, true);
    let p = opened(&mut ui).expect("it opens on the press");
    assert_eq!(p.geometry.width, 180, "as wide as what it hangs from");
    assert!(p.geometry.height <= 280, "and scrolls past its height");
    assert!(p.logical.width > p.geometry.width, "room for the shadow");
    press(&mut ui, 50.0, 20.0, false);
    assert!(
        !closed(&mut ui),
        "letting go of the press isn't picking anything"
    );
    place(&mut ui, &p);

    // Option 2, in the window's coordinates.
    let y = p.anchor.bottom() as f32 + 4.0 + 5.0 + 2.5 * 30.0;
    press(&mut ui, 50.0, y, true);
    press(&mut ui, 50.0, y, false);
    assert_eq!(value.get(), 2);
    assert!(closed(&mut ui));
}

#[test]
fn pressing_it_again_closes_it_instead_of_opening_it_again() {
    let value = signal(0u32);
    let mut ui = mount(value);
    press(&mut ui, 50.0, 20.0, true);
    press(&mut ui, 50.0, 20.0, false);
    let p = opened(&mut ui).unwrap();
    place(&mut ui, &p);
    press(&mut ui, 50.0, 20.0, true);
    assert!(closed(&mut ui));
    press(&mut ui, 50.0, 20.0, false);
    assert!(opened(&mut ui).is_none());
    assert_eq!(value.get(), 0);
}

#[test]
fn the_keyboard_works_from_where_the_focus_stays() {
    let value = signal(3u32);
    let mut ui = mount(value);
    key(&mut ui, NamedKey::Tab);
    ui.take_requests();
    key(&mut ui, NamedKey::Enter);
    let p = opened(&mut ui).expect("enter opens it");
    place(&mut ui, &p);
    key(&mut ui, NamedKey::ArrowDown);
    key(&mut ui, NamedKey::ArrowDown);
    assert_eq!(value.get(), 3, "nothing is picked yet");
    key(&mut ui, NamedKey::Enter);
    assert_eq!(value.get(), 5);
    assert!(closed(&mut ui));

    key(&mut ui, NamedKey::ArrowDown);
    let p = opened(&mut ui).expect("arrow down opens it");
    place(&mut ui, &p);
    key(&mut ui, NamedKey::End);
    key(&mut ui, NamedKey::Escape);
    assert!(closed(&mut ui));
    assert_eq!(value.get(), 5, "escape leaves it as it was");
}
