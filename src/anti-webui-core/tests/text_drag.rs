//! Dragging selected text and dropping text on inputs. See `docs/design/23-drag-and-drop.md`.

use anti_webui_core::Ui;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, Scale, SizeI};
use anti_webui_core::platform::{DragOffer, PlatformRequest};
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{Layer, Primitive, Scene};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

const WORDS: &str = "alpha beta gamma delta";

/// Far enough apart for no two presses to be one double click.
struct Hand {
    ui: Ui,
    time: u32,
}

fn mount<V: IntoView>(f: impl FnOnce() -> V) -> Hand {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(800, 600),
        scale: Scale::ONE,
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    Hand { ui, time: 0 }
}

fn platform<V: IntoView>(f: impl FnOnce() -> V) -> Hand {
    let mut hand = mount(f);
    hand.ui.set_platform_drag(true);
    hand
}

fn at(x: f32, y: f32) -> PointF {
    PointF::new(x, y)
}

impl Hand {
    fn moved(&mut self, x: f32, y: f32) {
        self.ui.handle_event(WindowEvent::PointerMoved {
            position: at(x, y),
            modifiers: Modifiers::default(),
        });
        self.ui.update();
    }

    fn button(&mut self, x: f32, y: f32, pressed: bool, modifiers: Modifiers) {
        self.ui.handle_event(WindowEvent::PointerButton {
            position: at(x, y),
            button: PointerButton::Primary,
            pressed,
            modifiers,
            time: self.time,
        });
        self.ui.update();
    }

    fn press(&mut self, x: f32, y: f32) {
        self.time += 10_000;
        self.moved(x, y);
        self.button(x, y, true, Modifiers::default());
    }

    fn release(&mut self, x: f32, y: f32) {
        self.button(x, y, false, Modifiers::default());
    }

    fn click(&mut self, x: f32, y: f32) {
        self.press(x, y);
        self.release(x, y);
    }

    /// Selects by dragging, and returns what got selected.
    fn select(&mut self, x0: f32, x1: f32, y: f32) -> String {
        self.requests();
        self.press(x0, y);
        self.moved(x1, y);
        self.release(x1, y);
        self.primary()
    }

    fn primary(&mut self) -> String {
        self.requests()
            .into_iter()
            .find_map(|r| match r {
                PlatformRequest::SetClipboard {
                    kind: ClipboardKind::Primary,
                    text,
                } => Some(text),
                _ => None,
            })
            .unwrap_or_default()
    }

    fn requests(&mut self) -> Vec<PlatformRequest> {
        self.ui.take_requests()
    }

    /// What the drag and drop came to since the last time.
    fn drag_requests(&mut self) -> Vec<PlatformRequest> {
        self.requests()
            .into_iter()
            .filter(|r| {
                matches!(
                    r,
                    PlatformRequest::StartDrag(_)
                        | PlatformRequest::AcceptDrag(_)
                        | PlatformRequest::ReceiveDrop(_)
                        | PlatformRequest::FinishDrop
                )
            })
            .collect()
    }

    fn key(&mut self, c: &str, modifiers: Modifiers) {
        self.ui.handle_event(WindowEvent::Key(KeyEvent {
            key: Key::Character(c.into()),
            text: None,
            modifiers,
            pressed: true,
            repeat: false,
        }));
        self.ui.update();
    }

    fn ctrl(&mut self, c: &str) {
        self.key(
            c,
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
        );
    }

    fn escape(&mut self) {
        self.ui.handle_event(WindowEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Escape),
            text: None,
            modifiers: Modifiers::default(),
            pressed: true,
            repeat: false,
        }));
        self.ui.update();
    }

    /// What Ctrl+C takes from what has the focus or the document.
    fn copied(&mut self) -> String {
        self.requests();
        self.ctrl("c");
        self.requests()
            .into_iter()
            .find_map(|r| match r {
                PlatformRequest::SetClipboard {
                    kind: ClipboardKind::Clipboard,
                    text,
                } => Some(text),
                _ => None,
            })
            .unwrap_or_default()
    }

    fn scene(&mut self) -> Scene {
        self.ui.update();
        self.ui.paint(Instant::now())
    }

    fn entered(&mut self, x: f32, y: f32, mimes: &[&str]) {
        self.ui.handle_event(WindowEvent::DragEntered {
            position: at(x, y),
            mimes: mimes.iter().map(|m| m.to_string()).collect(),
            modifiers: Modifiers::default(),
        });
        self.ui.update();
    }

    fn dropped(&mut self, x: f32, y: f32, action: DragAction) {
        self.ui.handle_event(WindowEvent::DragDropped {
            position: at(x, y),
            action,
        });
        self.ui.update();
    }
}

fn drag_layer(scene: &Scene) -> Option<&Layer> {
    scene.primitives.iter().find_map(|p| match p {
        Primitive::Layer(l) if l.key == u64::MAX - 1 => Some(l),
        _ => None,
    })
}

fn glyphs_of(scene: &Scene) -> usize {
    scene
        .primitives
        .iter()
        .map(|p| match p {
            Primitive::Glyphs(g) => g.glyphs.len(),
            _ => 0,
        })
        .sum()
}

