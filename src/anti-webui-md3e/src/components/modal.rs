//! What goes above everything and takes the attention: dialogs, bottom and side sheets and the
//! navigation drawer. Sizes, colors and timing are those of Jetpack Compose. See
//! `docs/design/20-md3e-components.md`.

use super::icon::sized_icon;
use super::surface::When;
use crate::color::color_scheme;
use crate::icons::Symbol;
use crate::motion::{default_effects, default_spatial, fast_effects};
use crate::typography::TextStyle;
use anti_webui::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

const SCRIM: f32 = 0.32;

/// What keeps a modal up: `open`, and then how far it has come in and how dark the rest is.
#[derive(Clone)]
struct Modal {
    open: When,
    dismiss: Rc<dyn Fn()>,
    shown: Animated<f32>,
}

impl Modal {
    /// `appear` and `leave` are the springs of `shown`.
    fn new(
        open: impl Fn() -> bool + 'static,
        dismiss: impl Fn() + 'static,
        appear: Spring,
        leave: Spring,
    ) -> Self {
        crate::ensure_ready();
        let open: When = Rc::new(open);
        let shown = animated(if untrack(|| open()) { 1.0f32 } else { 0.0 });
        let opening = open.clone();
        let last = Cell::new(untrack(|| opening()));
        effect(move || {
            let now = opening();
            if last.replace(now) != now {
                untrack(|| {
                    if now {
                        shown.spring_to(1.0, appear);
                    } else {
                        shown.spring_to(0.0, leave);
                    }
                });
            }
        });
        Self {
            open,
            dismiss: Rc::new(dismiss),
            shown,
        }
    }

    /// The scrim and what is on it, for as long as something is to be seen. The behavior is the
    /// standard one (`modal_with`): Tab stays inside, and Escape and the scrim dismiss.
    fn view(self, content: impl Fn() -> Div + 'static) -> Show {
        let Self {
            open,
            dismiss,
            shown,
        } = self;
        modal_with(
            move || open(),
            move || dismiss(),
            move |m| {
                let scrim = follow(
                    move || if m.open() { 1.0f32 } else { 0.0 },
                    default_effects(),
                );
                m.hold_while(move || shown.get() > 0.0 || scrim.get() > 0.0);
                move || {
                    div()
                        .size_full()
                        .child(m.scrim(div().absolute().inset(0.0).dyn_style(move |s| {
                            s.bg(color_scheme()
                                .scrim
                                .with_alpha(SCRIM * scrim.get().clamp(0.0, 1.0)))
                        })))
                        .child(div().absolute().inset(0.0).child(content()))
                }
            },
        )
    }
}

/// What is said in a dialog and what can be done about it. Buttons are made each time it opens.
#[derive(Clone, Default)]
pub struct AlertDialog {
    icon: Option<&'static Symbol>,
    title: Option<String>,
    text: Option<String>,
    confirm: Option<Rc<dyn Fn() -> Div>>,
    dismiss: Option<Rc<dyn Fn() -> Div>>,
}

impl AlertDialog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn icon(mut self, symbol: &'static Symbol) -> Self {
        self.icon = Some(symbol);
        self
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// The button that says yes.
    pub fn confirm(mut self, f: impl Fn() -> Div + 'static) -> Self {
        self.confirm = Some(Rc::new(f));
        self
    }

    /// The button that says no.
    pub fn dismiss(mut self, f: impl Fn() -> Div + 'static) -> Self {
        self.dismiss = Some(Rc::new(f));
        self
    }
}

/// A dialog with anything in it. It closes when `on_dismiss` makes `open` false; that runs for a
/// click outside and for Escape.
pub fn dialog(
    open: impl Fn() -> bool + 'static,
    on_dismiss: impl Fn() + 'static,
    content: impl Fn() -> Div + 'static,
) -> Show {
    let modal = Modal::new(open, on_dismiss, default_spatial(), fast_effects());
    let shown = modal.shown;
    let fade_open = modal.open.clone();
    let fade = follow(
        move || if fade_open() { 1.0f32 } else { 0.0 },
        fast_effects(),
    );
    modal.view(move || {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p(24.0)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w(280.0)
                    .max_w(560.0)
                    .rounded(28.0)
                    .pointer_events_auto()
                    .dyn_style(move |s| {
                        let c = color_scheme();
                        s.bg(c.surface_container_high)
                            .text_color(c.on_surface)
                            .opacity(fade.get().clamp(0.0, 1.0))
                            .scale(0.8 + 0.2 * shown.get())
                    })
                    .child(content()),
            )
    })
}

