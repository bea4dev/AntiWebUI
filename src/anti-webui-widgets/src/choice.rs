//! Checkboxes, radio buttons and switches.
//!
//! Each has `x_with`, which gives a look of one's own the state and adds the doing to the box it
//! builds, and `styled_x`, which changes the parts of the standard look. See
//! `docs/design/33-customization.md`.

use crate::theme::{QUICK, Theme, fade, focus_ring, smooth, themed};
use anti_webui_core::prelude::*;
use anti_webui_core::view::TextSource;
use std::rc::Rc;

fn row_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.cursor(CursorIcon::Pointer).rounded(4.0))
        .focus_visible(|s| focus_ring(s, t).outline_offset(2.0))
        .disabled(|s| s.text_color(t.text_disabled).cursor(CursorIcon::Default))
}

fn row() -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(8.0)
        .select_none()
        .sheet(|| themed(row_sheet))
}

fn off_box_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        smooth(s)
            .bg(t.surface)
            .border_color(t.border_strong)
            .text_color(Color::TRANSPARENT)
    })
    .disabled(|s| s.bg(t.surface_muted).border_color(t.border))
}

fn on_box_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        smooth(s)
            .bg(t.accent)
            .border_color(t.accent)
            .text_color(t.on_accent)
    })
    .disabled(|s| {
        s.bg(t.text_disabled)
            .border_color(t.text_disabled)
            .text_color(t.surface)
    })
    .accent_background()
    .accent_border()
}

/// The state of a checkbox that can also stand for a mix.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CheckState {
    #[default]
    Off,
    On,
    /// Partly on: some of what it stands for is on (ARIA's `mixed`, the DOM's `indeterminate`).
    Mixed,
}

type Part<S> = Rc<dyn Fn(Div, S) -> Div>;

fn apply<S>(part: &Option<Part<S>>, el: Div, s: S) -> Div {
    match part {
        Some(f) => f(el, s),
        None => el,
    }
}

// Checkboxes.

/// What a checkbox's look gets to see. Reading it tracks it.
#[derive(Clone, Copy)]
pub struct CheckboxState {
    state: Memo<CheckState>,
    ui: Interaction,
}

impl CheckboxState {
    pub fn state(&self) -> CheckState {
        self.state.get()
    }

    /// On or a mix.
    pub fn checked(&self) -> bool {
        self.state.get() != CheckState::Off
    }

    /// What the pointer and the keyboard are doing to it.
    pub fn interaction(&self) -> Interaction {
        self.ui
    }

    pub fn hovered(&self) -> bool {
        self.ui.hovered()
    }

    pub fn pressed(&self) -> bool {
        self.ui.pressed()
    }

    pub fn focus_visible(&self) -> bool {
        self.ui.focus_visible()
    }
}

/// Changes to the parts of the standard checkbox. Each gets the part with its standard styles on
/// and returns it; what it says goes above the theme in every state.
#[derive(Clone, Default)]
pub struct CheckboxParts {
    row: Option<Part<CheckboxState>>,
    indicator: Option<Part<CheckboxState>>,
    label: Option<Part<CheckboxState>>,
}

impl CheckboxParts {
    pub fn new() -> Self {
        Self::default()
    }

    /// The whole of it: what is clicked and gets the focus ring.
    pub fn row(mut self, f: impl Fn(Div, CheckboxState) -> Div + 'static) -> Self {
        self.row = Some(Rc::new(f));
        self
    }

    /// The box with the mark in it.
    pub fn indicator(mut self, f: impl Fn(Div, CheckboxState) -> Div + 'static) -> Self {
        self.indicator = Some(Rc::new(f));
        self
    }

    /// What holds the label.
    pub fn label(mut self, f: impl Fn(Div, CheckboxState) -> Div + 'static) -> Self {
        self.label = Some(Rc::new(f));
        self
    }
}

