//! Tooltips.

use crate::theme::{QUICK, Theme, shared};
use anti_webui_core::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

const SHOW_AFTER: Duration = Duration::from_millis(500);

fn tip_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(t.tooltip).text_color(t.on_tooltip))
}

/// Shows a short text above `target` while the pointer rests on it or the focus from the keyboard
/// is on it. `target` is what comes back, and can be styled further.
pub fn tooltip(target: Div, label: impl Into<String>) -> Div {
    let label = label.into();
    let up = signal(false);
    let timer: Rc<Cell<Option<Timeout>>> = Rc::default();
    let hover_timer = timer.clone();
    let press_timer = timer;
    target
        .on_hover(move |inside| {
            if let Some(running) = hover_timer.take() {
                running.cancel();
            }
            if inside {
                hover_timer.set(Some(set_timeout(SHOW_AFTER, move || up.set(true))));
            } else {
                up.set(false);
            }
        })
        .on_focus_visible(move |focused| up.set(focused))
        .on_pointer_down(move |_| {
            if let Some(running) = press_timer.take() {
                running.cancel();
            }
            up.set(false);
        })
        .child(
            show(
                move || up.get(),
                move || {
                    popup()
                        .kind(PopupKind::Tooltip)
                        .placement(Placement::AboveCenter)
                        .gap(6)
                        .child(
                            div()
                                .max_w(280.0)
                                .px(8.0)
                                .py(4.0)
                                .rounded(4.0)
                                .text_size(12.0)
                                .sheet(|| shared(tip_sheet))
                                .child(text(label.clone())),
                        )
                },
            )
            .enter(|s| s.opacity(0.0))
            .transition(QUICK),
        )
}
