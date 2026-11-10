//! Input handling tests driven through `Ui` with synthetic events.

use anti_webui_core::Ui;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, RectI, Scale, SizeI};
use anti_webui_core::platform::PlatformRequest;
use anti_webui_core::prelude::*;
use anti_webui_core::text::selection;
use anti_webui_core::tree::with_doc;

fn mount<V: IntoView>(scale: Scale, f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(600, 400),
        scale,
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    ui
}

fn press(ui: &mut Ui, p: PointF, time: u32) {
    ui.handle_event(WindowEvent::PointerButton {
        position: p,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::default(),
        time,
    });
}

fn release(ui: &mut Ui, p: PointF, time: u32) {
    ui.handle_event(WindowEvent::PointerButton {
        position: p,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::default(),
        time,
    });
}

fn move_to(ui: &mut Ui, p: PointF) {
    ui.handle_event(WindowEvent::PointerMoved {
        position: p,
        modifiers: Modifiers::default(),
    });
}

fn key(ui: &mut Ui, key: Key, text: Option<&str>, modifiers: Modifiers) {
    ui.handle_event(WindowEvent::Key(KeyEvent {
        key: key.clone(),
        text: text.map(Into::into),
        modifiers,
        pressed: true,
        repeat: false,
    }));
    ui.handle_event(WindowEvent::Key(KeyEvent {
        key,
        text: None,
        modifiers,
        pressed: false,
        repeat: false,
    }));
    ui.update();
}

fn typ(ui: &mut Ui, s: &str) {
    for c in s.chars() {
        key(
            ui,
            Key::Character(c.to_string()),
            Some(&c.to_string()),
            Modifiers::default(),
        );
    }
}

fn named(ui: &mut Ui, k: NamedKey, modifiers: Modifiers) {
    key(ui, Key::Named(k), None, modifiers);
}

const CTRL: Modifiers = Modifiers::PRIMARY;
const SHIFT: Modifiers = Modifiers {
    shift: true,
    ctrl: false,
    alt: false,
    logo: false,
};

fn at(r: RectI, fx: f32, fy: f32) -> PointF {
    PointF::new(
        r.x as f32 + r.width as f32 * fx,
        r.y as f32 + r.height as f32 * fy,
    )
}

fn selected() -> String {
    with_doc(|d| {
        d.state
            .selection
            .map(|s| selection::selected_text(d, &s))
            .unwrap_or_default()
    })
}

#[test]
fn drag_selects_across_elements() {
    let a = node_ref();
    let b = node_ref();
    let mut ui = mount(Scale(150), move || {
        div()
            .flex()
            .flex_col()
            .gap(20.0)
            .p(10.0)
            .child(div().node_ref(a).child(text("First paragraph here")))
            .child(div().node_ref(b).child(text("Second paragraph there")))
    });
    let (ra, rb) = (a.rect().unwrap(), b.rect().unwrap());
    press(&mut ui, at(ra, 0.0, 0.5), 0);
    // Drag through the gap and past the end of the second paragraph.
    move_to(
        &mut ui,
        PointF::new(ra.x as f32 + 5.0, (ra.bottom() + rb.y) as f32 / 2.0),
    );
    move_to(&mut ui, PointF::new(590.0, rb.y as f32 + 2.0));
    release(&mut ui, PointF::new(590.0, rb.y as f32 + 2.0), 10);
    assert_eq!(selected(), "First paragraph here\nSecond paragraph there");
    let reqs = ui.take_requests();
    assert!(reqs.iter().any(|r| matches!(r, PlatformRequest::SetClipboard { kind: ClipboardKind::Primary, text } if text.starts_with("First"))));

    // Ctrl+C copies to the clipboard.
    key(&mut ui, Key::Character("c".into()), None, CTRL);
    let reqs = ui.take_requests();
    assert!(reqs.iter().any(|r| matches!(
        r,
        PlatformRequest::SetClipboard {
            kind: ClipboardKind::Clipboard,
            ..
        }
    )));

    // Selecting backwards from the middle of the second paragraph into the first works too.
    let in_second = PointF::new(rb.x as f32 + 40.0, rb.y as f32 + rb.height as f32 / 2.0);
    let in_first = PointF::new(ra.x as f32 + 40.0, ra.y as f32 + ra.height as f32 / 2.0);
    // Pressing the selection could drag it, so it's cleared first.
    press(&mut ui, in_second, 1000);
    release(&mut ui, in_second, 1005);
    press(&mut ui, in_second, 2000);
    move_to(&mut ui, in_first);
    release(&mut ui, in_first, 2010);
    let s = selected();
    let (first, second) = s.split_once('\n').expect("selection spans both paragraphs");
    assert!(
        "First paragraph here".ends_with(first) && !first.is_empty() && first.len() < 20,
        "{s:?}"
    );
    assert!(
        "Second paragraph there".starts_with(second) && !second.is_empty() && second.len() < 22,
        "{s:?}"
    );
}