fn check_box(s: CheckboxState) -> Div {
    div()
        .flex_none()
        .w(16.0)
        .h(16.0)
        .rounded(4.0)
        .border(1.0)
        .flex()
        .items_center()
        .justify_center()
        .text_size(12.0)
        .font_weight(FontWeight::BOLD)
        .sheet(move || {
            themed(if s.state() == CheckState::Off {
                off_box_sheet
            } else {
                on_box_sheet
            })
        })
        .child(
            text(move || {
                if s.state() == CheckState::On {
                    "✓"
                } else {
                    ""
                }
                .to_owned()
            })
            .line_height(LineHeight::Relative(1.0)),
        )
        .child(show(
            move || s.state() == CheckState::Mixed,
            // The color of the mark, whatever gives it that.
            || {
                div()
                    .w(8.0)
                    .h(2.0)
                    .rounded(1.0)
                    .dyn_style(|s| s.bg(crate::theme().on_accent))
            },
        ))
}

fn standard_checkbox(s: CheckboxState, label: TextSource, parts: CheckboxParts) -> Div {
    apply(&parts.row, row(), s)
        .child(apply(&parts.indicator, check_box(s), s))
        .child(apply(&parts.label, div().child(text(label)), s))
}

/// A checkbox bound to `checked`.
pub fn checkbox(checked: impl Into<Binding<bool>>, label: impl IntoTextSource) -> Div {
    styled_checkbox(checked, label, CheckboxParts::new())
}

/// A checkbox with the standard look, with its parts changed by `parts`.
pub fn styled_checkbox(
    checked: impl Into<Binding<bool>>,
    label: impl IntoTextSource,
    parts: CheckboxParts,
) -> Div {
    let label = label.into_text_source();
    checkbox_with(checked, move |s| standard_checkbox(s, label, parts))
}

/// A checkbox that can also stand for a mix, as one above a group of checkboxes does. Pressing it
/// makes a mix on, and on off.
pub fn tri_checkbox(state: impl Into<Binding<CheckState>>, label: impl IntoTextSource) -> Div {
    let label = label.into_text_source();
    tri_checkbox_with(state, move |s| {
        standard_checkbox(s, label, CheckboxParts::new())
    })
}

/// A checkbox that looks like whatever `look` builds. The checkbox makes the box that comes back
/// focusable and flips `checked` when it is clicked.
pub fn checkbox_with(
    checked: impl Into<Binding<bool>>,
    look: impl FnOnce(CheckboxState) -> Div,
) -> Div {
    let checked = checked.into();
    let state = memo(move || {
        if checked.get() {
            CheckState::On
        } else {
            CheckState::Off
        }
    });
    behave_checkbox(state, look, move || checked.set(!checked.get_untracked()))
}

/// [`tri_checkbox`] with a look of its own.
pub fn tri_checkbox_with(
    state: impl Into<Binding<CheckState>>,
    look: impl FnOnce(CheckboxState) -> Div,
) -> Div {
    let binding = state.into();
    let state = memo(move || binding.get());
    behave_checkbox(state, look, move || {
        binding.set(if binding.get_untracked() == CheckState::On {
            CheckState::Off
        } else {
            CheckState::On
        })
    })
}

fn behave_checkbox(
    state: Memo<CheckState>,
    look: impl FnOnce(CheckboxState) -> Div,
    flip: impl Fn() + 'static,
) -> Div {
    let ui = Interaction::new();
    look(CheckboxState { state, ui })
        .focusable()
        .interaction(ui)
        .access(move |a| {
            a.role(Role::Checkbox).checked(match state.get() {
                CheckState::Off => Checked::False,
                CheckState::On => Checked::True,
                CheckState::Mixed => Checked::Mixed,
            })
        })
        .on_click(move |_| flip())
}

// Radio buttons.

/// What a radio button's look gets to see. Reading it tracks it.
#[derive(Clone, Copy)]
pub struct RadioState {
    selected: Memo<bool>,
    ui: Interaction,
}

impl RadioState {
    /// Its value is the one that is chosen.
    pub fn selected(&self) -> bool {
        self.selected.get()
    }

    pub fn interaction(&self) -> Interaction {
        self.ui
    }

    pub fn hovered(&self) -> bool {
        self.ui.hovered()
    }

    pub fn pressed(&self) -> bool {
        self.ui.pressed()
    }

