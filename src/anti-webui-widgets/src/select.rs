//! Choosing from lists.
//!
//! Each has `x_with`, which keeps what it does (the keys, the pointer, what is chosen) and leaves
//! what it looks like to the program. See `docs/design/33-customization.md`.

use crate::theme::{Theme, focus_ring, smooth, themed};
use anti_webui_core::prelude::*;
use std::rc::Rc;

fn select_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        smooth(s)
            .bg(t.surface)
            .border_color(t.border)
            .text_color(t.text)
            .cursor(CursorIcon::Pointer)
    })
    .hover(|s| s.bg(t.hover).border_color(t.border_strong))
    .active(|s| s.bg(t.pressed))
    .focus_visible(|s| focus_ring(s, t))
    .disabled(|s| {
        s.bg(t.surface_muted)
            .border_color(t.border)
            .text_color(t.text_disabled)
            .cursor(CursorIcon::Default)
    })
}

fn option_sheet(_: &Theme) -> Sheet {
    Sheet::new(|s| s.cursor(CursorIcon::Pointer))
}

fn lit_option_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        s.bg(t.accent)
            .text_color(t.on_accent)
            .cursor(CursorIcon::Pointer)
    })
    .accent_background()
}

/// The options of a list and the place of each row, shared by the states below.
struct Options<T: 'static> {
    values: Signal<Rc<Vec<T>>>,
    refs: Signal<Rc<Vec<NodeRef>>>,
}

impl<T> Clone for Options<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Options<T> {}

impl<T: Clone + PartialEq + 'static> Options<T> {
    fn new(values: Vec<T>) -> Self {
        let refs = values.iter().map(|_| node_ref()).collect();
        Self {
            values: signal(Rc::new(values)),
            refs: signal(Rc::new(refs)),
        }
    }

    fn count(&self) -> usize {
        self.values.with_untracked(|v| v.len())
    }

    fn get(&self, i: usize) -> Option<T> {
        self.values.with_untracked(|v| v.get(i).cloned())
    }

    fn position(&self, value: &T) -> Option<usize> {
        self.values
            .with_untracked(|v| v.iter().position(|x| x == value))
    }

    fn reveal(&self, i: usize) {
        if let Some(row) = self.refs.with_untracked(|r| r.get(i).copied()) {
            row.scroll_into_view();
        }
    }

    fn row(&self, i: usize, el: Div) -> Div {
        match self.refs.with_untracked(|r| r.get(i).copied()) {
            Some(r) => el.node_ref(r),
            None => el,
        }
    }
}

// Pull-down lists.

/// What the look of a pull-down list gets to see. Reading it tracks it.
pub struct SelectState<T: 'static> {
    value: Binding<T>,
    options: Options<T>,
    open: Signal<bool>,
    lit: Signal<usize>,
}

impl<T> Clone for SelectState<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for SelectState<T> {}

impl<T: Clone + PartialEq + 'static> SelectState<T> {
    pub fn value(&self) -> T {
        self.value.get()
    }

    /// The index of the chosen option.
    pub fn selected(&self) -> Option<usize> {
        let v = self.value.get();
        self.options
            .values
            .with(|all| all.iter().position(|x| *x == v))
    }

    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    /// Option `i` is where the keys or the pointer are, while the list is open.
    pub fn is_lit(&self, i: usize) -> bool {
        self.lit.get() == i
    }

    pub fn count(&self) -> usize {
        self.options.values.with(|v| v.len())
    }

    /// Makes `el` option `i` of the list: the pointer lights it and a click chooses it.
    pub fn option(&self, i: usize, el: Div) -> Div {
        let state = *self;
        self.options
            .row(i, el)
            .access(move |a| a.role(Role::ListOption).selected(state.is_lit(i)))
            .on_hover(move |inside| {
                if inside {
                    state.lit.set(i);
                }
            })
            .on_click(move |e| {
                e.stop_propagation();
                state.choose(i);
            })
    }

    fn choose(&self, i: usize) {
        if let Some(v) = self.options.get(i) {
            self.value.set(v);
        }
        self.open.set(false);
    }

    fn show_list(&self) {
        let at = self.options.position(&self.value.get_untracked());
        self.lit.set(at.unwrap_or(0));
        self.open.set(true);
    }

    fn key(&self, e: &KeyboardEvent) {
        let last = self.options.count().saturating_sub(1);
        let is_open = self.open.get_untracked();
        let moved = match &e.key {
            Key::Named(NamedKey::Enter | NamedKey::Space) => {
                e.prevent_default();
                if is_open {
                    self.choose(self.lit.get_untracked());
                } else {
                    self.show_list();
                }
                return;
            }
            Key::Named(NamedKey::Tab) => {
                self.open.set(false);
                return;
            }
            Key::Named(NamedKey::ArrowDown) if !is_open => {
                e.prevent_default();
                self.show_list();
                return;
            }
            Key::Named(NamedKey::ArrowDown) => (self.lit.get_untracked() + 1).min(last),
            Key::Named(NamedKey::ArrowUp) => self.lit.get_untracked().saturating_sub(1),
            Key::Named(NamedKey::Home) => 0,
            Key::Named(NamedKey::End) => last,
            _ => return,
        };
        e.prevent_default();
        if is_open {
            self.lit.set(moved);
            self.options.reveal(moved);
        }
    }
}

