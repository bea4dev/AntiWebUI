//! What arranges the rest: panes that share room, and ways through pages.

use crate::theme::{Theme, focus_ring, theme, themed};
use anti_webui_core::prelude::*;
use std::rc::Rc;

fn handle_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(t.border))
        .hover(|s| s.bg(t.accent))
        .active(|s| s.bg(t.accent))
        .focus_visible(|s| focus_ring(s, t))
        .accent_background()
}

const SMALLEST: f32 = 0.05;

fn split(horizontal: bool, ratio: Signal<f32>, first: View, second: View) -> Div {
    let area = node_ref();
    let clamp = |v: f32| v.clamp(SMALLEST, 1.0 - SMALLEST);
    let handle = div()
        .relative()
        .flex_none()
        .focusable()
        .select_none()
        .sheet(|| themed(handle_sheet))
        .cursor(if horizontal {
            CursorIcon::EwResize
        } else {
            CursorIcon::NsResize
        })
        // Room to hit that doesn't take room: it is the line that has a size.
        .child(div().absolute().dyn_style(move |s| {
            if horizontal {
                s.top(0.0).bottom(0.0).left(-3.0).right(-3.0)
            } else {
                s.left(0.0).right(0.0).top(-3.0).bottom(-3.0)
            }
        }))
        .grab_threshold(1.0)
        .on_grab(move |g| {
            let Some(rect) = area.rect() else {
                return;
            };
            let v = if horizontal {
                (g.position.x - rect.x as f32) / rect.width.max(1) as f32
            } else {
                (g.position.y - rect.y as f32) / rect.height.max(1) as f32
            };
            if matches!(g.phase, GrabPhase::Start | GrabPhase::Move) {
                ratio.set(clamp(v));
            }
        })
        .on_key_down(move |e| {
            let by = match (&e.key, horizontal) {
                (Key::Named(NamedKey::ArrowLeft), true)
                | (Key::Named(NamedKey::ArrowUp), false) => -0.02,
                (Key::Named(NamedKey::ArrowRight), true)
                | (Key::Named(NamedKey::ArrowDown), false) => 0.02,
                _ => return,
            };
            e.prevent_default();
            ratio.update(|v| *v = clamp(*v + by));
        });
    let handle = if horizontal {
        handle.w(1.0)
    } else {
        handle.h(1.0)
    };
    let pane = |view: View| {
        div()
            .flex()
            .flex_col()
            .min_w(0.0)
            .min_h(0.0)
            .overflow_clip()
            .child(view)
    };
    let root = div().flex().min_w(0.0).min_h(0.0).node_ref(area);
    let root = if horizontal {
        root.flex_row()
    } else {
        root.flex_col()
    };
    root.child(pane(first).dyn_style(move |s| {
        let share = ratio.get() * 100.0;
        s.flex_grow(share).flex_basis(0.0)
    }))
    .child(handle)
    .child(pane(second).dyn_style(move |s| {
        let share = (1.0 - ratio.get()) * 100.0;
        s.flex_grow(share).flex_basis(0.0)
    }))
}

/// Two panes side by side with a line to drag between them, bound to the part of the room the first
/// one takes.
pub fn split_row(ratio: Signal<f32>, first: impl IntoView, second: impl IntoView) -> Div {
    split(true, ratio, first.into_view(), second.into_view())
}

/// Two panes one above the other with a line to drag between them.
pub fn split_col(ratio: Signal<f32>, first: impl IntoView, second: impl IntoView) -> Div {
    split(false, ratio, first.into_view(), second.into_view())
}

/// A step of a [`breadcrumb`].
pub struct Crumb {
    label: String,
    on_click: Option<Rc<dyn Fn()>>,
}

/// A step of a [`breadcrumb`]. Without [`Crumb::on_click`], it is where one is.
pub fn crumb(label: impl Into<String>) -> Crumb {
    Crumb {
        label: label.into(),
        on_click: None,
    }
}