#[test]
fn double_and_triple_click() {
    let a = node_ref();
    let mut ui = mount(Scale(120), move || {
        div()
            .p(10.0)
            .child(div().node_ref(a).child(text("alpha beta gamma")))
    });
    let r = a.rect().unwrap();
    // Inside "beta": the text starts at the left edge of the block.
    let p = PointF::new(r.x as f32 + 52.0, r.y as f32 + r.height as f32 / 2.0);
    press(&mut ui, p, 0);
    release(&mut ui, p, 5);
    press(&mut ui, p, 100);
    release(&mut ui, p, 105);
    assert_eq!(selected().trim(), "beta");
    press(&mut ui, p, 200);
    release(&mut ui, p, 205);
    assert_eq!(selected(), "alpha beta gamma");
}

#[test]
fn japanese_double_click_selects_a_word() {
    let a = node_ref();
    let mut ui = mount(Scale(120), move || {
        div()
            .p(10.0)
            .child(div().node_ref(a).child(text("日本語の文章を選択する")))
    });
    let r = a.rect().unwrap();
    let p = at(r, 0.1, 0.5);
    press(&mut ui, p, 0);
    release(&mut ui, p, 5);
    press(&mut ui, p, 50);
    release(&mut ui, p, 55);
    let s = selected();
    assert!(
        !s.is_empty() && s.chars().count() < 11,
        "dictionary-based word expected, got {s:?}"
    );
}

#[test]
fn text_input_editing() {
    let field = node_ref();
    let value = signal(String::new());
    let submitted = signal(String::new());
    let mut ui = mount(Scale(180), move || {
        div().p(10.0).child(
            text_input()
                .w(300.0)
                .node_ref(field)
                .value(value)
                .on_submit(move |s| submitted.set(s.to_owned())),
        )
    });
    let r = field.rect().unwrap();
    press(&mut ui, at(r, 0.5, 0.5), 0);
    release(&mut ui, at(r, 0.5, 0.5), 5);
    ui.update();
    typ(&mut ui, "hello world");
    assert_eq!(value.get(), "hello world");

    named(&mut ui, NamedKey::Backspace, Modifiers::WORD);
    assert_eq!(value.get(), "hello ");
    named(&mut ui, NamedKey::Home, Modifiers::default());
    typ(&mut ui, ">");
    assert_eq!(value.get(), ">hello ");
    named(&mut ui, NamedKey::End, SHIFT);
    typ(&mut ui, "X");
    assert_eq!(value.get(), ">X");
    key(&mut ui, Key::Character("z".into()), None, CTRL);
    assert_eq!(value.get(), ">hello ");
    key(&mut ui, Key::Character("a".into()), None, CTRL);
    named(&mut ui, NamedKey::Delete, Modifiers::default());
    assert_eq!(value.get(), "");

    // External changes flow back into the field.
    value.set("from signal".into());
    ui.update();
    named(&mut ui, NamedKey::End, Modifiers::default());
    typ(&mut ui, "!");
    assert_eq!(value.get(), "from signal!");
    named(&mut ui, NamedKey::Enter, Modifiers::default());
    assert_eq!(submitted.get(), "from signal!");
}