/// Text at the top, and a place to drop things below it.
fn words() -> Div {
    div()
        .absolute()
        .left(0.0)
        .top(0.0)
        .w(600.0)
        .child(text(WORDS))
}

fn field_at(top: f32, value: Signal<String>) -> TextInput {
    text_input()
        .absolute()
        .left(0.0)
        .top(top)
        .w(500.0)
        .h(30.0)
        .value(value)
}

fn text_and_field(value: Signal<String>) -> Div {
    div()
        .w(800.0)
        .h(600.0)
        .child(words())
        .child(field_at(100.0, value))
}

/// Where the middle of the first line of what is selected by `select(2.0, 60.0, 8.0)` is.
const IN: (f32, f32) = (20.0, 8.0);

#[test]
fn pressing_selected_text_leaves_it_selected_and_a_click_clears_it() {
    let mut h = mount(words);
    let selected = h.select(2.0, 60.0, 8.0);
    assert!(
        !selected.is_empty() && WORDS.starts_with(&selected),
        "{selected:?}"
    );

    h.press(IN.0, IN.1);
    assert_eq!(h.copied(), selected);
    h.release(IN.0, IN.1);
    assert_eq!(h.copied(), "");
}

#[test]
fn a_click_may_shake_as_long_as_it_stays_a_click() {
    let mut h = mount(words);
    let selected = h.select(2.0, 60.0, 8.0);
    h.press(IN.0, IN.1);
    h.moved(IN.0 + 3.0, IN.1 + 3.0);
    assert_eq!(h.copied(), selected);
    assert!(glyphs_of(&h.scene()) > 0);
    h.release(IN.0 + 3.0, IN.1 + 3.0);
    assert_eq!(h.copied(), "");
}

#[test]
fn it_goes_along_after_four_pixels_on_either_axis() {
    for (dx, dy) in [(4.0, 0.0), (0.0, 4.0), (-4.0, 0.0), (0.0, -4.0)] {
        let mut h = platform(words);
        let selected = h.select(2.0, 60.0, 8.0);
        h.press(IN.0, IN.1);
        h.requests();
        h.moved(IN.0 + dx * 0.75, IN.1 + dy * 0.75);
        assert!(h.drag_requests().is_empty());
        h.moved(IN.0 + dx, IN.1 + dy);
        let requests = h.drag_requests();
        let [PlatformRequest::StartDrag(DragOffer { items, .. })] = &requests[..] else {
            panic!("{requests:?}");
        };
        let plain = items.iter().find(|(m, _)| m == "text/plain").expect("text");
        assert_eq!(std::str::from_utf8(&plain.1).unwrap(), selected);
    }
}

#[test]
fn dragging_it_does_not_extend_the_selection_and_no_click_comes() {
    let clicks = Rc::new(Cell::new(0));
    let c = clicks.clone();
    let mut h = mount(|| words().on_click(move |_| c.set(c.get() + 1)));
    let selected = h.select(2.0, 60.0, 8.0);
    clicks.set(0);
    h.press(IN.0, IN.1);
    h.moved(200.0, 8.0);
    assert_eq!(h.copied(), selected);
    h.release(200.0, 8.0);
    assert_eq!(h.copied(), selected);
    assert_eq!(clicks.get(), 0);
}

#[test]
fn pressing_off_the_selection_starts_a_new_one() {
    let mut h = mount(words);
    h.select(2.0, 60.0, 8.0);
    h.press(120.0, 8.0);
    assert_eq!(h.copied(), "");
}

#[test]
fn pressing_on_it_with_shift_extends_it() {
    let mut h = mount(words);
    let first = h.select(2.0, 60.0, 8.0);
    h.time += 10_000;
    h.moved(120.0, 8.0);
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };
    h.button(120.0, 8.0, true, shift);
    h.button(120.0, 8.0, false, shift);
    assert!(h.copied().len() > first.len());
}

#[test]
fn a_second_click_on_it_selects_the_word() {
    let mut h = mount(words);
    h.select(2.0, 60.0, 8.0);
    h.click(IN.0, IN.1);
    h.button(IN.0, IN.1, true, Modifiers::default());
    h.button(IN.0, IN.1, false, Modifiers::default());
    assert_eq!(h.copied(), "alpha");
}

#[test]
fn right_of_the_highlight_is_not_on_it() {
    let mut h = platform(words);
    let selected = h.select(2.0, 60.0, 8.0);
    assert!(selected.len() < WORDS.len());
    h.press(400.0, 8.0);
    h.moved(420.0, 8.0);
    assert!(h.drag_requests().is_empty());
}

#[test]
fn the_picture_is_the_selected_text_and_nothing_else() {
    let color = rgb(0x123456);
    let mut h = mount(|| {
        div().child(
            words()
                .bg(rgb(0x00ff00))
                .text_color(color)
                .selection_color(rgb(0xff0000)),
        )
    });
    let selected = h.select(2.0, 60.0, 8.0);
    h.press(IN.0, IN.1);
    h.moved(IN.0 + 30.0, IN.1);
    let scene = h.scene();
    let layer = drag_layer(&scene).expect("it shows");
    assert!(
        (layer.opacity - 191.0 / 255.0).abs() < 0.01,
        "{}",
        layer.opacity
    );
    let mut glyphs = 0;
    for p in &layer.content.primitives {
        let Primitive::Glyphs(g) = p else {
            panic!("only text: {p:?}");
        };
        assert_eq!(g.color, color);
        glyphs += g.glyphs.len();
    }
    assert_eq!(glyphs, selected.chars().count());
}

