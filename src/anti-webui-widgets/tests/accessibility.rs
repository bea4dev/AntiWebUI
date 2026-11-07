//! What the standard controls say to assistive technology, which comes from their behavior and so
//! stays with any look. See `docs/design/34-accessibility.md`.

mod common;

use anti_webui_core::prelude::*;
use anti_webui_widgets::*;
use common::*;

fn column_of(child: impl IntoView) -> Div {
    div().flex().flex_col().items_start().p(20.0).child(child)
}

fn with_role(role: Role) -> Vec<accessibility::AccessNode> {
    accessibility::snapshot()
        .nodes
        .into_iter()
        .filter(|n| n.role == role)
        .collect()
}

#[test]
fn controls_say_what_they_are_and_their_state() {
    let (on, checked, chosen) = (signal(true), signal(false), signal(1));
    let _ui = start(move || {
        column_of(button("Save"))
            .child(switch(on))
            .child(checkbox(checked, "Agree"))
            .child(radio(chosen, 1, "One"))
            .child(radio(chosen, 2, "Two"))
    });
    let tree = accessibility::snapshot();
    assert!(tree.find(Role::Button, "Save").is_some());
    assert_eq!(with_role(Role::Switch)[0].checked, Some(Checked::True));
    let agree = tree
        .find(Role::Checkbox, "Agree")
        .expect("named by its label");
    assert_eq!(agree.checked, Some(Checked::False));
    assert_eq!(
        tree.find(Role::Radio, "One").unwrap().checked,
        Some(Checked::True)
    );
    assert_eq!(
        tree.find(Role::Radio, "Two").unwrap().checked,
        Some(Checked::False)
    );
}

#[test]
fn a_look_of_ones_own_says_the_same() {
    let on = signal(false);
    let _ui = start(move || column_of(switch_with(on, |_| div().w(40.0).h(20.0))));
    let switch = &with_role(Role::Switch)[0];
    assert_eq!(switch.checked, Some(Checked::False));
    assert!(switch.focusable);
}

#[test]
fn a_slider_has_its_value_and_goes_with_increment() {
    let value = signal(50.0f32);
    let mut ui = start(move || column_of(slider(value, 0.0, 100.0).w(200.0)));
    let slider = with_role(Role::Slider).remove(0);
    let v = slider.value.unwrap();
    assert_eq!((v.now, v.min, v.max), (50.0, 0.0, 100.0));
    accessibility::perform(slider.id, accessibility::AccessAction::Increment);
    ui.update();
    assert_eq!(value.get(), 55.0);
}

#[test]
fn tabs_say_which_is_chosen() {
    let selected = signal(1usize);
    let _ui = start(move || column_of(tabs(selected, vec![tab("A", div), tab("B", div)])));
    let tree = accessibility::snapshot();
    assert_eq!(with_role(Role::TabList).len(), 1);
    assert_eq!(tree.find(Role::Tab, "A").unwrap().selected, Some(false));
    assert_eq!(tree.find(Role::Tab, "B").unwrap().selected, Some(true));
    assert_eq!(with_role(Role::TabPanel).len(), 1);
}

#[test]
fn a_menu_says_its_items() {
    let open = signal(true);
    let mut ui = start(move || {
        column_of(button("File")).child(menu(
            move || open.get(),
            move || open.set(false),
            vec![
                menu_item("New").into(),
                menu_item("Wrap").checked_when(|| true).into(),
                submenu("More", vec![]),
            ],
        ))
    });
    let shown = popups_shown(&mut ui);
    place(&mut ui, &shown[0]);
    let tree = accessibility::snapshot();
    assert_eq!(with_role(Role::Menu).len(), 1);
    assert!(tree.find(Role::MenuItem, "New").is_some());
    let wrap = tree
        .find(Role::MenuItemCheckbox, "Wrap")
        .expect("the check is its state");
    assert_eq!(wrap.checked, Some(Checked::True));
    assert_eq!(
        tree.find(Role::MenuItem, "More").map(|n| n.expanded),
        Some(Some(false))
    );
}
