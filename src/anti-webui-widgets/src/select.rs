//! Choosing from lists.

use crate::theme::{Theme, ring, shared, smooth};
use anti_webui_core::prelude::*;
use std::cell::RefCell;
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
    .focus_visible(|s| ring(s, t))
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

/// A pull-down list bound to `value`. Opens where it is pressed, like a browser's `<select>`.
pub fn select<T: Clone + PartialEq + 'static>(value: Signal<T>, options: Vec<(T, String)>) -> Div {
    let options = Rc::new(options);
    let open = signal(false);
    let highlighted = signal(0usize);
    let rows = Rc::new(RefCell::new(Vec::<NodeRef>::new()));
    let current = {
        let options = options.clone();
        move || {
            let v = value.get_untracked();
            options.iter().position(|(o, _)| *o == v).unwrap_or(0)
        }
    };
    let show_list = {
        let current = current.clone();
        move || {
            highlighted.set(current());
            open.set(true);
        }
    };
    let keys = {
        let (options, rows) = (options.clone(), rows.clone());
        let show_list = show_list.clone();
        move |e: &KeyboardEvent| {
            let last = options.len().saturating_sub(1);
            let is_open = open.get_untracked();
            let moved = match &e.key {
                Key::Named(NamedKey::Enter | NamedKey::Space) => {
                    e.prevent_default();
                    if is_open {
                        if let Some((v, _)) = options.get(highlighted.get_untracked()) {
                            value.set(v.clone());
                        }
                        open.set(false);
                    } else {
                        show_list();
                    }
                    return;
                }
                Key::Named(NamedKey::Tab) => {
                    open.set(false);
                    return;
                }
                Key::Named(NamedKey::ArrowDown) if !is_open => {
                    e.prevent_default();
                    show_list();
                    return;
                }
                Key::Named(NamedKey::ArrowDown) => (highlighted.get_untracked() + 1).min(last),
                Key::Named(NamedKey::ArrowUp) => highlighted.get_untracked().saturating_sub(1),
                Key::Named(NamedKey::Home) => 0,
                Key::Named(NamedKey::End) => last,
                _ => return,
            };
            e.prevent_default();
            if is_open {
                highlighted.set(moved);
                if let Some(row) = rows.borrow().get(moved) {
                    row.scroll_into_view();
                }
            }
        }
    };
    let label = {
        let options = options.clone();
        move || {
            let v = value.get();
            options
                .iter()
                .find(|(o, _)| *o == v)
                .map_or(String::new(), |(_, label)| label.clone())
        }
    };
    let list = move || {
        let refs: Vec<NodeRef> = options.iter().map(|_| node_ref()).collect();
        *rows.borrow_mut() = refs.clone();
        let items = options.iter().enumerate().map(|(i, (v, name))| {
            let v = v.clone();
            div()
                .px(8.0)
                .py(5.0)
                .rounded(4.0)
                .node_ref(refs[i])
                .sheet(move || {
                    shared(if highlighted.get() == i {
                        lit_option_sheet
                    } else {
                        option_sheet
                    })
                })
                .on_hover(move |inside| {
                    if inside {
                        highlighted.set(i);
                    }
                })
                .on_click(move |e| {
                    e.stop_propagation();
                    value.set(v.clone());
                    open.set(false);
                })
                .child(text(name.clone()))
        });
        let list = popup()
            .gap(4)
            .match_anchor_width()
            .on_dismiss(move || open.set(false))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .p(4.0)
                    .max_h(280.0)
                    .rounded(8.0)
                    .border(1.0)
                    .shadow_lg()
                    .overflow_y_scroll()
                    .sheet(|| shared(crate::menu::surface_sheet))
                    // What it is declared in has nothing to do with what is pressed here.
                    .on_click(|e| e.stop_propagation())
                    .children(items.collect::<Vec<_>>()),
            );
        if let Some(row) = refs.get(highlighted.get_untracked()) {
            row.scroll_into_view();
        }
        list
    };
    div()
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
        .select_none()
        .focusable()
        .sheet(|| shared(select_sheet))
        // What is pressed in the list comes here too.
        .on_pointer_down(move |e| {
            if e.button == Some(PointerButton::Primary) && !open.get_untracked() {
                show_list();
            }
        })
        .on_key_down(keys)
        .child(text(label))
        .child(text("▾").dyn_style(|s| s.text_color(crate::theme().text_muted)))
        .child(show(move || open.get(), list))
}

fn list_box_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(t.surface).border_color(t.border))
        .focus_visible(|s| ring(s, t))
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
        .select_none()
        .focusable()
        .sheet(|| shared(list_box_sheet))
}