type ListOf<T> = Rc<dyn Fn(SelectState<T>) -> Div>;

/// A pull-down list bound to `value`. Opens where it is pressed, like a browser's `<select>`.
pub fn select<T: Clone + PartialEq + 'static>(
    value: impl Into<Binding<T>>,
    options: Vec<(T, String)>,
) -> Div {
    let (values, names): (Vec<T>, Vec<String>) = options.into_iter().unzip();
    let names = Rc::new(names);
    select_with(value, values, move |s| {
        let label_names = names.clone();
        let field = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(12.0)
            .px(12.0)
            .py(5.0)
            .rounded(6.0)
            .border(1.0)
            .font_weight(FontWeight::MEDIUM)
            .sheet(|| themed(select_sheet))
            .child(text(move || {
                s.selected()
                    .and_then(|i| label_names.get(i).cloned())
                    .unwrap_or_default()
            }))
            .child(text("▾").dyn_style(|s| s.text_color(crate::theme().text_muted)));
        let list: ListOf<T> = Rc::new(move |s| {
            div()
                .flex()
                .flex_col()
                .p(4.0)
                .max_h(280.0)
                .rounded(8.0)
                .border(1.0)
                .shadow_lg()
                .overflow_y_scroll()
                .sheet(|| themed(crate::menu::surface_sheet))
                .children(names.iter().enumerate().map(|(i, name)| {
                    let row = div()
                        .px(8.0)
                        .py(5.0)
                        .rounded(4.0)
                        .sheet(move || {
                            themed(if s.is_lit(i) {
                                lit_option_sheet
                            } else {
                                option_sheet
                            })
                        })
                        .child(text(name.clone()));
                    s.option(i, row)
                }))
        });
        (field, move || list(s))
    })
}

/// A pull-down list that looks like whatever `setup` builds. It gets the state once and returns the
/// box that shows the value, which gets the focus, the keys and the pointer, and what builds the
/// list each time it opens, with each option made with [`SelectState::option`]. The list hangs
/// under the box, as wide as it. The keys go as in a browser's `<select>`: Down or Enter opens it,
/// Up and Down go through it, Enter chooses.
pub fn select_with<T, F>(
    value: impl Into<Binding<T>>,
    values: Vec<T>,
    setup: impl FnOnce(SelectState<T>) -> (Div, F),
) -> Div
where
    T: Clone + PartialEq + 'static,
    F: Fn() -> Div + 'static,
{
    let state = SelectState {
        value: value.into(),
        options: Options::new(values),
        open: signal(false),
        lit: signal(0),
    };
    let (field, list) = setup(state);
    let list = Rc::new(list);
    field
        .select_none()
        .focusable()
        .access(move |a| a.role(Role::ComboBox).expanded(state.open.get()))
        // What is pressed in the list comes here too.
        .on_pointer_down(move |e| {
            if e.button == Some(PointerButton::Primary) && !state.open.get_untracked() {
                state.show_list();
            }
        })
        .on_key_down(move |e| state.key(e))
        .child(show(
            move || state.open.get(),
            move || {
                let surface = popup()
                    .gap(4)
                    .match_anchor_width()
                    .on_dismiss(move || state.open.set(false))
                    // What it is declared in has nothing to do with what is pressed here.
                    .child(
                        list()
                            .access(|a| a.role(Role::ListBox))
                            .on_click(|e| e.stop_propagation()),
                    );
                state.options.reveal(state.lit.get_untracked());
                surface
            },
        ))
}

// Lists that stay open.

fn list_box_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(t.surface).border_color(t.border))
        .focus_visible(|s| focus_ring(s, t))
        .disabled(|s| s.bg(t.surface_muted).text_color(t.text_disabled))
}