/// A dialog that says something and asks what to do.
pub fn alert_dialog(
    open: impl Fn() -> bool + 'static,
    on_dismiss: impl Fn() + 'static,
    content: AlertDialog,
) -> Show {
    dialog(open, on_dismiss, move || {
        let AlertDialog {
            icon,
            title,
            text: body,
            confirm,
            dismiss,
        } = content.clone();
        let centered = icon.is_some();
        div()
            .flex()
            .flex_col()
            .p(24.0)
            .children(icon.map(|symbol| {
                sized_icon(symbol, 24.0)
                    .self_center()
                    .mb(16.0)
                    .dyn_style(|s| s.text_color(color_scheme().secondary))
            }))
            .children(title.map(|title| {
                let line = TextStyle::HEADLINE_SMALL.apply(div().mb(16.0).child(text(title)));
                let line = line.dyn_style(|s| s.text_color(color_scheme().on_surface));
                if centered { line.self_center() } else { line }
            }))
            .children(body.map(|body| {
                TextStyle::BODY_MEDIUM
                    .apply(div().mb(24.0).child(text(body)))
                    .dyn_style(|s| s.text_color(color_scheme().on_surface_variant))
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(8.0)
                    .children(dismiss.map(|f| f()))
                    .children(confirm.map(|f| f())),
            )
    })
}

/// A sheet that comes from the bottom of the window. It goes down when dragged by its handle.
pub fn bottom_sheet(
    open: impl Fn() -> bool + 'static,
    on_dismiss: impl Fn() + 'static,
    content: impl Fn() -> Div + 'static,
) -> Show {
    // What it bounces past the top with is covered by this much more of the sheet below.
    const BELOW: f32 = 48.0;
    let modal = Modal::new(open, on_dismiss, default_spatial(), fast_effects());
    let shown = modal.shown;
    let dismiss = modal.dismiss.clone();
    modal.view(move || {
        let height = signal(800.0f32);
        let dismiss = dismiss.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_end()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .max_w(640.0)
                    .mb(-BELOW)
                    .pb(BELOW)
                    .max_h(pct(90.0))
                    .rounded_t(28.0)
                    .pointer_events_auto()
                    .on_resized(move |_, h| height.set(h - BELOW))
                    .dyn_style(move |s| {
                        let c = color_scheme();
                        s.bg(c.surface_container_low)
                            .text_color(c.on_surface)
                            .translate_y((1.0 - shown.get()) * height.get())
                    })
                    .child(
                        div()
                            .flex()
                            .justify_center()
                            .py(22.0)
                            .cursor(CursorIcon::Grab)
                            .on_grab(move |g| match g.phase {
                                GrabPhase::Start => {}
                                GrabPhase::Move => {
                                    let h = height.get_untracked().max(1.0);
                                    shown.set(1.0 - g.total.y.max(0.0) / h);
                                }
                                GrabPhase::End => {
                                    let h = height.get_untracked().max(1.0);
                                    let away = (1.0 - shown.get_untracked()) * h;
                                    if away > 56.0 || g.velocity.y > 125.0 {
                                        dismiss();
                                    } else {
                                        shown.spring_to(1.0, default_spatial());
                                    }
                                }
                                GrabPhase::Cancel => shown.spring_to(1.0, default_spatial()),
                            })
                            .child(
                                div()
                                    .w(32.0)
                                    .h(4.0)
                                    .rounded(2.0)
                                    .dyn_style(|s| s.bg(color_scheme().on_surface_variant)),
                            ),
                    )
                    .child(content()),
            )
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Edge {
    Left,
    Right,
}

fn edge_sheet(
    edge: Edge,
    max_width: f32,
    open: impl Fn() -> bool + 'static,
    on_dismiss: impl Fn() + 'static,
    content: impl Fn() -> Div + 'static,
) -> Show {
    // What it bounces past the edge with is covered by this much more of the sheet.
    const BEYOND: f32 = 40.0;
    let modal = Modal::new(open, on_dismiss, default_spatial(), fast_effects());
    let shown = modal.shown;
    let dismiss = modal.dismiss.clone();
    modal.view(move || {
        let width = signal(max_width);
        let dismiss = dismiss.clone();
        let side = if edge == Edge::Left { -1.0 } else { 1.0 };
        let radius = 16.0;
        let sheet = div()
            .flex()
            .flex_col()
            .h_full()
            .min_w(240.0)
            .max_w(max_width + BEYOND)
            .on_resized(move |w, _| width.set(w - BEYOND))
            .pointer_events_auto()
            .dyn_style(move |s| {
                let c = color_scheme();
                s.bg(c.surface_container_low)
                    .text_color(c.on_surface)
                    .translate_x(side * (1.0 - shown.get()) * width.get())
            })
            .on_grab(move |g| match g.phase {
                GrabPhase::Start => {}
                GrabPhase::Move => {
                    let w = width.get_untracked().max(1.0);
                    shown.set(1.0 - (g.total.x * side).max(0.0) / w);
                }
                GrabPhase::End => {
                    if shown.get_untracked() >= 1.0 {
                        return;
                    }
                    if shown.get_untracked() < 0.5 || g.velocity.x * side > 400.0 {
                        dismiss();
                    } else {
                        shown.spring_to(1.0, default_spatial());
                    }
                }
                GrabPhase::Cancel => shown.spring_to(1.0, default_spatial()),
            });
        let mut sheet = sheet;
        let style = sheet.style_mut();
        if edge == Edge::Left {
            style.radius_top_right = Some(radius);
            style.radius_bottom_right = Some(radius);
            style.margin_left = Some(Length::Px(-BEYOND));
            style.padding_left = Some(BEYOND);
        } else {
            style.radius_top_left = Some(radius);
            style.radius_bottom_left = Some(radius);
            style.margin_right = Some(Length::Px(-BEYOND));
            style.padding_right = Some(BEYOND);
        }
        let root = div().size_full().flex().flex_row();
        let root = if edge == Edge::Left {
            root.justify_start()
        } else {
            root.justify_end()
        };
        root.child(sheet.child(content()))
    })
}

/// A drawer that comes from the left of the window, usually with `drawer_item`s.
pub fn navigation_drawer(
    open: impl Fn() -> bool + 'static,
    on_dismiss: impl Fn() + 'static,
    content: impl Fn() -> Div + 'static,
) -> Show {
    edge_sheet(Edge::Left, 360.0, open, on_dismiss, content)
}

/// A sheet that comes from the right of the window.
pub fn side_sheet(
    open: impl Fn() -> bool + 'static,
    on_dismiss: impl Fn() + 'static,
    content: impl Fn() -> Div + 'static,
) -> Show {
    edge_sheet(Edge::Right, 400.0, open, on_dismiss, content)
}
