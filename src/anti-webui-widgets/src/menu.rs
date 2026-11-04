//! Menus.

use crate::theme::{QUICK, Theme, shared, theme};
use anti_webui_core::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

const SUBMENU_AFTER: Duration = Duration::from_millis(150);

/// A row of a menu.
#[derive(Clone)]
pub struct MenuItem {
    label: String,
    shortcut: Option<String>,
    checked: Option<Rc<dyn Fn() -> bool>>,
    disabled: Option<Rc<dyn Fn() -> bool>>,
    on_click: Option<Rc<dyn Fn()>>,
}

/// What a menu has.
#[derive(Clone)]
pub enum MenuEntry {
    Item(MenuItem),
    Separator,
    Submenu(String, Rc<Vec<MenuEntry>>),
}

/// A row of a menu.
pub fn menu_item(label: impl Into<String>) -> MenuItem {
    MenuItem {
        label: label.into(),
        shortcut: None,
        checked: None,
        disabled: None,
        on_click: None,
    }
}

/// A line between what belongs together.
pub fn menu_separator() -> MenuEntry {
    MenuEntry::Separator
}

/// A row that opens another menu.
pub fn submenu(label: impl Into<String>, entries: Vec<MenuEntry>) -> MenuEntry {
    MenuEntry::Submenu(label.into(), Rc::new(entries))
}

impl MenuItem {
    /// What it says at the end of the row. It doesn't make the keys work.
    pub fn shortcut(mut self, keys: impl Into<String>) -> Self {
        self.shortcut = Some(keys.into());
        self
    }

    /// With a check in front of it while `f` returns true.
    pub fn checked_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.checked = Some(Rc::new(f));
        self
    }

    /// Doesn't react and looks that way while `f` returns true.
    pub fn disabled_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(f));
        self
    }

    /// What it does.
    pub fn on_click(mut self, f: impl Fn() + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl From<MenuItem> for MenuEntry {
    fn from(item: MenuItem) -> Self {
        MenuEntry::Item(item)
    }
}

impl MenuEntry {
    fn can_be_lit(&self) -> bool {
        match self {
            MenuEntry::Item(item) => !item.disabled.as_ref().is_some_and(|f| f()),
            MenuEntry::Submenu(..) => true,
            MenuEntry::Separator => false,
        }
    }
}

pub(crate) fn surface_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        s.bg(t.overlay)
            .border_color(t.border)
            .text_color(t.text)
            .caret_color(t.text)
    })
}

fn row_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.cursor(CursorIcon::Pointer).text_color(t.text))
        .disabled(|s| s.text_color(t.text_disabled).cursor(CursorIcon::Default))
}

fn lit_row_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        s.bg(t.accent)
            .text_color(t.on_accent)
            .cursor(CursorIcon::Pointer)
    })
    .disabled(|s| {
        s.bg(Color::TRANSPARENT)
            .text_color(t.text_disabled)
            .cursor(CursorIcon::Default)
    })
    .accent_background()
}

/// A row without what it does.
fn row_frame(i: usize, lit: Signal<Option<usize>>, r: NodeRef) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(6.0)
        .min_h(26.0)
        .px(8.0)
        .rounded(4.0)
        .node_ref(r)
        .sheet(move || {
            shared(if lit.get() == Some(i) {
                lit_row_sheet
            } else {
                row_sheet
            })
        })
}

fn check_slot(checked: Option<Rc<dyn Fn() -> bool>>) -> Div {
    div().flex_none().w(14.0).child(text(move || {
        if checked.as_ref().is_some_and(|f| f()) {
            "✓"
        } else {
            ""
        }
        .to_owned()
    }))
}