fn row_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        s.border_color(Color::TRANSPARENT)
            .cursor(CursorIcon::Pointer)
    })
    .hover(|s| s.bg(t.hover))
    .disabled(|s| s.bg(Color::TRANSPARENT).cursor(CursorIcon::Default))
}

fn picked_row_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        s.bg(t.selected)
            .border_color(Color::TRANSPARENT)
            .cursor(CursorIcon::Pointer)
    })
    .disabled(|s| s.bg(t.surface_muted).cursor(CursorIcon::Default))
}

fn cursor_row_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        s.bg(t.selected)
            .border_color(t.accent)
            .cursor(CursorIcon::Pointer)
    })
    .disabled(|s| s.bg(t.surface_muted).cursor(CursorIcon::Default))
    .accent_border()
}

fn scroller() -> Div {
    div()
        .flex()
        .flex_col()
        .max_h(240.0)
        .rounded(6.0)
        .border(1.0)
        .p(2.0)
        .overflow_y_scroll()
        .sheet(|| themed(list_box_sheet))
}

fn option_row(name: String) -> Div {
    div()
        .px(8.0)
        .py(3.0)
        .rounded(4.0)
        .border(1.0)
        .child(text(name))
}

/// What the look of a list that stays open gets to see. Reading it tracks it.
pub struct ListBoxState<T: 'static> {
    value: Binding<T>,
    options: Options<T>,
    keyed: Signal<bool>,
}

impl<T> Clone for ListBoxState<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for ListBoxState<T> {}

impl<T: Clone + PartialEq + 'static> ListBoxState<T> {
    pub fn value(&self) -> T {
        self.value.get()
    }

    pub fn is_selected(&self, i: usize) -> bool {
        let v = self.value.get();
        self.options.values.with(|all| all.get(i) == Some(&v))
    }

    /// The list has the focus from the keyboard.
    pub fn focus_visible(&self) -> bool {
        self.keyed.get()
    }

    /// Makes `el` option `i`: a click chooses it.
    pub fn option(&self, i: usize, el: Div) -> Div {
        let state = *self;
        let el = el.access(move |a| a.role(Role::ListOption).selected(state.is_selected(i)));
        self.options.row(i, el).on_click(move |_| {
            if let Some(v) = state.options.get(i) {
                state.value.set(v);
            }
        })
    }
}

/// A list that stays open, bound to `value` (`<select size="n">`).
pub fn list_box<T: Clone + PartialEq + 'static>(
    value: impl Into<Binding<T>>,
    options: Vec<(T, String)>,
) -> Div {
    let (values, names): (Vec<T>, Vec<String>) = options.into_iter().unzip();
    list_box_with(value, values, move |s| {
        scroller().children(names.into_iter().enumerate().map(|(i, name)| {
            let row = option_row(name).sheet(move || {
                themed(if s.is_selected(i) {
                    picked_row_sheet
                } else {
                    row_sheet
                })
            });
            s.option(i, row)
        }))
    })
}

/// A list that stays open and looks like whatever `look` builds, with each option made with
/// [`ListBoxState::option`]. The box that comes back gets the focus, and Up, Down, Home and End
/// choose.
pub fn list_box_with<T: Clone + PartialEq + 'static>(
    value: impl Into<Binding<T>>,
    values: Vec<T>,
    look: impl FnOnce(ListBoxState<T>) -> Div,
) -> Div {
    let state = ListBoxState {
        value: value.into(),
        options: Options::new(values),
        keyed: signal(false),
    };
    look(state)
        .select_none()
        .focusable()
        .access(|a| a.role(Role::ListBox))
        .on_focus_visible(move |v| state.keyed.set(v))
        .on_key_down(move |e| {
            let options = state.options;
            let last = options.count().saturating_sub(1);
            let at = options.position(&state.value.get_untracked());
            let next = match e.key {
                Key::Named(NamedKey::ArrowDown) => at.map_or(0, |i| (i + 1).min(last)),
                Key::Named(NamedKey::ArrowUp) => at.map_or(0, |i| i.saturating_sub(1)),
                Key::Named(NamedKey::Home) => 0,
                Key::Named(NamedKey::End) => last,
                _ => return,
            };
            e.prevent_default();
            if let Some(v) = options.get(next) {
                state.value.set(v);
                options.reveal(next);
            }
        })
}

/// What the look of a list where several can be chosen gets to see. Reading it tracks it.
pub struct MultiListBoxState<T: 'static> {
    values: Binding<Vec<T>>,
    options: Options<T>,
    cursor: Signal<usize>,
    anchor: Signal<usize>,
    keyed: Signal<bool>,
}

