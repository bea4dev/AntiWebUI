//! Snackbars. Sizes, colors and durations are those of Jetpack Compose. See
//! `docs/design/20-md3e-components.md`.

use super::icon::sized_icon;
use super::surface::{Ink, elevation_shadows};
use crate::color::color_scheme;
use crate::icons;
use crate::typography::TextStyle;
use anti_webui::prelude::*;
use std::rc::Rc;
use std::time::Duration;

const SHORT: Duration = Duration::from_millis(4000);
const LONG: Duration = Duration::from_millis(10000);

/// A short message, with something to do about it if there is.
#[derive(Clone)]
pub struct Snackbar {
    text: String,
    action: Option<(String, Rc<dyn Fn()>)>,
    closable: bool,
    lasts: Option<Duration>,
}

impl Snackbar {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            action: None,
            closable: false,
            lasts: Some(SHORT),
        }
    }

    /// Something to do. It then stays until it is closed, unless told otherwise.
    pub fn action(mut self, label: impl Into<String>, f: impl Fn() + 'static) -> Self {
        self.action = Some((label.into(), Rc::new(f)));
        self.lasts = None;
        self
    }

    /// With a button that closes it.
    pub fn closable(mut self) -> Self {
        self.closable = true;
        self
    }

    /// Stays for 10 seconds rather than 4.
    pub fn long(mut self) -> Self {
        self.lasts = Some(LONG);
        self
    }

    /// Stays for this long.
    pub fn lasting(mut self, duration: Duration) -> Self {
        self.lasts = Some(duration);
        self
    }
}

/// Where snackbars show, one at a time, in the order they came. It is copied around to show them
/// from anywhere; its [`view`](Snackbars::view) goes once somewhere in the tree. The queue is the
/// standard one (`MessageQueue`): the pointer on a snackbar makes it wait.
#[derive(Clone, Copy)]
pub struct Snackbars {
    queue: MessageQueue<Snackbar>,
}

/// Makes a place for snackbars.
pub fn snackbars() -> Snackbars {
    Snackbars {
        queue: message_queue(),
    }
}

impl Snackbars {
    pub fn show(&self, snackbar: Snackbar) {
        self.queue.push(snackbar);
    }

    /// Closes the one that is showing, and the next comes.
    pub fn dismiss(&self) {
        self.queue.close_first();
    }

    fn remove(&self, id: u64) {
        self.queue.close(id);
    }

    /// The place itself: above everything, at the bottom of the window.
    pub fn view(&self) -> View {
        crate::ensure_ready();
        let this = *self;
        let showing = move || {
            this.queue
                .list()
                .into_iter()
                .take(1)
                .collect::<Vec<(u64, Snackbar)>>()
        };
        portal()
            .child(
                div()
                    .absolute()
                    .left(0.0)
                    .right(0.0)
                    .bottom(24.0)
                    .h(0.0)
                    .pointer_events_none()
                    .child(
                        for_each(
                            showing,
                            |(id, _)| *id,
                            move |(id, item)| this.item(id, item),
                        )
                        .enter(|s| s.opacity(0.0).scale(0.8))
                        .exit(|s| s.opacity(0.0).scale(0.8))
                        .transition(
                            Transition::ms(220).easing(Easing::CubicBezier(0.3, 1.3, 0.6, 1.0)),
                        ),
                    ),
            )
            .into_view()
    }

    fn item(&self, id: u64, item: Snackbar) -> Div {
        let this = *self;
        let Snackbar {
            text: message,
            action,
            closable,
            lasts,
        } = item;
        let action = action.map(|(label, f)| {
            let ink = Ink::new(|c| c.inverse_primary, || false);
            ink.paint(
                ink.listen(
                    TextStyle::LABEL_LARGE.apply(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .h(40.0)
                            .px(12.0)
                            .rounded(20.0)
                            .focusable()
                            .cursor(CursorIcon::Pointer)
                            .dyn_style(|s| s.text_color(color_scheme().inverse_primary))
                            .on_click(move |_| {
                                f();
                                this.remove(id);
                            })
                            .child(text(label)),
                    ),
                ),
            )
        });
        let close = closable.then(|| {
            let ink = Ink::new(|c| c.inverse_on_surface, || false);
            ink.paint(
                ink.listen(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .w(40.0)
                        .h(40.0)
                        .ml(4.0)
                        .rounded(20.0)
                        .focusable()
                        .cursor(CursorIcon::Pointer)
                        .dyn_style(|s| s.text_color(color_scheme().inverse_on_surface))
                        .on_click(move |_| this.remove(id))
                        .child(sized_icon(icons::CLOSE, 24.0)),
                ),
            )
        });
        div()
            .absolute()
            .left(0.0)
            .right(0.0)
            .bottom(0.0)
            .flex()
            .justify_center()
            .child(
                TextStyle::BODY_MEDIUM.apply(
                    this.queue
                        .timed(id, lasts, div())
                        .access(|a| a.role(Role::Status))
                        .flex()
                        .flex_row()
                        .items_center()
                        .min_h(48.0)
                        .max_w(600.0)
                        .pl(16.0)
                        .pr(8.0)
                        .rounded(4.0)
                        .pointer_events_auto()
                        .shadows(elevation_shadows(3.0))
                        .dyn_style(|s| {
                            let c = color_scheme();
                            s.bg(c.inverse_surface).text_color(c.inverse_on_surface)
                        })
                        .child(div().py(6.0).pr(8.0).flex_shrink(1.0).child(text(message)))
                        .children(action)
                        .children(close),
                ),
            )
    }
}
