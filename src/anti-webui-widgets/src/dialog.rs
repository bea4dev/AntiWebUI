//! Dialogs.
//!
//! `modal_with` keeps what makes something modal (the portal, the focus, Escape and the scrim) and
//! leaves what it looks like to the program. See `docs/design/33-customization.md`.

use crate::theme::{Theme, fade, themed};
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

/// What the look of something modal gets to see, and what it can ask for. Reading it tracks it.
#[derive(Clone, Copy)]
pub struct ModalState {
    open: Memo<bool>,
    dismiss: Signal<Rc<dyn Fn()>>,
    hold: Signal<bool>,
}

impl ModalState {
    /// It should be up.
    pub fn open(&self) -> bool {
        self.open.get()
    }

    /// Asks for it to close: calls `on_dismiss`.
    pub fn dismiss(&self) {
        let dismiss = self.dismiss.get_untracked();
        dismiss();
    }

    /// Keeps it there after it should go, while `f` returns true: for a look that fades out. Call
    /// it once, where the look is set up.
    pub fn hold_while(&self, f: impl Fn() -> bool + 'static) {
        let hold = self.hold;
        effect(move || hold.set_if_changed(f()));
    }

    /// Makes `el` the scrim: clicking it dismisses.
    pub fn scrim(&self, el: Div) -> Div {
        let state = *self;
        el.pointer_events_auto().on_click(move |e| {
            e.stop_propagation();
            state.dismiss();
        })
    }

    /// Makes `el` what holds the content: it takes the pointer, and what is pressed in it goes no
    /// further, whatever the dialog is declared in.
    pub fn surface(&self, el: Div) -> Div {
        el.pointer_events_auto()
            .on_pointer_down(|e| e.stop_propagation())
            .on_click(|e| e.stop_propagation())
    }
}

type Part = Rc<dyn Fn(Div, ModalState) -> Div>;

/// Changes to the parts of the standard dialog. Each gets the part with its standard styles on and
/// returns it; what it says goes above the theme.
#[derive(Clone, Default)]
pub struct DialogParts {
    scrim: Option<Part>,
    surface: Option<Part>,
}

impl DialogParts {
    pub fn new() -> Self {
        Self::default()
    }

    /// What covers the rest of the window.
    pub fn scrim(mut self, f: impl Fn(Div, ModalState) -> Div + 'static) -> Self {
        self.scrim = Some(Rc::new(f));
        self
    }

    /// The box in the middle that holds the content.
    pub fn surface(mut self, f: impl Fn(Div, ModalState) -> Div + 'static) -> Self {
        self.surface = Some(Rc::new(f));
        self
    }
}

/// What is built by `content` in the middle of the window, while `open` says so, on a scrim that
/// takes the rest of the window. It keeps Tab inside, and gives the focus back when it closes.
/// Pressing the scrim and Escape call `on_dismiss`, which is the place to change `open`.
pub fn dialog<V: IntoView>(
    open: impl Fn() -> bool + 'static,
    on_dismiss: impl Fn() + 'static,
    content: impl Fn() -> V + 'static,
) -> Show {
    styled_dialog(open, on_dismiss, content, DialogParts::new())
}

/// A dialog with the standard look, with its parts changed by `parts`.
pub fn styled_dialog<V: IntoView>(
    open: impl Fn() -> bool + 'static,
    on_dismiss: impl Fn() + 'static,
    content: impl Fn() -> V + 'static,
    parts: DialogParts,
) -> Show {
    modal_with(open, on_dismiss, move |m| {
        let shown = fade(move || m.open());
        m.hold_while(move || shown.get() > 0.0);
        move || {
            let scrim = div().absolute().inset(0.0).dyn_style(move |s| {
                let scrim = crate::theme().scrim;
                s.bg(scrim.with_alpha(scrim.a * shown.get().clamp(0.0, 1.0)))
            });
            let scrim = match &parts.scrim {
                Some(f) => f(scrim, m),
                None => scrim,
            };
            let surface = div()
                .flex()
                .flex_col()
                .min_w(280.0)
                .max_w(560.0)
                .max_h(pct(100.0))
                .rounded(12.0)
                .border(1.0)
                .sheet(|| themed(surface_sheet))
                .dyn_style(move |s| {
                    let v = shown.get().clamp(0.0, 1.0);
                    s.opacity(v).scale(0.97 + 0.03 * v)
                })
                .child(content());
            let surface = match &parts.surface {
                Some(f) => f(surface, m),
                None => surface,
            };
            div().size_full().child(m.scrim(scrim)).child(
                div()
                    .absolute()
                    .inset(0.0)
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(24.0)
                    .child(m.surface(surface)),
            )
        }
    })
}

/// Something modal that looks like whatever is built. `setup` runs once with the state, and
/// returns what builds the content each time it opens; that fills the window, above everything,
/// and takes no pointer but where [`ModalState::scrim`] and [`ModalState::surface`] say. While
/// `open` says so or the look holds it, it keeps Tab inside, and gives the focus back when it
/// closes. Escape and the scrim call `on_dismiss`, which is the place to change `open`.
pub fn modal_with<F, V>(
    open: impl Fn() -> bool + 'static,
    on_dismiss: impl Fn() + 'static,
    setup: impl FnOnce(ModalState) -> F,
) -> Show
where
    F: Fn() -> V + 'static,
    V: IntoView,
{
    let dismiss: Rc<dyn Fn()> = Rc::new(on_dismiss);
    let state = ModalState {
        open: memo(open),
        dismiss: signal(dismiss),
        hold: signal(false),
    };
    let content = setup(state);
    show(
        move || state.open.get() || state.hold.get(),
        move || {
            let root = node_ref();
            let view = portal()
                .modal()
                .child(
                    div()
                        .absolute()
                        .inset(0.0)
                        .focusable()
                        .node_ref(root)
                        .access(|a| a.role(Role::Dialog))
                        .pointer_events_none()
                        .on_key_down(move |e| {
                            e.stop_propagation();
                            if matches!(e.key, Key::Named(NamedKey::Escape)) {
                                e.prevent_default();
                                state.dismiss();
                            }
                        })
                        .child(content()),
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