impl<T> Clone for MultiListBoxState<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for MultiListBoxState<T> {}

impl<T: Clone + PartialEq + 'static> MultiListBoxState<T> {
    pub fn is_selected(&self, i: usize) -> bool {
        let Some(v) = self.options.values.with(|all| all.get(i).cloned()) else {
            return false;
        };
        self.values.get().contains(&v)
    }

    /// The option the keys are on.
    pub fn cursor(&self) -> usize {
        self.cursor.get()
    }

    /// The list has the focus from the keyboard; the cursor is the one to show it on.
    pub fn focus_visible(&self) -> bool {
        self.keyed.get()
    }

    /// Makes `el` option `i`: a click chooses only it, with Ctrl adds or takes it away, and with
    /// Shift chooses from the last one clicked to it.
    pub fn option(&self, i: usize, el: Div) -> Div {
        let state = *self;
        let el = el.access(move |a| a.role(Role::ListOption).selected(state.is_selected(i)));
        self.options.row(i, el).on_click(move |e| {
            state.cursor.set(i);
            if e.modifiers.shift {
                state.range(i);
                return;
            }
            state.anchor.set(i);
            if e.modifiers.primary() {
                state.flip(i);
            } else {
                state.only(i);
            }
        })
    }

    fn range(&self, to: usize) {
        let from = self.anchor.get_untracked();
        let (a, b) = (from.min(to), from.max(to));
        let picked = self
            .options
            .values
            .with_untracked(|all| all[a..=b].to_vec());
        self.values.set(picked);
    }

    fn flip(&self, i: usize) {
        let Some(v) = self.options.get(i) else {
            return;
        };
        let mut all = self.values.get_untracked();
        match all.iter().position(|o| *o == v) {
            Some(at) => {
                all.remove(at);
            }
            None => all.push(v),
        }
        self.values.set(all);
    }

    fn only(&self, i: usize) {
        if let Some(v) = self.options.get(i) {
            self.values.set(vec![v]);
        }
    }
}

/// A list where several can be chosen, bound to `values` (`<select multiple>`). Ctrl adds to what
/// is chosen and Shift goes from the last one, with the pointer and with the arrow keys; Space
/// switches the one at the cursor.
pub fn multi_list_box<T: Clone + PartialEq + 'static>(
    values: impl Into<Binding<Vec<T>>>,
    options: Vec<(T, String)>,
) -> Div {
    let (all, names): (Vec<T>, Vec<String>) = options.into_iter().unzip();
    multi_list_box_with(values, all, move |s| {
        scroller().children(names.into_iter().enumerate().map(|(i, name)| {
            let row = option_row(name).sheet(move || {
                themed(if s.focus_visible() && s.cursor() == i {
                    cursor_row_sheet
                } else if s.is_selected(i) {
                    picked_row_sheet
                } else {
                    row_sheet
                })
            });
            s.option(i, row)
        }))
    })
}

/// [`multi_list_box`] that looks like whatever `look` builds, with each option made with
/// [`MultiListBoxState::option`].
pub fn multi_list_box_with<T: Clone + PartialEq + 'static>(
    values: impl Into<Binding<Vec<T>>>,
    all: Vec<T>,
    look: impl FnOnce(MultiListBoxState<T>) -> Div,
) -> Div {
    let state = MultiListBoxState {
        values: values.into(),
        options: Options::new(all),
        cursor: signal(0),
        anchor: signal(0),
        keyed: signal(false),
    };
    look(state)
        .select_none()
        .focusable()
        .access(|a| a.role(Role::ListBox).multiselectable())
        .on_focus_visible(move |v| state.keyed.set(v))
        .on_key_down(move |e| {
            let last = state.options.count().saturating_sub(1);
            let at = state.cursor.get_untracked();
            let next = match e.key {
                Key::Named(NamedKey::ArrowDown) => (at + 1).min(last),
                Key::Named(NamedKey::ArrowUp) => at.saturating_sub(1),
                Key::Named(NamedKey::Home) => 0,
                Key::Named(NamedKey::End) => last,
                Key::Named(NamedKey::Space) => {
                    e.prevent_default();
                    state.anchor.set(at);
                    state.flip(at);
                    return;
                }
                _ => return,
            };
            e.prevent_default();
            state.keyed.set(true);
            state.cursor.set(next);
            state.options.reveal(next);
            if e.modifiers.shift {
                state.range(next);
            } else if !e.modifiers.primary() {
                state.anchor.set(next);
                state.only(next);
            }
        })
}
