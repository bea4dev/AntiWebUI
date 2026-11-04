//! What organizes the rest: tabs, parts that open, toolbars.

use crate::theme::{QUICK, Theme, ring, shared, theme};
use anti_webui_core::prelude::*;
use std::rc::Rc;

/// A tab: what it is called and what it shows, which is built each time it is chosen.
pub struct Tab {
    label: String,
    content: Rc<dyn Fn() -> View>,
}

/// A tab of [`tabs`].
pub fn tab<V: IntoView>(label: impl Into<String>, content: impl Fn() -> V + 'static) -> Tab {
    Tab {
        label: label.into(),
        content: Rc::new(move || content().into_view()),
    }
}

fn tab_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        crate::theme::smooth(s)
            .text_color(t.text_muted)
            .border_color(Color::TRANSPARENT)
            .cursor(CursorIcon::Pointer)
    })
    .hover(|s| s.text_color(t.text).border_color(t.border_strong))
    .disabled(|s| s.text_color(t.text_disabled).cursor(CursorIcon::Default))
}

fn current_tab_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        crate::theme::smooth(s)
            .text_color(t.text)
            .border_color(t.accent)
            .cursor(CursorIcon::Pointer)
    })
    .disabled(|s| s.text_color(t.text_disabled).cursor(CursorIcon::Default))
    .accent_border()
}

fn ringed_tab_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        ring(crate::theme::smooth(s), t)
            .rounded(4.0)
            .text_color(t.text)
            .border_color(t.accent)
            .cursor(CursorIcon::Pointer)
    })
    .accent_border()
}

/// Tabs, bound to the index of the one that is chosen. They are one stop for Tab, and the arrow
/// keys go through them.
pub fn tabs(selected: Signal<usize>, items: Vec<Tab>) -> Div {
    let count = items.len();
    let keyed = signal(false);
    let contents: Vec<_> = items.iter().map(|t| t.content.clone()).collect();
    let heads = items
        .into_iter()
        .enumerate()
        .map(|(i, item)| {
            div()
                .flex_none()
                .px(12.0)
                .py(6.0)
                .border_b(2.0)
                .select_none()
                .sheet(move || {
                    shared(if selected.get() != i {
                        tab_sheet
                    } else if keyed.get() {
                        ringed_tab_sheet
                    } else {
                        current_tab_sheet
                    })
                })
                .on_click(move |_| selected.set(i))
                .child(text(item.label))
        })
        .collect::<Vec<_>>();
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .flex_row()
                .flex_none()
                .border_b(1.0)
                .focusable()
                .dyn_style(|s| s.border_color(theme().border))
                .on_focus_visible(move |v| keyed.set(v))
                .on_key_down(move |e| {
                    let at = selected.get_untracked();
                    let next = match e.key {
                        Key::Named(NamedKey::ArrowRight) => (at + 1) % count.max(1),
                        Key::Named(NamedKey::ArrowLeft) => (at + count - 1) % count.max(1),
                        Key::Named(NamedKey::Home) => 0,
                        Key::Named(NamedKey::End) => count.saturating_sub(1),
                        _ => return,
                    };
                    e.prevent_default();
                    selected.set(next);
                })
                .children(heads),
        )
        .child(div().flex().flex_col().flex_1().min_h(0.0).child(for_each(
            move || vec![selected.get()],
            |i| *i,
            move |i| match contents.get(i) {
                Some(content) => content(),
                None => fragment(Vec::<View>::new()),
            },
        )))
}

fn head_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.cursor(CursorIcon::Pointer).text_color(t.text))
        .hover(|s| s.bg(t.hover))
        .active(|s| s.bg(t.pressed))
        .focus_visible(|s| ring(s, t).rounded(4.0))
        .disabled(|s| s.text_color(t.text_disabled).cursor(CursorIcon::Default))
}

/// A title that opens and closes what is under it, bound to `open`. What is under it only exists
/// while it shows.
pub fn disclosure<V: IntoView>(
    open: Signal<bool>,
    title: impl IntoTextSource,
    content: impl Fn() -> V + 'static,
) -> Div {
    disclosure_of(
        open,
        title,
        move || content().into_view(),
        move || open.update(|v| *v = !*v),
    )
}

fn disclosure_of(
    open: Signal<bool>,
    title: impl IntoTextSource,
    content: impl Fn() -> View + 'static,
    press: impl Fn() + 'static,
) -> Div {
    // How high what is under it is, once it was there. Before that, an open one takes what it
    // needs and a closed one has no height to start from.
    let measured = signal(None::<f32>);
    let built = signal(untrack(|| open.get()));
    let was_open = untrack(|| open.get());
    effect(move || {
        if open.get() {
            built.set_if_changed(true);
        } else {
            let timer = set_timeout(QUICK.duration, move || {
                if !open.get_untracked() {
                    built.set_if_changed(false);
                }
            });
            on_cleanup(move || timer.cancel());
        }
    });
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(6.0)
                .px(6.0)
                .py(5.0)
                .rounded(4.0)
                .select_none()
                .font_weight(FontWeight::MEDIUM)
                .focusable()
                .sheet(|| shared(head_sheet))
                .on_click(move |_| press())
                .child(
                    text("▸")
                        .text_size(14.0)
                        .transition_of([Prop::Rotate], QUICK)
                        .dyn_style(move |s| s.rotate(if open.get() { 90.0 } else { 0.0 })),
                )
                .child(text(title)),
        )
        .child(
            div()
                .overflow_clip()
                .transition_of([Prop::Height], QUICK)
                .dyn_style(move |s| match (open.get(), measured.get()) {
                    (true, Some(h)) => s.h(h),
                    (true, None) if was_open => s,
                    _ => s.h(0.0),
                })
                .child(show(move || built.get(), {
                    let content = Rc::new(content);
                    move || {
                        let content = content.clone();
                        div()
                            .flex()
                            .flex_col()
                            .flex_none()
                            .pl(24.0)
                            .pr(6.0)
                            .py(6.0)
                            .on_resized(move |_, h| measured.set_if_changed(Some(h)))
                            .child(content())
                    }
                })),
        )
}

/// Parts of which one is open at a time. Each has a title and what is under it.
pub fn accordion(items: Vec<(String, Rc<dyn Fn() -> View>)>) -> Div {
    let current = signal(None::<usize>);
    div().flex().flex_col().gap(2.0).children(
        items
            .into_iter()
            .enumerate()
            .map(|(i, (title, content))| {
                let open = signal(false);
                effect(move || open.set_if_changed(current.get() == Some(i)));
                disclosure_of(
                    open,
                    title,
                    move || content(),
                    move || current.update(|c| *c = if *c == Some(i) { None } else { Some(i) }),
                )
            })
            .collect::<Vec<_>>(),
    )
}

/// A row of controls for an area. [`quiet_button`](crate::quiet_button) and
/// [`toggle_button`](crate::toggle_button) go well in it.
pub fn toolbar() -> Div {
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(2.0)
        .px(4.0)
        .py(4.0)
        .border_b(1.0)
        .dyn_style(|s| s.border_color(theme().border))
}

/// A line between what belongs together in a toolbar.
pub fn toolbar_separator() -> Div {
    div()
        .flex_none()
        .w(1.0)
        .h(18.0)
        .mx(4.0)
        .dyn_style(|s| s.bg(theme().border))
}

/// Pushes what comes after it to the other end of a toolbar.
pub fn toolbar_spacer() -> Div {
    div().flex_1()
}