    pub fn focus_visible(&self) -> bool {
        self.ui.focus_visible()
    }
}

/// Changes to the parts of the standard radio button. Each gets the part with its standard styles
/// on and returns it; what it says goes above the theme in every state.
#[derive(Clone, Default)]
pub struct RadioParts {
    row: Option<Part<RadioState>>,
    indicator: Option<Part<RadioState>>,
    dot: Option<Part<RadioState>>,
    label: Option<Part<RadioState>>,
}

impl RadioParts {
    pub fn new() -> Self {
        Self::default()
    }

    /// The whole of it: what is clicked and gets the focus ring.
    pub fn row(mut self, f: impl Fn(Div, RadioState) -> Div + 'static) -> Self {
        self.row = Some(Rc::new(f));
        self
    }

    /// The circle.
    pub fn indicator(mut self, f: impl Fn(Div, RadioState) -> Div + 'static) -> Self {
        self.indicator = Some(Rc::new(f));
        self
    }

    /// The dot in the circle, which is clear while it is not chosen.
    pub fn dot(mut self, f: impl Fn(Div, RadioState) -> Div + 'static) -> Self {
        self.dot = Some(Rc::new(f));
        self
    }

    /// What holds the label.
    pub fn label(mut self, f: impl Fn(Div, RadioState) -> Div + 'static) -> Self {
        self.label = Some(Rc::new(f));
        self
    }
}

fn radio_circle(on: impl Fn() -> bool + 'static) -> Div {
    div()
        .flex_none()
        .w(16.0)
        .h(16.0)
        .rounded(8.0)
        .border(1.0)
        .flex()
        .items_center()
        .justify_center()
        .sheet(move || themed(if on() { on_box_sheet } else { off_box_sheet }))
}

fn radio_dot(on: impl Fn() -> bool + 'static) -> Div {
    div()
        .w(6.0)
        .h(6.0)
        .rounded(3.0)
        .transition_of([Prop::Background], QUICK)
        .dyn_style(move |s| {
            s.bg(if on() {
                crate::theme().on_accent
            } else {
                Color::TRANSPARENT
            })
        })
}

/// A radio button that sets `selected` to `value`. See also [`radio_group`], which is one stop for
/// Tab and goes through its options with the arrow keys.
pub fn radio<T: Clone + PartialEq + 'static>(
    selected: impl Into<Binding<T>>,
    value: T,
    label: impl IntoTextSource,
) -> Div {
    styled_radio(selected, value, label, RadioParts::new())
}

/// A radio button with the standard look, with its parts changed by `parts`.
pub fn styled_radio<T: Clone + PartialEq + 'static>(
    selected: impl Into<Binding<T>>,
    value: T,
    label: impl IntoTextSource,
    parts: RadioParts,
) -> Div {
    let label = label.into_text_source();
    radio_with(selected, value, move |s| {
        let circle = radio_circle(move || s.selected()).child(apply(
            &parts.dot,
            radio_dot(move || s.selected()),
            s,
        ));
        apply(&parts.row, row(), s)
            .child(apply(&parts.indicator, circle, s))
            .child(apply(&parts.label, div().child(text(label)), s))
    })
}

/// A radio button that looks like whatever `look` builds. The radio button makes the box that
/// comes back focusable and sets `selected` to `value` when it is clicked.
pub fn radio_with<T: Clone + PartialEq + 'static>(
    selected: impl Into<Binding<T>>,
    value: T,
    look: impl FnOnce(RadioState) -> Div,
) -> Div {
    let selected = selected.into();
    let mine = value.clone();
    let on = memo(move || selected.get() == mine);
    let ui = Interaction::new();
    look(RadioState { selected: on, ui })
        .focusable()
        .interaction(ui)
        .access(move |a| a.role(Role::Radio).checked(on.get()))
        .on_click(move |_| selected.set(value.clone()))
}

fn ringed_row_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| focus_ring(s.cursor(CursorIcon::Pointer).rounded(4.0), t).outline_offset(2.0))
        .disabled(|s| s.text_color(t.text_disabled).cursor(CursorIcon::Default))
}

