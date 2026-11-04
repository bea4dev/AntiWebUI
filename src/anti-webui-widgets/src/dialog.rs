//! Dialogs.

use crate::theme::{Theme, fade, shared};
use anti_webui_core::prelude::*;
use std::rc::Rc;

fn surface_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        s.bg(t.overlay)
            .border_color(t.border)
            .text_color(t.text)
            .shadow_lg()
    })
}

/// What is built by `content` in the middle of the window, while `open` says so, on a scrim that
/// takes the rest of the window. It keeps Tab inside, and gives the focus back when it closes.
/// Pressing the scrim and Escape call `on_dismiss`, which is the place to change `open`.
pub fn dialog<V: IntoView>(
    open: impl Fn() -> bool + 'static,
    on_dismiss: impl Fn() + 'static,
    content: impl Fn() -> V + 'static,
) -> Show {
    let open = Rc::new(open);
    let dismiss: Rc<dyn Fn()> = Rc::new(on_dismiss);
    let fading = open.clone();
    let shown = fade(move || fading());
    let content = Rc::new(content);
    show(
        move || open() || shown.get() > 0.0,
        move || {
            let root = node_ref();
            let (by_scrim, by_key) = (dismiss.clone(), dismiss.clone());
            let content = content.clone();
            let view = portal()
                .modal()
                .child(
                    div()
                        .absolute()
                        .inset(0.0)
                        .dyn_style(move |s| {
                            let scrim = crate::theme().scrim;
                            s.bg(scrim.with_alpha(scrim.a * shown.get().clamp(0.0, 1.0)))
                        })
                        .on_click(move |e| {
                            e.stop_propagation();
                            by_scrim();
                        }),
                )
                .child(
                    div()
                        .absolute()
                        .inset(0.0)
                        .flex()
                        .items_center()
                        .justify_center()
                        .p(24.0)
                        .focusable()
                        .node_ref(root)
                        .pointer_events_none()
                        .on_key_down(move |e| {
                            e.stop_propagation();
                            if matches!(e.key, Key::Named(NamedKey::Escape)) {
                                e.prevent_default();
                                by_key();
                            }
                        })
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .min_w(280.0)
                                .max_w(560.0)
                                .max_h(pct(100.0))
                                .rounded(12.0)
                                .border(1.0)
                                .pointer_events_auto()
                                .sheet(|| shared(surface_sheet))
                                // What it is declared in has nothing to do with what is pressed
                                // here.
                                .on_pointer_down(|e| e.stop_propagation())
                                .on_click(|e| e.stop_propagation())
                                .dyn_style(move |s| {
                                    let v = shown.get().clamp(0.0, 1.0);
                                    s.opacity(v).scale(0.97 + 0.03 * v)
                                })
                                .child(content()),
                        ),
                )
                .into_view();
            root.focus();
            view
        },
    )
}

/// A dialog with a title, a text and buttons in a row at the end.
pub fn alert_dialog(
    open: impl Fn() -> bool + 'static,
    on_dismiss: impl Fn() + 'static,
    title: impl Into<String>,
    body: impl Into<String>,
    actions: impl Fn() -> Vec<Div> + 'static,
) -> Show {
    let (title, body) = (title.into(), body.into());
    dialog(open, on_dismiss, move || {
        div()
            .flex()
            .flex_col()
            .gap(12.0)
            .p(20.0)
            .child(crate::heading(title.clone()).text_size(16.0))
            .child(text(body.clone()))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(8.0)
                    .mt(4.0)
                    .children(actions()),
            )
    })
}