#[test]
fn it_is_where_the_text_was_and_keeps_to_the_pointer() {
    let mut h = mount(words);
    let selected = h.select(2.0, 60.0, 8.0);
    assert!(!selected.is_empty());
    let page = h.scene();
    let first_on_page = page
        .primitives
        .iter()
        .find_map(|p| match p {
            Primitive::Glyphs(g) => g.glyphs.first().copied(),
            _ => None,
        })
        .expect("text");
    h.press(IN.0, IN.1);
    h.moved(IN.0 + 30.0, IN.1 + 7.0);
    let at_first = {
        let scene = h.scene();
        let layer = drag_layer(&scene).expect("it shows");
        let first = layer
            .content
            .primitives
            .iter()
            .find_map(|p| match p {
                Primitive::Glyphs(g) => g.glyphs.first().copied(),
                _ => None,
            })
            .expect("text");
        (
            first.x + layer.origin.x as f32,
            first.y + layer.origin.y as f32,
        )
    };
    assert!(
        (at_first.0 - (first_on_page.x + 30.0)).abs() <= 1.0,
        "{at_first:?}"
    );
    assert!(
        (at_first.1 - (first_on_page.y + 7.0)).abs() <= 1.0,
        "{at_first:?}"
    );
    h.moved(IN.0 + 130.0, IN.1 + 7.0);
    let scene = h.scene();
    let layer = drag_layer(&scene).expect("it shows");
    let first = layer
        .content
        .primitives
        .iter()
        .find_map(|p| match p {
            Primitive::Glyphs(g) => g.glyphs.first().copied(),
            _ => None,
        })
        .expect("text");
    assert!((first.x + layer.origin.x as f32 - (first_on_page.x + 130.0)).abs() <= 1.0);
}