/// What the look of a group of radio buttons gets to see. Reading it tracks it.
pub struct RadioGroupState<T: 'static> {
    selected: Binding<T>,
    values: Signal<Rc<Vec<T>>>,
    keyed: Signal<bool>,
}

impl<T> Clone for RadioGroupState<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for RadioGroupState<T> {}

impl<T: Clone + PartialEq + 'static> RadioGroupState<T> {
    pub fn selected(&self) -> T {
        self.selected.get()
    }

    /// Option `i` is the chosen one.
    pub fn is_selected(&self, i: usize) -> bool {
        let selected = self.selected.get();
        self.values.with(|v| v.get(i) == Some(&selected))
    }

    pub fn count(&self) -> usize {
        self.values.with(|v| v.len())
    }

    /// The group has the focus from the keyboard. The chosen option is the one to show it on.
    pub fn focus_visible(&self) -> bool {
        self.keyed.get()
    }

    /// Makes `el` option `i`: clicking it chooses it.
    pub fn item(&self, i: usize, el: Div) -> Div {
        let state = *self;
        el.access(move |a| a.role(Role::Radio).checked(state.is_selected(i)))
            .on_click(move |_| {
                if let Some(v) = state.values.with_untracked(|v| v.get(i).cloned()) {
                    state.selected.set(v);
                }
            })
    }
}

/// Radio buttons for `options`, one under the other. It is one stop for Tab, and the arrow keys go
/// through the options.
pub fn radio_group<T: Clone + PartialEq + 'static>(
    selected: impl Into<Binding<T>>,
    options: Vec<(T, String)>,
) -> Div {
    let (values, names): (Vec<T>, Vec<String>) = options.into_iter().unzip();
    radio_group_with(selected, values, move |g| {
        div()
            .flex()
            .flex_col()
            .gap(6.0)
            .children(names.into_iter().enumerate().map(|(i, name)| {
                let row = row()
                    .sheet(move || {
                        themed(if g.focus_visible() && g.is_selected(i) {
                            ringed_row_sheet
                        } else {
                            row_sheet
                        })
                    })
                    .child(
                        radio_circle(move || g.is_selected(i))
                            .child(radio_dot(move || g.is_selected(i))),
                    )
                    .child(text(name));
                g.item(i, row)
            }))
    })
}

/// A group of radio buttons that looks like whatever `look` builds; `look` makes each option with
/// [`RadioGroupState::item`]. The group is one stop for Tab, and the arrow keys choose the next or
/// the one before, around the ends.
pub fn radio_group_with<T: Clone + PartialEq + 'static>(
    selected: impl Into<Binding<T>>,
    values: Vec<T>,
    look: impl FnOnce(RadioGroupState<T>) -> Div,
) -> Div {
    let state = RadioGroupState {
        selected: selected.into(),
        values: signal(Rc::new(values)),
        keyed: signal(false),
    };
    look(state)
        .focusable()
        .access(|a| a.role(Role::RadioGroup))
        .on_focus_visible(move |v| state.keyed.set(v))
        .on_key_down(move |e| {
            let count = state.values.with_untracked(|v| v.len());
            let step = match e.key {
                Key::Named(NamedKey::ArrowDown | NamedKey::ArrowRight) => 1,
                Key::Named(NamedKey::ArrowUp | NamedKey::ArrowLeft) => count.saturating_sub(1),
                _ => return,
            };
            e.prevent_default();
            let now = state.selected.get_untracked();
            let next = state.values.with_untracked(|v| {
                let at = v.iter().position(|x| *x == now);
                v.get(at.map_or(0, |i| (i + step) % count.max(1))).cloned()
            });
            if let Some(v) = next {
                state.selected.set(v);
            }
        })
}

// Switches.

fn switch_off_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        smooth(s)
            .bg(t.border_strong)
            .rounded(10.0)
            .cursor(CursorIcon::Pointer)
    })
    .hover(|s| s.bg(t.text_muted))
    .focus_visible(|s| focus_ring(s, t))
    .disabled(|s| s.bg(t.border).cursor(CursorIcon::Default))
}