fn option_row(name: String, r: NodeRef) -> Div {
    div()
        .px(8.0)
        .py(3.0)
        .rounded(4.0)
        .border(1.0)
        .node_ref(r)
        .child(text(name))
}

/// A list that stays open, bound to `value` (`<select size="n">`).
pub fn list_box<T: Clone + PartialEq + 'static>(
    value: Signal<T>,
    options: Vec<(T, String)>,
) -> Div {
    let options = Rc::new(options);
    let refs: Rc<Vec<NodeRef>> = Rc::new(options.iter().map(|_| node_ref()).collect());
    let keys = options.clone();
    let rows = options.iter().enumerate().map(|(i, (v, name))| {
        let (mine, picked) = (v.clone(), v.clone());
        option_row(name.clone(), refs[i])
            .sheet(move || {
                shared(if value.with(|c| *c == mine) {
                    picked_row_sheet
                } else {
                    row_sheet
                })
            })
            .on_click(move |_| value.set(picked.clone()))
    });
    let list = rows.collect::<Vec<_>>();
    scroller()
        .on_key_down(move |e| {
            let last = keys.len().saturating_sub(1);
            let at = value.with_untracked(|c| keys.iter().position(|(v, _)| v == c));
            let next = match e.key {
                Key::Named(NamedKey::ArrowDown) => at.map_or(0, |i| (i + 1).min(last)),
                Key::Named(NamedKey::ArrowUp) => at.map_or(0, |i| i.saturating_sub(1)),
                Key::Named(NamedKey::Home) => 0,
                Key::Named(NamedKey::End) => last,
                _ => return,
            };
            e.prevent_default();
            if let Some((v, _)) = keys.get(next) {
                value.set(v.clone());
                refs[next].scroll_into_view();
            }
        })
        .children(list)
}

/// A list where several can be chosen, bound to `values` (`<select multiple>`). Ctrl adds to what
/// is chosen and Shift goes from the last one, with the pointer and with the arrow keys; Space
/// switches the one at the cursor.
pub fn multi_list_box<T: Clone + PartialEq + 'static>(
    values: Signal<Vec<T>>,
    options: Vec<(T, String)>,
) -> Div {
    let options = Rc::new(options);
    let refs: Rc<Vec<NodeRef>> = Rc::new(options.iter().map(|_| node_ref()).collect());
    let cursor = signal(0usize);
    let anchor = signal(0usize);
    let keyed = signal(false);

    let range = {
        let options = options.clone();
        move |to: usize| {
            let from = anchor.get_untracked();
            let (a, b) = (from.min(to), from.max(to));
            values.set(options[a..=b].iter().map(|(v, _)| v.clone()).collect());
        }
    };
    let flip = {
        let options = options.clone();
        move |i: usize| {
            let v = options[i].0.clone();
            values.update(|all| match all.iter().position(|o| *o == v) {
                Some(at) => {
                    all.remove(at);
                }
                None => all.push(v),
            });
        }
    };
    let only = {
        let options = options.clone();
        move |i: usize| values.set(vec![options[i].0.clone()])
    };

    let rows = options
        .iter()
        .enumerate()
        .map(|(i, (v, name))| {
            let mine = v.clone();
            let (range, flip, only) = (range.clone(), flip.clone(), only.clone());
            option_row(name.clone(), refs[i])
                .sheet(move || {
                    let picked = values.with(|all| all.contains(&mine));
                    shared(if keyed.get() && cursor.get() == i {
                        cursor_row_sheet
                    } else if picked {
                        picked_row_sheet
                    } else {
                        row_sheet
                    })
                })
                .on_click(move |e| {
                    cursor.set(i);
                    if e.modifiers.shift {
                        range(i);
                        return;
                    }
                    anchor.set(i);
                    if e.modifiers.primary() {
                        flip(i);
                    } else {
                        only(i);
                    }
                })
        })
        .collect::<Vec<_>>();

    let last = options.len().saturating_sub(1);
    scroller()
        .on_focus_visible(move |v| keyed.set(v))
        .on_key_down(move |e| {
            let at = cursor.get_untracked();
            let next = match e.key {
                Key::Named(NamedKey::ArrowDown) => (at + 1).min(last),
                Key::Named(NamedKey::ArrowUp) => at.saturating_sub(1),
                Key::Named(NamedKey::Home) => 0,
                Key::Named(NamedKey::End) => last,
                Key::Named(NamedKey::Space) => {
                    e.prevent_default();
                    anchor.set(at);
                    flip(at);
                    return;
                }
                _ => return,
            };
            e.prevent_default();
            keyed.set(true);
            cursor.set(next);
            refs[next].scroll_into_view();
            if e.modifiers.shift {
                range(next);
            } else if !e.modifiers.primary() {
                anchor.set(next);
                only(next);
            }
        })
        .children(rows)
}