#[test]
fn all_of_it_is_drawn_where_the_page_draws_it() {
    let mut h = mount(words);
    h.ctrl("a");
    let page = h.scene();
    let on_page: Vec<_> = page
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Glyphs(g) => Some(g.glyphs.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    h.press(IN.0, IN.1);
    h.moved(IN.0 + 30.0, IN.1);
    let scene = h.scene();
    let layer = drag_layer(&scene).expect("it shows");
    let in_picture: Vec<_> = layer
        .content
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Glyphs(g) => Some(g.glyphs.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(on_page.len(), in_picture.len());
    for (a, b) in on_page.iter().zip(&in_picture) {
        assert_eq!(a.id, b.id);
        assert!((a.x - b.x).abs() < 0.01, "{} {}", a.x, b.x);
        assert!((a.y - b.y).abs() < 0.01, "{} {}", a.y, b.y);
    }
}

#[test]
fn what_is_scrolled_away_is_not_in_it_and_the_text_still_is() {
    let lines: String = (0..300).map(|i| format!("line {i}\n")).collect();
    let all = lines.clone();
    let mut h = platform(move || {
        div()
            .absolute()
            .left(0.0)
            .top(0.0)
            .w(300.0)
            .h(200.0)
            .overflow_y_scroll()
            .child(text(all))
    });
    h.ctrl("a");
    h.press(10.0, 8.0);
    h.requests();
    h.moved(30.0, 8.0);
    let requests = h.drag_requests();
    let [PlatformRequest::StartDrag(DragOffer { items, .. })] = &requests[..] else {
        panic!("{requests:?}");
    };
    let plain = items.iter().find(|(m, _)| m == "text/plain").expect("text");
    assert_eq!(plain.1.len(), lines.len());
    let frame = h.ui.paint_frame(Instant::now());
    let icon = frame.drag_icon.expect("a surface");
    assert!(
        icon.scene.width <= 300 && icon.scene.height <= 220,
        "{}x{}",
        icon.scene.width,
        icon.scene.height
    );
    let layer = drag_layer(&icon.scene).expect("it shows");
    assert!(
        glyphs_of(&layer.content) < 200,
        "{}",
        glyphs_of(&layer.content)
    );
}

#[test]
fn selected_text_comes_before_what_can_be_dragged() {
    #[derive(Clone)]
    struct Card;
    let mut h = platform(|| words().draggable(|| DragData::new(Card).text("the card")));
    h.ctrl("a");
    h.press(IN.0, IN.1);
    h.requests();
    h.moved(IN.0 + 30.0, IN.1);
    let requests = h.drag_requests();
    let [PlatformRequest::StartDrag(DragOffer { items, .. })] = &requests[..] else {
        panic!("{requests:?}");
    };
    let plain = items.iter().find(|(m, _)| m == "text/plain").expect("text");
    assert_eq!(std::str::from_utf8(&plain.1).unwrap(), WORDS);
}

#[test]
fn what_is_selected_can_only_be_copied_unless_it_can_be_edited() {
    let actions = |h: &mut Hand| {
        h.requests();
        h.press(IN.0, IN.1);
        h.moved(IN.0 + 30.0, IN.1);
        let requests = h.drag_requests();
        let [PlatformRequest::StartDrag(DragOffer { actions, .. })] = &requests[..] else {
            panic!("{requests:?}");
        };
        *actions
    };
    let mut h = platform(words);
    h.ctrl("a");
    assert_eq!(actions(&mut h), DragActions::Copy);

    let value = signal(WORDS.to_string());
    let mut h = platform(|| text_input().w(500.0).h(30.0).value(value));
    h.click(IN.0, 15.0);
    h.ctrl("a");
    h.ui.update();
    let actions_of_input = |h: &mut Hand| {
        h.requests();
        h.press(IN.0, 15.0);
        h.moved(IN.0 + 30.0, 15.0);
        let requests = h.drag_requests();
        let [PlatformRequest::StartDrag(DragOffer { actions, .. })] = &requests[..] else {
            panic!("{requests:?}");
        };
        *actions
    };
    assert_eq!(actions_of_input(&mut h), DragActions::Both);

    let value = signal(WORDS.to_string());
    let mut h = platform(|| text_input().w(500.0).h(30.0).value(value).read_only(true));
    h.click(IN.0, 15.0);
    h.ctrl("a");
    assert_eq!(actions_of_input(&mut h), DragActions::Copy);
}

#[test]
fn passwords_stay_where_they_are() {
    let value = signal(WORDS.to_string());
    let mut h = platform(|| {
        text_input()
            .w(500.0)
            .h(30.0)
            .value(value)
            .purpose(ImePurpose::Password)
    });
    h.click(IN.0, 15.0);
    h.ctrl("a");
    h.press(IN.0, 15.0);
    h.requests();
    h.moved(IN.0 + 30.0, 15.0);
    assert!(h.drag_requests().is_empty());
}

#[test]
fn text_dropped_on_an_input_goes_where_it_was_dropped_and_ends_up_selected() {
    let value = signal("0123456789".to_string());
    let told = Rc::new(RefCell::new(Vec::new()));
    let t = told.clone();
    let mut h = mount(|| {
        div()
            .w(800.0)
            .h(600.0)
            .child(words())
            .child(field_at(100.0, value).on_input(move |s| t.borrow_mut().push(s.to_owned())))
    });
    let selected = h.select(2.0, 60.0, 8.0);
    h.press(IN.0, IN.1);
    // Before it all: it can't be left of the very start.
    h.moved(0.0, 115.0);
    h.release(0.0, 115.0);
    let want = format!("{selected}0123456789");
    assert_eq!(value.get_untracked(), want);
    assert_eq!(*told.borrow(), [want]);
    assert_eq!(h.copied(), selected, "it is selected and has the focus");
}

#[test]
fn the_document_selection_goes_away_when_it_lands() {
    let highlight = rgb(0xff0000);
    let value = signal(String::new());
    let mut h = mount(|| text_and_field(value).selection_color(highlight));
    let selected = h.select(2.0, 60.0, 8.0);
    let highlighted_above = |h: &mut Hand| {
        h.scene()
            .primitives
            .iter()
            .any(|p| matches!(p, Primitive::Quad(q) if q.background == highlight && q.rect.y < 50))
    };
    assert!(highlighted_above(&mut h));
    h.press(IN.0, IN.1);
    h.moved(10.0, 115.0);
    assert!(highlighted_above(&mut h), "it stays until then");
    h.release(10.0, 115.0);
    assert_eq!(value.get_untracked(), selected);
    assert!(!highlighted_above(&mut h));
}

#[test]
fn a_third_click_on_a_selected_word_selects_the_line() {
    let mut h = mount(words);
    h.click(IN.0, IN.1);
    h.button(IN.0, IN.1, true, Modifiers::default());
    h.button(IN.0, IN.1, false, Modifiers::default());
    assert_eq!(h.copied(), "alpha");
    h.button(IN.0, IN.1, true, Modifiers::default());
    h.button(IN.0, IN.1, false, Modifiers::default());
    assert_eq!(h.copied(), WORDS);
}

#[test]
fn its_own_selection_is_no_place_to_land() {
    let value = signal("alpha beta gamma delta epsilon".to_string());
    let mut h = platform(|| div().child(field_at(0.0, value)));
    h.click(5.0, 15.0);
    h.button(5.0, 15.0, true, Modifiers::default());
    h.button(5.0, 15.0, false, Modifiers::default());
    h.press(5.0, 15.0);
    h.moved(30.0, 15.0);
    h.drag_requests();
    h.entered(400.0, 15.0, &["text/plain"]);
    assert!(
        matches!(
            &h.drag_requests()[..],
            [PlatformRequest::AcceptDrag(Some(_))]
        ),
        "after it, it can go"
    );
    h.ui.handle_event(WindowEvent::DragMoved {
        position: at(5.0, 15.0),
        modifiers: Modifiers::default(),
    });
    h.ui.update();
    assert_eq!(h.drag_requests(), [PlatformRequest::AcceptDrag(None)]);
}

#[test]
fn a_drop_can_be_undone_in_one_step() {
    let value = signal("abc".to_string());
    let mut h = mount(|| text_and_field(value));
    h.select(2.0, 60.0, 8.0);
    h.press(IN.0, IN.1);
    h.moved(0.0, 115.0);
    h.release(0.0, 115.0);
    assert_ne!(value.get_untracked(), "abc");
    h.ctrl("z");
    assert_eq!(value.get_untracked(), "abc");
}

#[test]
fn it_moves_inside_of_an_input_in_one_step_of_undo() {
    let value = signal("alpha beta gamma delta epsilon".to_string());
    let mut h = mount(|| div().child(field_at(0.0, value)));
    h.click(5.0, 15.0);
    // "alpha", by the word.
    h.button(5.0, 15.0, true, Modifiers::default());
    h.button(5.0, 15.0, false, Modifiers::default());
    assert_eq!(h.copied(), "alpha");
    h.press(5.0, 15.0);
    h.moved(490.0, 15.0);
    h.release(490.0, 15.0);
    assert_eq!(value.get_untracked(), " beta gamma delta epsilonalpha");
    assert_eq!(h.copied(), "alpha", "it stays selected");
    h.ctrl("z");
    assert_eq!(value.get_untracked(), "alpha beta gamma delta epsilon");
}

#[test]
fn control_copies_inside_of_an_input() {
    let value = signal("alpha beta".to_string());
    let mut h = mount(|| div().child(field_at(0.0, value)));
    h.click(5.0, 15.0);
    h.button(5.0, 15.0, true, Modifiers::default());
    h.button(5.0, 15.0, false, Modifiers::default());
    h.press(5.0, 15.0);
    let ctrl = Modifiers {
        ctrl: true,
        ..Default::default()
    };
    h.ui.handle_event(WindowEvent::PointerMoved {
        position: at(490.0, 15.0),
        modifiers: ctrl,
    });
    h.ui.update();
    h.button(490.0, 15.0, false, ctrl);
    assert_eq!(value.get_untracked(), "alpha betaalpha");
}

#[test]
fn it_cannot_be_dropped_on_itself() {
    let value = signal("alpha beta".to_string());
    let mut h = mount(|| div().child(field_at(0.0, value)));
    h.click(5.0, 15.0);
    h.button(5.0, 15.0, true, Modifiers::default());
    h.button(5.0, 15.0, false, Modifiers::default());
    h.press(5.0, 15.0);
    // Far enough to go along, and still on the word.
    h.moved(14.0, 15.0);
    h.release(14.0, 15.0);
    assert_eq!(value.get_untracked(), "alpha beta");
    assert_eq!(h.copied(), "alpha");
}

#[test]
fn it_moves_from_one_input_to_another_and_each_undoes_its_own() {
    let (from, to) = (signal("alpha beta".to_string()), signal("xyz".to_string()));
    let mut h = mount(|| div().child(field_at(0.0, from)).child(field_at(100.0, to)));
    h.click(5.0, 15.0);
    h.button(5.0, 15.0, true, Modifiers::default());
    h.button(5.0, 15.0, false, Modifiers::default());
    h.press(5.0, 15.0);
    h.moved(0.0, 115.0);
    h.release(0.0, 115.0);
    assert_eq!(from.get_untracked(), " beta");
    assert_eq!(to.get_untracked(), "alphaxyz");
    h.ctrl("z");
    assert_eq!(to.get_untracked(), "xyz");
    assert_eq!(from.get_untracked(), " beta");
}

#[test]
fn what_cannot_be_edited_takes_nothing() {
    let (a, b, c) = (
        signal(String::new()),
        signal(String::new()),
        signal(String::new()),
    );
    let mut h = mount(|| {
        div()
            .w(800.0)
            .h(600.0)
            .child(words())
            .child(field_at(100.0, a).read_only(true))
            .child(field_at(200.0, b).disabled(|| true))
            .child(field_at(300.0, c))
    });
    h.ctrl("a");
    for y in [115.0, 215.0] {
        h.press(IN.0, IN.1);
        h.moved(10.0, y);
        h.release(10.0, y);
    }
    assert_eq!(a.get_untracked(), "");
    assert_eq!(b.get_untracked(), "");
    h.press(IN.0, IN.1);
    h.moved(10.0, 315.0);
    h.release(10.0, 315.0);
    assert_eq!(c.get_untracked(), WORDS);
}

#[test]
fn a_single_line_has_no_line_breaks_but_many_lines_do() {
    let (one, many) = (signal(String::new()), signal(String::new()));
    let mut h = platform(|| {
        div().w(800.0).h(600.0).child(field_at(0.0, one)).child(
            text_area()
                .absolute()
                .left(0.0)
                .top(100.0)
                .w(500.0)
                .h(100.0)
                .value(many),
        )
    });
    let drop_on = |h: &mut Hand, y: f32| {
        h.entered(10.0, y, &["text/plain;charset=utf-8"]);
        h.dropped(10.0, y, DragAction::Copy);
        h.ui.handle_event(WindowEvent::DropData {
            mime: "text/plain;charset=utf-8".into(),
            bytes: b"a\r\nb".to_vec(),
        });
        h.ui.update();
    };
    drop_on(&mut h, 15.0);
    assert_eq!(one.get_untracked(), "a  b");
    drop_on(&mut h, 120.0);
    assert_eq!(many.get_untracked(), "a\nb");
}

#[test]
fn text_from_another_program_is_read_and_put_in_as_a_copy() {
    let value = signal("abc".to_string());
    let mut h = platform(|| div().child(field_at(0.0, value)));
    h.entered(
        0.0,
        15.0,
        &["text/html", "text/plain", "text/plain;charset=utf-8"],
    );
    let mime = "text/plain;charset=utf-8".to_string();
    assert_eq!(
        h.drag_requests(),
        [PlatformRequest::AcceptDrag(Some((
            mime.clone(),
            DragAction::Copy
        )))]
    );
    h.dropped(0.0, 15.0, DragAction::Copy);
    assert_eq!(
        h.drag_requests(),
        [PlatformRequest::ReceiveDrop(mime.clone())]
    );
    assert_eq!(value.get_untracked(), "abc", "not yet");
    h.ui.handle_event(WindowEvent::DropData {
        mime,
        bytes: "日本語".as_bytes().to_vec(),
    });
    h.ui.update();
    assert_eq!(value.get_untracked(), "日本語abc");
    assert_eq!(h.copied(), "日本語");
}

#[test]
fn what_is_not_text_does_not_go_in() {
    let value = signal(String::new());
    let mut h = platform(|| div().child(field_at(0.0, value)));
    h.entered(10.0, 15.0, &["image/png"]);
    assert_eq!(h.drag_requests(), [] as [PlatformRequest; 0]);
}

#[test]
fn a_handler_on_the_input_itself_comes_first() {
    let value = signal(String::new());
    let got = Rc::new(RefCell::new(String::new()));
    let g = got.clone();
    let mut h =
        mount(|| {
            div().w(800.0).h(600.0).child(words()).child(
                field_at(100.0, value).on_drop_text(move |s, _| *g.borrow_mut() = s.to_owned()),
            )
        });
    h.ctrl("a");
    h.press(IN.0, IN.1);
    h.moved(10.0, 115.0);
    h.release(10.0, 115.0);
    assert_eq!(*got.borrow(), WORDS);
    assert_eq!(value.get_untracked(), "");
}

#[test]
fn drop_targets_get_it_as_text_or_by_its_type() {
    let (a, b) = (
        Rc::new(RefCell::new(String::new())),
        Rc::new(RefCell::new(String::new())),
    );
    let (x, y) = (a.clone(), b.clone());
    let mut h = mount(|| {
        div()
            .w(800.0)
            .h(600.0)
            .child(words())
            .child(
                div()
                    .absolute()
                    .top(100.0)
                    .w(100.0)
                    .h(50.0)
                    .on_drop_text(move |s, _| *x.borrow_mut() = s.to_owned()),
            )
            .child(
                div()
                    .absolute()
                    .top(200.0)
                    .w(100.0)
                    .h(50.0)
                    .drop_target(move |t: &DraggedText, _| *y.borrow_mut() = t.0.clone()),
            )
    });
    h.ctrl("a");
    for y in [120.0, 220.0] {
        h.press(IN.0, IN.1);
        h.moved(10.0, y);
        assert_eq!(
            dragged::<DraggedText>(),
            Some(DraggedText(WORDS.to_string()))
        );
        h.release(10.0, y);
    }
    assert_eq!(*a.borrow(), WORDS);
    assert_eq!(*b.borrow(), WORDS);
    assert_eq!(dragged::<DraggedText>(), None);
}

/// The narrow boxes of a color in a frame.
fn narrow_boxes_of(scene: &Scene, color: Color) -> usize {
    scene
        .primitives
        .iter()
        .map(|p| match p {
            Primitive::Quad(q) if q.background == color && q.rect.width <= 4 => 1,
            Primitive::Layer(l) => narrow_boxes_of(&l.content, color),
            _ => 0,
        })
        .sum()
}

#[test]
fn the_caret_takes_the_color_of_the_text_unless_it_has_its_own() {
    let white = rgb(0xffffff);
    let value = signal("text".to_owned());
    let mut h = mount(|| {
        div()
            .w(800.0)
            .h(600.0)
            .bg(rgb(0x2b2d31))
            .text_color(white)
            .child(field_at(0.0, value))
    });
    h.press(100.0, 15.0);
    assert_eq!(narrow_boxes_of(&h.scene(), white), 1);

    let own = rgb(0xff00ff);
    let mut h = mount(|| {
        div()
            .w(800.0)
            .h(600.0)
            .text_color(white)
            .child(field_at(0.0, value).caret_color(own))
    });
    h.press(100.0, 15.0);
    let scene = h.scene();
    assert_eq!(narrow_boxes_of(&scene, own), 1);
    assert_eq!(narrow_boxes_of(&scene, white), 0);
}

/// The caret's quads: only that has this color.
fn carets(scene: &Scene) -> Vec<(i32, i32)> {
    scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Quad(q) if q.background == rgb(0xff00ff) => Some((q.rect.x, q.rect.y)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_caret_shows_where_it_would_go_for_as_long_as_it_is_over() {
    let value = signal("0123456789".to_string());
    let mut h = mount(|| {
        div()
            .w(800.0)
            .h(600.0)
            .child(words())
            .child(field_at(100.0, value).caret_color(rgb(0xff00ff)))
    });
    h.ctrl("a");
    h.press(IN.0, IN.1);
    h.moved(IN.0 + 30.0, IN.1);
    assert!(carets(&h.scene()).is_empty());
    h.moved(0.0, 115.0);
    let left = carets(&h.scene());
    assert_eq!(left.len(), 1);
    h.moved(400.0, 115.0);
    let right = carets(&h.scene());
    assert_eq!(right.len(), 1);
    assert!(right[0].0 > left[0].0);
    h.moved(400.0, 300.0);
    assert!(carets(&h.scene()).is_empty());
    h.moved(400.0, 115.0);
    assert_eq!(carets(&h.scene()).len(), 1);
    h.escape();
    assert!(carets(&h.scene()).is_empty());
    assert_eq!(value.get_untracked(), "0123456789");
}

#[test]
fn the_caret_also_goes_when_it_lands_or_leaves_the_window() {
    let value = signal("abc".to_string());
    let mut h = platform(|| div().child(field_at(0.0, value).caret_color(rgb(0xff00ff))));
    h.entered(10.0, 15.0, &["text/plain"]);
    assert_eq!(carets(&h.scene()).len(), 1);
    h.ui.handle_event(WindowEvent::DragLeft);
    assert!(carets(&h.scene()).is_empty());
    h.entered(10.0, 15.0, &["text/plain"]);
    h.dropped(10.0, 15.0, DragAction::Copy);
    assert!(carets(&h.scene()).is_empty());
}

#[test]
fn over_its_own_window_it_lands_with_what_the_platform_says() {
    let (from, to) = (signal("alpha beta".to_string()), signal("xyz".to_string()));
    let mut h = platform(|| div().child(field_at(0.0, from)).child(field_at(100.0, to)));
    h.click(5.0, 15.0);
    h.button(5.0, 15.0, true, Modifiers::default());
    h.button(5.0, 15.0, false, Modifiers::default());
    h.press(5.0, 15.0);
    h.moved(30.0, 15.0);
    h.drag_requests();
    h.entered(0.0, 115.0, &["text/plain"]);
    let requests = h.drag_requests();
    assert!(
        matches!(
            &requests[..],
            [PlatformRequest::AcceptDrag(Some((_, DragAction::Move)))]
        ),
        "{requests:?}"
    );
    h.dropped(0.0, 115.0, DragAction::Move);
    assert_eq!(h.drag_requests(), [PlatformRequest::FinishDrop]);
    assert_eq!(from.get_untracked(), " beta");
    assert_eq!(to.get_untracked(), "alphaxyz");
    h.ui.handle_event(WindowEvent::DragSourceEnded(DragResult::Moved));
    h.ui.update();
    assert_eq!(from.get_untracked(), " beta", "only once");
}

#[test]
fn moving_it_to_another_program_leaves_it_here() {
    let value = signal("alpha beta".to_string());
    let mut h = platform(|| div().child(field_at(0.0, value)));
    h.click(5.0, 15.0);
    h.button(5.0, 15.0, true, Modifiers::default());
    h.button(5.0, 15.0, false, Modifiers::default());
    h.press(5.0, 15.0);
    h.moved(30.0, 15.0);
    h.ui.handle_event(WindowEvent::DragSourceEnded(DragResult::Moved));
    h.ui.update();
    assert_eq!(value.get_untracked(), "alpha beta");
    assert_eq!(h.copied(), "alpha");
}

#[test]
fn the_icon_is_the_picture_and_the_pointer_is_on_what_was_pressed() {
    let mut h = platform(words);
    h.select(2.0, 60.0, 8.0);
    h.press(IN.0, IN.1);
    h.moved(IN.0 + 30.0, IN.1);
    let frame = h.ui.paint_frame(Instant::now());
    assert!(drag_layer(&frame.window).is_none());
    let icon = frame.drag_icon.expect("a surface");
    assert!(
        (icon.hotspot.x - IN.0 as i32).abs() <= 3,
        "{:?}",
        icon.hotspot
    );
    // What was selected, and a little room for what hangs out of it: not all of the text.
    assert!(
        icon.scene.width < 110 && icon.scene.height < 40,
        "{}x{}",
        icon.scene.width,
        icon.scene.height
    );
    assert_eq!(icon.scene.clear.a, 0.0);
}

#[test]
fn when_there_is_nothing_to_see_it_still_goes() {
    let mut h = platform(|| {
        div()
            .absolute()
            .left(0.0)
            .top(0.0)
            .w(400.0)
            .child(text("          "))
    });
    h.ctrl("a");
    h.press(5.0, 8.0);
    h.requests();
    h.moved(30.0, 8.0);
    assert_eq!(h.drag_requests().len(), 1);
}

#[test]
fn nothing_is_left_when_escape_calls_it_off() {
    let value = signal(String::new());
    let mut h = mount(|| text_and_field(value));
    let selected = h.select(2.0, 60.0, 8.0);
    h.press(IN.0, IN.1);
    h.moved(10.0, 115.0);
    h.escape();
    assert!(drag_layer(&h.scene()).is_none());
    assert_eq!(h.copied(), selected);
    h.release(10.0, 115.0);
    assert_eq!(value.get_untracked(), "");
    assert_eq!(h.copied(), selected);
}

/// Where the first selected glyph is in the window, with what is dragged where it started.
fn first_in_picture(scene: &Scene) -> (f32, f32) {
    let layer = drag_layer(scene).expect("it shows");
    let first = layer
        .content
        .primitives
        .iter()
        .find_map(|p| match p {
            Primitive::Glyphs(g) => g.glyphs.first().copied(),
            _ => None,
        })
        .expect("text");
    (
        first.x + layer.origin.x as f32,
        first.y + layer.origin.y as f32,
    )
}

#[test]
fn it_is_found_and_drawn_where_it_shows_when_moved_by_a_transform() {
    let mut h = mount(|| words().translate(100.0, 50.0));
    h.ctrl("a");
    h.press(120.0, 58.0);
    h.moved(124.0, 58.0);
    let (x, y) = first_in_picture(&h.scene());
    assert!((100.0..112.0).contains(&x), "{x}");
    assert!((50.0..75.0).contains(&y), "{y}");
}

#[test]
fn it_is_found_and_drawn_where_it_shows_when_scrolled() {
    let lines: String = (0..300).map(|i| format!("line {i}\n")).collect();
    let mut h = mount(move || {
        div()
            .absolute()
            .left(50.0)
            .top(20.0)
            .w(300.0)
            .h(200.0)
            .overflow_y_scroll()
            .child(text(lines))
    });
    h.ui.handle_event(WindowEvent::Scroll {
        position: at(100.0, 100.0),
        delta_x: 0.0,
        delta_y: 3000.0,
        source: ScrollSource::Unknown,
        phase: ScrollPhase::Single,
        modifiers: Modifiers::default(),
        time: 0,
    });
    h.ui.update();
    h.ctrl("a");

    // The lines where the page has them.
    let page = h.scene();
    let mut lines = Vec::new();
    for p in &page.primitives {
        if let Primitive::Layer(l) = p {
            for q in &l.content.primitives {
                if let Primitive::Glyphs(g) = q {
                    lines.push(g.glyphs[0].y + l.origin.y as f32);
                }
            }
        }
    }
    assert!(lines.len() > 3);

    h.press(60.0, 100.0);
    h.moved(60.0, 104.0);
    let scene = h.scene();
    let layer = drag_layer(&scene).expect("it shows");
    let mut checked = 0;
    for q in &layer.content.primitives {
        let Primitive::Glyphs(g) = q else { continue };
        assert_eq!(g.clip.rect.y, 20, "what is scrolled away is clipped");
        assert_eq!(g.clip.rect.bottom(), 220);
        let y = g.glyphs[0].y;
        if (20.0..220.0).contains(&y) {
            assert!(
                lines.iter().any(|l| (l - y).abs() < 0.5),
                "{y} in {lines:?}"
            );
            checked += 1;
        }
    }
    assert!(checked > 3, "{checked}");
    assert!(glyphs_of(&layer.content) < 200);
}

#[test]
fn many_lines_scroll_when_it_is_held_near_their_edge() {
    let lines: String = (0..100).map(|i| format!("line {i}\n")).collect();
    let value = signal(lines);
    let mut h = platform(|| {
        div().child(
            text_area()
                .absolute()
                .left(0.0)
                .top(0.0)
                .w(300.0)
                .h(200.0)
                .caret_color(rgb(0xff00ff))
                .value(value),
        )
    });
    h.entered(50.0, 195.0, &["text/plain"]);
    let before = carets(&h.scene());
    assert_eq!(before.len(), 1);
    let start = Instant::now();
    for i in 0..60 {
        h.ui.animate(start + std::time::Duration::from_millis(16 * i));
        h.ui.update();
    }
    h.dropped(50.0, 195.0, DragAction::Copy);
    h.ui.handle_event(WindowEvent::DropData {
        mime: "text/plain".into(),
        bytes: b"X".to_vec(),
    });
    h.ui.update();
    let got = value.get_untracked();
    let at_line = got[..got.find('X').expect("it went in")]
        .matches('\n')
        .count();
    assert!(at_line > 12, "it went in on line {at_line}");
}

#[test]
fn the_selection_in_the_document_goes_when_an_input_is_pressed_on_its_own() {
    let value = signal("alpha beta".to_string());
    let mut h = mount(|| text_and_field(value));
    h.click(5.0, 115.0);
    h.button(5.0, 115.0, true, Modifiers::default());
    h.button(5.0, 115.0, false, Modifiers::default());
    h.ctrl("a");
    assert_eq!(h.copied(), "alpha beta");
    h.select(2.0, 60.0, 8.0);
    h.press(5.0, 115.0);
    h.release(5.0, 115.0);
    assert_eq!(
        h.copied(),
        "",
        "the input has the caret, and the document nothing"
    );
}