/// A menu in `surface`, which is told where it goes. It takes the focus and gives it back.
fn menu_view(
    entries: Rc<Vec<MenuEntry>>,
    surface: Popup,
    is_sub: bool,
    close: Rc<dyn Fn()>,
    close_all: Rc<dyn Fn()>,
) -> View {
    let lit = signal(None::<usize>);
    let sub_open = signal(None::<usize>);
    let sub_focus = signal(false);
    let timer: Rc<Cell<Option<Timeout>>> = Rc::default();
    let root = node_ref();
    let refs: Rc<Vec<NodeRef>> = Rc::new(entries.iter().map(|_| node_ref()).collect());

    let activate: Rc<dyn Fn(usize, bool)> = {
        let (entries, close_all) = (entries.clone(), close_all.clone());
        Rc::new(move |i, from_keys| match entries.get(i) {
            Some(MenuEntry::Item(item)) if entries[i].can_be_lit() => {
                close_all();
                if let Some(f) = &item.on_click {
                    f();
                }
            }
            Some(MenuEntry::Submenu(..)) => {
                sub_focus.set(from_keys);
                sub_open.set(Some(i));
            }
            _ => {}
        })
    };

    let rows = entries
        .iter()
        .enumerate()
        .map(|(i, entry)| -> View {
            let timer = timer.clone();
            let hover = move |inside: bool| {
                if let Some(running) = timer.take() {
                    running.cancel();
                }
                if !inside {
                    return;
                }
                lit.set(Some(i));
                let goes_to = i;
                let is_sub = sub_open.get_untracked() == Some(goes_to);
                if !is_sub {
                    let timer_value = set_timeout(SUBMENU_AFTER, move || {
                        // A row that opens nothing closes what another one opened.
                        sub_focus.set(false);
                        sub_open.set(Some(goes_to));
                    });
                    timer.set(Some(timer_value));
                }
            };
            match entry {
                MenuEntry::Separator => div()
                    .h(1.0)
                    .my(4.0)
                    .mx(4.0)
                    .flex_none()
                    .dyn_style(|s| s.bg(theme().border))
                    .into_view(),
                MenuEntry::Item(item) => {
                    let activate = activate.clone();
                    let mut row = row_frame(i, lit, refs[i])
                        .on_hover(hover)
                        .on_click(move |e| {
                            e.stop_propagation();
                            activate(i, false);
                        })
                        .child(check_slot(item.checked.clone()))
                        .child(div().flex_1().pr(16.0).child(text(item.label.clone())))
                        .children(item.shortcut.clone().map(|keys| {
                            text(keys).text_size(12.0).dyn_style(move |s| {
                                if lit.get() == Some(i) {
                                    s
                                } else {
                                    s.text_color(theme().text_muted)
                                }
                            })
                        }));
                    if let Some(off) = item.disabled.clone() {
                        row = row.disabled(move || off());
                    }
                    row.into_view()
                }
                MenuEntry::Submenu(label, inner) => {
                    let activate = activate.clone();
                    let inner = inner.clone();
                    let close_all = close_all.clone();
                    let anchor = refs[i];
                    row_frame(i, lit, refs[i])
                        .on_hover(hover)
                        .on_click(move |e| {
                            e.stop_propagation();
                            activate(i, false);
                        })
                        .child(check_slot(None))
                        .child(div().flex_1().pr(16.0).child(text(label.clone())))
                        .child(text("▸").text_size(10.0))
                        .child(show(
                            move || sub_open.get() == Some(i),
                            move || {
                                let surface = popup()
                                    .placement(Placement::RightStart)
                                    .anchor(anchor)
                                    .gap(2)
                                    .modal()
                                    .on_dismiss(move || sub_open.set(None));
                                menu_view(
                                    inner.clone(),
                                    surface,
                                    true,
                                    Rc::new(move || sub_open.set(None)),
                                    close_all.clone(),
                                )
                            },
                        ))
                        .into_view()
                }
            }
        })
        .collect::<Vec<_>>();

    let keys = entries.clone();
    let key_refs = refs.clone();
    let press = activate;
    let closer = close;
    let tab_closer = close_all;
    let move_lit = move |from: Option<usize>, down: bool| -> Option<usize> {
        let count = keys.len();
        let mut at = from;
        for _ in 0..count {
            let next = match at {
                Some(i) if down => (i + 1) % count,
                Some(i) => (i + count - 1) % count,
                None if down => 0,
                None => count - 1,
            };
            if keys[next].can_be_lit() {
                return Some(next);
            }
            at = Some(next);
        }
        None
    };
    let edge_keys = entries.clone();
    let view = surface
        .child(
            div()
                .flex()
                .flex_col()
                .min_w(160.0)
                .max_w(360.0)
                .max_h(480.0)
                .p(4.0)
                .rounded(8.0)
                .border(1.0)
                .shadow_lg()
                .overflow_y_scroll()
                .select_none()
                .focusable()
                .node_ref(root)
                .sheet(|| shared(surface_sheet))
                // What it is declared in has nothing to do with what is pressed here.
                .on_pointer_down(|e| e.stop_propagation())
                .on_click(|e| e.stop_propagation())
                .on_key_down(move |e| {
                    // The menu it is a submenu of, or the button it hangs from, has no part in it.
                    e.stop_propagation();
                    let next = match e.key {
                        Key::Named(NamedKey::ArrowDown) => move_lit(lit.get_untracked(), true),
                        Key::Named(NamedKey::ArrowUp) => move_lit(lit.get_untracked(), false),
                        Key::Named(NamedKey::Home) => move_lit(None, true),
                        Key::Named(NamedKey::End) => {
                            edge_keys.iter().rposition(MenuEntry::can_be_lit)
                        }
                        Key::Named(NamedKey::Enter | NamedKey::Space) => {
                            e.prevent_default();
                            if let Some(i) = lit.get_untracked() {
                                press(i, true);
                            }
                            return;
                        }
                        Key::Named(NamedKey::ArrowRight) => {
                            e.prevent_default();
                            if let Some(i) = lit.get_untracked()
                                && matches!(edge_keys.get(i), Some(MenuEntry::Submenu(..)))
                            {
                                press(i, true);
                            }
                            return;
                        }
                        Key::Named(NamedKey::ArrowLeft) if is_sub => {
                            e.prevent_default();
                            closer();
                            return;
                        }
                        Key::Named(NamedKey::Tab) => {
                            e.prevent_default();
                            tab_closer();
                            return;
                        }
                        _ => return,
                    };
                    e.prevent_default();
                    sub_open.set(None);
                    lit.set(next);
                    if let Some(row) = next.and_then(|i| key_refs.get(i)) {
                        row.scroll_into_view();
                    }
                })
                .children(rows),
        )
        .into_view();
    root.focus();
    view
}