fn switch_on_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        smooth(s)
            .bg(t.success)
            .rounded(10.0)
            .cursor(CursorIcon::Pointer)
    })
    .focus_visible(|s| focus_ring(s, t))
    .disabled(|s| s.bg(t.border).cursor(CursorIcon::Default))
    .accent_background()
}

/// What the look of something that is on or off gets to see: a switch, a toggle button. Reading it
/// tracks it.
#[derive(Clone, Copy)]
pub struct ToggleState {
    on: Binding<bool>,
    ui: Interaction,
}

/// What a switch's look gets to see.
pub type SwitchState = ToggleState;

impl ToggleState {
    pub fn on(&self) -> bool {
        self.on.get()
    }

    pub fn interaction(&self) -> Interaction {
        self.ui
    }

    pub fn hovered(&self) -> bool {
        self.ui.hovered()
    }

    pub fn pressed(&self) -> bool {
        self.ui.pressed()
    }

    pub fn focus_visible(&self) -> bool {
        self.ui.focus_visible()
    }
}

/// Changes to the parts of the standard switch. Each gets the part with its standard styles on and
/// returns it; what it says goes above the theme in every state.
#[derive(Clone, Default)]
pub struct SwitchParts {
    track: Option<Part<SwitchState>>,
    thumb: Option<Part<SwitchState>>,
}

impl SwitchParts {
    pub fn new() -> Self {
        Self::default()
    }

    /// The box that is clicked, which holds the thumb.
    pub fn track(mut self, f: impl Fn(Div, SwitchState) -> Div + 'static) -> Self {
        self.track = Some(Rc::new(f));
        self
    }

    /// What moves. It goes to the far end when on, whatever its size.
    pub fn thumb(mut self, f: impl Fn(Div, SwitchState) -> Div + 'static) -> Self {
        self.thumb = Some(Rc::new(f));
        self
    }
}

/// An on/off switch bound to `on`.
pub fn switch(on: impl Into<Binding<bool>>) -> Div {
    styled_switch(on, SwitchParts::new())
}

/// A switch with the standard look, with its parts changed by `parts`.
pub fn styled_switch(on: impl Into<Binding<bool>>, parts: SwitchParts) -> Div {
    switch_with(on, move |s| standard_switch(s, parts))
}

/// A switch that looks like whatever `look` builds. The switch makes the box that comes back
/// focusable and flips `on` when it is clicked.
pub fn switch_with(on: impl Into<Binding<bool>>, look: impl FnOnce(SwitchState) -> Div) -> Div {
    toggle_as(Role::Switch, on.into(), look)
}

/// Something that is on or off and looks like whatever `look` builds, like a toggle button or a
/// filter chip. It makes the box that comes back focusable and flips `on` when it is clicked.
pub fn toggle_with(on: impl Into<Binding<bool>>, look: impl FnOnce(ToggleState) -> Div) -> Div {
    toggle_as(Role::ToggleButton, on.into(), look)
}

fn toggle_as(role: Role, on: Binding<bool>, look: impl FnOnce(ToggleState) -> Div) -> Div {
    let ui = Interaction::new();
    look(ToggleState { on, ui })
        .select_none()
        .focusable()
        .interaction(ui)
        .access(move |a| a.role(role).checked(on.get()))
        .on_click(move |_| on.set(!on.get_untracked()))
}

fn standard_switch(s: SwitchState, parts: SwitchParts) -> Div {
    let thumb = div()
        .flex_none()
        .w(16.0)
        .h(16.0)
        .rounded(8.0)
        .bg(Color::WHITE)
        .shadow_sm();
    let track = div()
        .flex_none()
        .w(36.0)
        .h(20.0)
        .p(2.0)
        .flex()
        .flex_row()
        .items_center()
        .sheet(move || {
            themed(if s.on() {
                switch_on_sheet
            } else {
                switch_off_sheet
            })
        })
        // The spacer grows from nothing to all the room the thumb leaves, so the thumb goes to the
        // far end whatever its size and the track's.
        .child({
            let moved = fade(move || s.on());
            div().dyn_style(move |st| st.flex_grow(moved.get()))
        })
        .child(apply(&parts.thumb, thumb, s));
    apply(&parts.track, track, s)
}
