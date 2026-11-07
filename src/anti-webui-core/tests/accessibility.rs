//! What assistive technology sees and asks for. See `docs/design/34-accessibility.md`.

use anti_webui_core::Ui;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{Scale, SizeI};
use anti_webui_core::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

fn mount<V: IntoView>(f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(600, 400),
        scale: Scale::ONE,
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    ui
}

#[test]
fn what_says_nothing_is_left_out_and_a_button_is_named_by_its_text() {
    let _ui = mount(|| {
        div().child(
            div().child(
                div()
                    .focusable()
                    .access(|a| a.role(Role::Button))
                    .child(div().child("Save")),
            ),
        )
    });
    let tree = accessibility::snapshot();
    let root = tree.get(tree.root).unwrap();
    assert_eq!(root.children.len(), 1, "the boxes around it say nothing");
    let button = tree.get(root.children[0]).unwrap();
    assert_eq!(button.role, Role::Button);
    assert_eq!(button.label.as_deref(), Some("Save"));
    assert!(button.focusable);
    let text = tree.get(button.children[0]).unwrap();
    assert_eq!(
        (text.role, text.label.as_deref()),
        (Role::Text, Some("Save"))
    );
    assert!(button.bounds.width > 0.0);
}

#[test]
fn a_label_and_states_come_from_what_is_said() {
    let on = signal(false);
    let off = signal(false);
    let _ui = mount(move || {
        div().child(
            div()
                .focusable()
                .disabled(move || off.get())
                .access(move |a| a.role(Role::Switch).label("Wi-Fi").checked(on.get()))
                .child("ignored for the name"),
        )
    });
    let tree = accessibility::snapshot();
    let switch = tree.find(Role::Switch, "Wi-Fi").expect("by its label");
    assert_eq!(switch.checked, Some(Checked::False));
    assert!(!switch.disabled);
    on.set(true);
    off.set(true);
    let tree = accessibility::snapshot();
    let switch = tree.find(Role::Switch, "Wi-Fi").unwrap();
    assert_eq!(switch.checked, Some(Checked::True));
    assert!(switch.disabled);
}

#[test]
fn what_is_hidden_is_left_out_with_what_is_in_it() {
    let _ui = mount(|| {
        div()
            .child(div().access(|a| a.hidden()).child("Decoration"))
            .child("Shown")
    });
    let tree = accessibility::snapshot();
    assert!(tree.find(Role::Text, "Decoration").is_none());
    assert!(tree.find(Role::Text, "Shown").is_some());
}

#[test]
fn what_is_typed_in_is_a_text_input_with_its_value() {
    let value = signal("hello".to_string());
    let _ui = mount(move || div().child(text_input().value(value).access(|a| a.label("Name"))));
    let tree = accessibility::snapshot();
    let input = tree.find(Role::TextInput, "Name").expect("the input");
    assert_eq!(input.value_text.as_deref(), Some("hello"));
}

#[test]
fn watchers_hear_of_changes_and_only_of_changes() {
    let on = signal(false);
    let mut ui = mount(move || {
        div().child(
            div()
                .focusable()
                .access(move |a| a.role(Role::Checkbox).label("A").checked(on.get())),
        )
    });
    let heard: Rc<RefCell<Vec<Option<Checked>>>> = Rc::default();
    let log = heard.clone();
    let watch = accessibility::watch(move |tree| {
        log.borrow_mut()
            .push(tree.find(Role::Checkbox, "A").and_then(|n| n.checked));
    });
    assert_eq!(*heard.borrow(), vec![Some(Checked::False)], "at once");
    ui.update();
    assert_eq!(heard.borrow().len(), 1, "nothing changed");
    on.set(true);
    ui.update();
    assert_eq!(heard.borrow().last(), Some(&Some(Checked::True)));
    let count = heard.borrow().len();
    watch.stop();
    on.set(false);
    ui.update();
    assert_eq!(heard.borrow().len(), count, "stopped");
}

#[test]
fn assistive_technology_can_focus_and_press() {
    let clicks = Rc::new(Cell::new(0));
    let count = clicks.clone();
    let mut ui = mount(move || {
        div().child(
            div()
                .w(80.0)
                .h(30.0)
                .focusable()
                .access(|a| a.role(Role::Button).label("Go"))
                .on_click(move |_| count.set(count.get() + 1)),
        )
    });
    let id = accessibility::snapshot()
        .find(Role::Button, "Go")
        .unwrap()
        .id;
    assert!(accessibility::perform(
        id,
        accessibility::AccessAction::Focus
    ));
    ui.update();
    assert_eq!(accessibility::snapshot().focus, Some(id));
    accessibility::perform(id, accessibility::AccessAction::Click);
    ui.update();
    assert_eq!(clicks.get(), 1);
}

#[test]
fn increment_is_the_up_key_on_what_it_is_for() {
    let value = signal(5);
    let mut ui = mount(move || {
        div().child(
            div()
                .w(80.0)
                .h(20.0)
                .focusable()
                .access(move |a| a.role(Role::Slider).value(value.get() as f64, 0.0, 10.0))
                .on_key_down(move |e| match e.key {
                    Key::Named(NamedKey::ArrowUp) => value.update(|v| *v += 1),
                    Key::Named(NamedKey::ArrowDown) => value.update(|v| *v -= 1),
                    _ => {}
                }),
        )
    });
    let slider = accessibility::snapshot()
        .nodes
        .into_iter()
        .find(|n| n.role == Role::Slider)
        .unwrap();
    assert_eq!(slider.value.unwrap().now, 5.0);
    accessibility::perform(slider.id, accessibility::AccessAction::Increment);
    ui.update();
    assert_eq!(value.get(), 6);
    accessibility::perform(slider.id, accessibility::AccessAction::Decrement);
    accessibility::perform(slider.id, accessibility::AccessAction::Decrement);
    ui.update();
    assert_eq!(value.get(), 4);
}