impl Crumb {
    pub fn on_click(mut self, f: impl Fn() + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

/// The way to where one is.
pub fn breadcrumb(items: Vec<Crumb>) -> Div {
    let count = items.len();
    div().flex().flex_row().items_center().gap(6.0).children(
        items
            .into_iter()
            .enumerate()
            .map(|(i, item)| {
                let view = match item.on_click {
                    Some(f) => crate::link(item.label).on_click(move |_| f()),
                    None => div().child(text(item.label)),
                };
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(6.0)
                    .child(view)
                    .children(
                        (i + 1 < count)
                            .then(|| text("/").dyn_style(|s| s.text_color(theme().text_muted))),
                    )
            })
            .collect::<Vec<_>>(),
    )
}

/// The pages to show a button for: the first, the last, and those around the current one, with
/// `None` where others are left out.
pub(crate) fn pages_shown(current: usize, count: usize) -> Vec<Option<usize>> {
    if count <= 7 {
        return (0..count).map(Some).collect();
    }
    let mut out = vec![Some(0)];
    let from = current.saturating_sub(1).max(1);
    let to = (current + 1).min(count - 2);
    if from > 1 {
        out.push(None);
    }
    out.extend((from..=to).map(Some));
    if to < count - 2 {
        out.push(None);
    }
    out.push(Some(count - 1));
    out
}

fn page_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        crate::theme::smooth(s)
            .border_color(Color::TRANSPARENT)
            .bg(Color::TRANSPARENT)
            .text_color(t.text)
            .cursor(CursorIcon::Pointer)
    })
    .hover(|s| s.bg(t.hover))
    .active(|s| s.bg(t.pressed))
    .focus_visible(|s| focus_ring(s, t))
    .disabled(|s| s.text_color(t.text_disabled).cursor(CursorIcon::Default))
}

fn current_page_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        crate::theme::smooth(s)
            .border_color(t.accent)
            .bg(t.accent)
            .text_color(t.on_accent)
            .cursor(CursorIcon::Pointer)
    })
    .focus_visible(|s| focus_ring(s, t))
    .accent_background()
    .accent_border()
}

fn page_button(label: String) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .min_w(28.0)
        .h(28.0)
        .px(6.0)
        .rounded(6.0)
        .border(1.0)
        .select_none()
        .focusable()
        .child(text(label))
}

/// Buttons for the pages, bound to the index of the current one, of `count()` pages.
pub fn pagination(page: Signal<usize>, count: impl Fn() -> usize + 'static) -> Div {
    let count = Rc::new(count);
    let (shown_count, next_count) = (count.clone(), count);
    // The buttons keep their place, so that one that was pressed stays under the pointer: only
    // what is left out moves.
    let shown = memo(move || pages_shown(page.get(), shown_count()));
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(2.0)
        .child(
            page_button("‹".to_owned())
                .sheet(|| themed(page_sheet))
                .disabled(move || page.get() == 0)
                .on_click(move |_| page.update(|p| *p = p.saturating_sub(1))),
        )
        .child(for_each(
            move || {
                shown
                    .get()
                    .into_iter()
                    .enumerate()
                    .map(|(at, p)| (p.ok_or(at), p))
                    .collect::<Vec<_>>()
            },
            |(key, _)| *key,
            move |(_, p)| -> View {
                match p {
                    Some(i) => page_button((i + 1).to_string())
                        .sheet(move || {
                            themed(if page.get() == i {
                                current_page_sheet
                            } else {
                                page_sheet
                            })
                        })
                        .on_click(move |_| page.set(i))
                        .into_view(),
                    None => div()
                        .min_w(20.0)
                        .flex()
                        .justify_center()
                        .select_none()
                        .dyn_style(|s| s.text_color(theme().text_muted))
                        .child(text("…"))
                        .into_view(),
                }
            },
        ))
        .child(
            page_button("›".to_owned())
                .sheet(|| themed(page_sheet))
                .disabled({
                    let count = next_count.clone();
                    move || page.get() + 1 >= count()
                })
                .on_click(move |_| {
                    let last = next_count().saturating_sub(1);
                    page.update(|p| *p = (*p + 1).min(last));
                }),
        )
}