/// A menu that hangs from what it is a child of, while `open` says so. Pressing outside of it and
/// Escape call `on_dismiss`, as does choosing a row, which is the place to change `open`.
pub fn menu(
    open: impl Fn() -> bool + 'static,
    on_dismiss: impl Fn() + 'static,
    entries: Vec<MenuEntry>,
) -> Show {
    let entries = Rc::new(entries);
    let dismiss: Rc<dyn Fn()> = Rc::new(on_dismiss);
    show(open, move || {
        let on_dismiss = dismiss.clone();
        let surface = popup().gap(4).modal().on_dismiss(move || on_dismiss());
        menu_view(
            entries.clone(),
            surface,
            false,
            dismiss.clone(),
            dismiss.clone(),
        )
    })
    .enter(|s| s.opacity(0.0))
    .transition(QUICK)
}

/// A button that opens a menu.
pub fn dropdown_button(label: impl IntoTextSource, entries: Vec<MenuEntry>) -> Div {
    let open = signal(false);
    crate::button(label)
        .on_click(move |_| open.update(|v| *v = !*v))
        .on_key_down(move |e| {
            if matches!(e.key, Key::Named(NamedKey::ArrowDown)) && !open.get_untracked() {
                e.prevent_default();
                open.set(true);
            }
        })
        .child(text("▾").dyn_style(|s| s.text_color(theme().text_muted)))
        .child(menu(move || open.get(), move || open.set(false), entries))
}

/// Opens a menu where `target` is pressed with the secondary button. `target` is what comes back,
/// and can be styled further.
pub fn context_menu(target: Div, entries: Vec<MenuEntry>) -> Div {
    let at = signal(None::<RectI>);
    let entries = Rc::new(entries);
    target
        .on_pointer_down(move |e| {
            if e.button == Some(PointerButton::Secondary) {
                let (x, y) = (e.position.x as i32, e.position.y as i32);
                at.set(Some(RectI::new(x, y, 1, 1)));
            }
        })
        .child(
            show(
                move || at.with(Option::is_some),
                move || {
                    let close: Rc<dyn Fn()> = Rc::new(move || at.set(None));
                    let on_dismiss = close.clone();
                    let mut surface = popup().modal().on_dismiss(move || on_dismiss());
                    if let Some(rect) = at.get_untracked() {
                        surface = surface.anchor_rect(rect);
                    }
                    menu_view(
                        entries.clone(),
                        surface,
                        false,
                        close.clone(),
                        close.clone(),
                    )
                },
            )
            .enter(|s| s.opacity(0.0))
            .transition(QUICK),
        )
}