#[test]
fn ime_preedit_and_commit() {
    let field = node_ref();
    let value = signal(String::from("ab"));
    let mut ui = mount(Scale(150), move || {
        div()
            .p(10.0)
            .child(text_area().w(300.0).node_ref(field).value(value))
    });
    let r = field.rect().unwrap();
    press(&mut ui, at(r, 0.9, 0.2), 0);
    release(&mut ui, at(r, 0.9, 0.2), 5);
    ui.update();
    let reqs = ui.take_requests();
    let ime = reqs.iter().rev().find_map(|r| match r {
        PlatformRequest::SetIme(s) => Some(s.clone()),
        _ => None,
    });
    let ime = ime.expect("IME enabled on focus").expect("state");
    assert_eq!(ime.surrounding, "ab");
    assert_eq!(ime.cursor, 2);
    assert!(ime.multiline);

    ui.handle_event(WindowEvent::Ime(ImeUpdate {
        preedit: Some(("にほん".into(), Some((0, 9)))),
        ..Default::default()
    }));
    ui.update();
    // Pre-edit text is shown but not committed to the value.
    assert_eq!(value.get(), "ab");
    let display = with_doc(|d| {
        d.get(field.get().unwrap())
            .unwrap()
            .edit()
            .unwrap()
            .display_text()
    });
    assert_eq!(display, "abにほん");

    ui.handle_event(WindowEvent::Ime(ImeUpdate {
        commit: Some("日本".into()),
        ..Default::default()
    }));
    ui.update();
    assert_eq!(value.get(), "ab日本");
    let reqs = ui.take_requests();
    let ime = reqs
        .iter()
        .rev()
        .find_map(|r| match r {
            PlatformRequest::SetIme(Some(s)) => Some(s.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(ime.cursor, "ab日本".len());

    // Clicking elsewhere blurs the field and disables the IME.
    press(&mut ui, PointF::new(590.0, 390.0), 1000);
    release(&mut ui, PointF::new(590.0, 390.0), 1005);
    ui.update();
    let reqs = ui.take_requests();
    assert!(
        reqs.iter()
            .any(|r| matches!(r, PlatformRequest::SetIme(None)))
    );
}

#[test]
fn a_password_shows_as_dots_edits_by_its_characters_and_isnt_copied() {
    let field = node_ref();
    let value = signal(String::new());
    let mut ui = mount(Scale(120), move || {
        div().p(10.0).child(
            text_input()
                .w(300.0)
                .node_ref(field)
                .value(value)
                .purpose(ImePurpose::Password),
        )
    });
    let r = field.rect().unwrap();
    press(&mut ui, at(r, 0.5, 0.5), 0);
    release(&mut ui, at(r, 0.5, 0.5), 5);
    ui.update();
    typ(&mut ui, "pässwörd");
    assert_eq!(value.get(), "pässwörd");
    let shown = |id: anti_webui_core::tree::NodeId| {
        with_doc(|d| {
            let e = d.get(id).unwrap().edit().unwrap();
            (e.layout.layout.text_len(), e.caret_rect(2).x)
        })
    };
    let id = field.get().unwrap();
    // Laid out as one dot a character, whatever their length.
    assert_eq!(shown(id).0, 8 * '•'.len_utf8());

    // The caret goes by characters, the long ones too.
    named(&mut ui, NamedKey::Home, Modifiers::default());
    let start = shown(id).1;
    named(&mut ui, NamedKey::ArrowRight, Modifiers::default());
    named(&mut ui, NamedKey::ArrowRight, Modifiers::default());
    assert!(shown(id).1 > start);
    typ(&mut ui, "X");
    assert_eq!(value.get(), "päXsswörd");
    named(&mut ui, NamedKey::Backspace, Modifiers::default());
    named(&mut ui, NamedKey::Backspace, Modifiers::default());
    assert_eq!(value.get(), "psswörd");

    // A click at the end puts the caret after the last dot.
    press(&mut ui, at(r, 0.95, 0.5), 400);
    release(&mut ui, at(r, 0.95, 0.5), 405);
    ui.update();
    typ(&mut ui, "!");
    assert_eq!(value.get(), "psswörd!");

    ui.take_requests();
    key(&mut ui, Key::Character("a".into()), None, CTRL);
    key(&mut ui, Key::Character("c".into()), None, CTRL);
    key(&mut ui, Key::Character("x".into()), None, CTRL);
    assert!(
        !ui.take_requests()
            .iter()
            .any(|r| matches!(r, PlatformRequest::SetClipboard { .. }))
    );
    assert_eq!(value.get(), "psswörd!");
}

#[test]
fn paste_and_tab_focus() {
    let f1 = node_ref();
    let f2 = node_ref();
    let v1 = signal(String::new());
    let v2 = signal(String::new());
    let mut ui = mount(Scale(120), move || {
        div()
            .flex()
            .flex_col()
            .gap(8.0)
            .child(text_input().node_ref(f1).value(v1))
            .child(text_input().node_ref(f2).value(v2))
    });
    named(&mut ui, NamedKey::Tab, Modifiers::default());
    typ(&mut ui, "one");
    named(&mut ui, NamedKey::Tab, Modifiers::default());
    key(&mut ui, Key::Character("v".into()), None, CTRL);
    let reqs = ui.take_requests();
    assert!(
        reqs.iter()
            .any(|r| matches!(r, PlatformRequest::Paste(ClipboardKind::Clipboard)))
    );
    ui.handle_event(WindowEvent::Paste {
        kind: ClipboardKind::Clipboard,
        text: "pasted\ntext".into(),
    });
    ui.update();
    assert_eq!(v1.get(), "one");
    // Single-line inputs turn newlines into spaces.
    assert_eq!(v2.get(), "pasted text");
}

#[test]
fn buttons_click_and_keyboard_activation() {
    let count = signal(0);
    let b = node_ref();
    let mut ui = mount(Scale(120), move || {
        div().p(10.0).child(
            div()
                .node_ref(b)
                .w(80.0)
                .h(30.0)
                .focusable()
                .select_none()
                .on_click(move |_| count.update(|c| *c += 1)),
        )
    });
    let r = b.rect().unwrap();
    press(&mut ui, at(r, 0.5, 0.5), 0);
    release(&mut ui, at(r, 0.5, 0.5), 5);
    assert_eq!(count.get(), 1);
    // Released outside: no click.
    press(&mut ui, at(r, 0.5, 0.5), 1000);
    release(&mut ui, PointF::new(500.0, 300.0), 1005);
    assert_eq!(count.get(), 1);
    named(&mut ui, NamedKey::Enter, Modifiers::default());
    assert_eq!(count.get(), 2);
}

#[test]
fn keyed_list_reuses_nodes() {
    let items = signal(vec![1, 2, 3]);
    let mut ui = mount(Scale(120), move || {
        div().child(for_each(
            move || items.get(),
            |i| *i,
            |i| div().child(text(format!("item {i}"))),
        ))
    });
    let node_for = |label: &str| {
        with_doc(|d| {
            d.nodes.iter().find_map(|(id, n)| match &n.kind {
                anti_webui_core::tree::NodeKind::Text(t) if t.text == label => Some(id),
                _ => None,
            })
        })
    };
    let two = node_for("item 2").unwrap();
    items.set(vec![3, 2, 4]);
    ui.update();
    assert_eq!(node_for("item 2"), Some(two));
    assert!(node_for("item 1").is_none());
    assert!(node_for("item 4").is_some());
}

#[test]
fn release_outside_still_reaches_the_pressed_node() {
    let dragging = signal(false);
    let moves = signal(0);
    let s = node_ref();
    let mut ui = mount(Scale(120), move || {
        div().p(10.0).child(
            div()
                .node_ref(s)
                .w(100.0)
                .h(20.0)
                .select_none()
                .on_pointer_down(move |_| dragging.set(true))
                .on_pointer_move(move |_| {
                    if dragging.get_untracked() {
                        moves.update(|m| *m += 1);
                    }
                })
                .on_pointer_up(move |_| dragging.set(false)),
        )
    });
    let r = s.rect().unwrap();
    press(&mut ui, at(r, 0.5, 0.5), 0);
    assert!(dragging.get());
    // Drag far away and release there.
    move_to(&mut ui, PointF::new(500.0, 350.0));
    release(&mut ui, PointF::new(500.0, 350.0), 10);
    assert!(!dragging.get(), "pointer_up must reach the pressed node");
    // Later moves without a press no longer drag.
    let before = moves.get();
    move_to(&mut ui, PointF::new(400.0, 300.0));
    assert_eq!(moves.get(), before);

    // Losing window focus mid-drag also ends it.
    press(&mut ui, at(r, 0.5, 0.5), 1000);
    assert!(dragging.get());
    ui.handle_event(WindowEvent::Focused(false));
    assert!(!dragging.get());
}

#[test]
fn only_the_primary_button_clicks_the_others_aux_click() {
    let clicks = signal(0);
    let aux = signal(Vec::new());
    let b = node_ref();
    let mut ui = mount(Scale(120), move || {
        div().p(10.0).child(
            div()
                .node_ref(b)
                .w(80.0)
                .h(30.0)
                .on_click(move |_| clicks.update(|c| *c += 1))
                .on_aux_click(move |e| aux.update(|a| a.push(e.button))),
        )
    });
    let p = at(b.rect().unwrap(), 0.5, 0.5);
    for (i, button) in [
        PointerButton::Secondary,
        PointerButton::Middle,
        PointerButton::Primary,
    ]
    .into_iter()
    .enumerate()
    {
        for pressed in [true, false] {
            ui.handle_event(WindowEvent::PointerButton {
                position: p,
                button,
                pressed,
                modifiers: Modifiers::default(),
                time: i as u32 * 1000 + if pressed { 0 } else { 5 },
            });
        }
    }
    assert_eq!(clicks.get(), 1);
    assert_eq!(
        aux.get(),
        [Some(PointerButton::Secondary), Some(PointerButton::Middle)]
    );
}
