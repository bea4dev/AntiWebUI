//! Tooltips.
//!
//! `tooltip_with` keeps when a tooltip shows (the waits, the pointer, the keyboard) and leaves what
//! it looks like to the program. See `docs/design/33-customization.md`.

use crate::theme::{QUICK, Theme, themed};
use anti_webui_core::prelude::*;
use std::rc::Rc;
use std::time::Duration;

fn tip_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(t.tooltip).text_color(t.on_tooltip))
}

/// When a tooltip shows and where.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TooltipOptions {
    show_after: Duration,
    hide_after: Duration,
    on_focus: bool,
    kind: PopupKind,
    placement: Placement,
    gap: i32,
}

impl Default for TooltipOptions {
    fn default() -> Self {
        Self {
            show_after: Duration::from_millis(500),
            hide_after: Duration::ZERO,
            on_focus: true,
            kind: PopupKind::Tooltip,
            placement: Placement::AboveCenter,
            gap: 6,
        }
    }
}

impl TooltipOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// How long the pointer rests on the target before it shows. 500ms by default.
    pub fn show_after(mut self, wait: Duration) -> Self {
        self.show_after = wait;
        self
    }

    /// How long it stays after the pointer has left. At once by default.
    pub fn hide_after(mut self, wait: Duration) -> Self {
        self.hide_after = wait;
        self
    }

    /// Whether the focus from the keyboard on the target shows it, as it does by default.
    pub fn on_focus(mut self, shows: bool) -> Self {
        self.on_focus = shows;
        self
    }

    /// What kind of popup it is: a plain [`PopupKind::Tooltip`] by default, or a
    /// [`PopupKind::Bubble`] for one with things to press in it.
    pub fn kind(mut self, kind: PopupKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }

    /// The room between the target and the tooltip, in logical pixels.
    pub fn gap(mut self, gap: i32) -> Self {
        self.gap = gap;
        self
    }
}

const ON_TARGET: u8 = 1;
const ON_TIP: u8 = 2;
const FOCUSED: u8 = 4;

/// What a tooltip's look gets to see, and what it can ask for. Reading it tracks it.
#[derive(Clone, Copy)]
pub struct TooltipState {
    open: Signal<bool>,
    inside: Signal<u8>,
    hold: Signal<bool>,
    timer: Signal<Option<Timeout>>,
    options: TooltipOptions,
}

impl TooltipState {
    /// It should be up.
    pub fn open(&self) -> bool {
        self.open.get()
    }

    /// Keeps it there after it should go, while `f` returns true: for a look that fades out. Call
    /// it once, where the look is set up.
    pub fn hold_while(&self, f: impl Fn() -> bool + 'static) {
        let hold = self.hold;
        effect(move || hold.set_if_changed(f()));
    }

    /// Keeps it up while the pointer is over `el` too, as a tooltip with things to press in it
    /// needs.
    pub fn stay(&self, el: Div) -> Div {
        let state = *self;
        el.on_hover(move |on| state.track(ON_TIP, on))
    }

    /// Follows the pointer or the focus going in and out of one of the places that keep it up.
    fn track(self, place: u8, on: bool) {
        self.inside
            .update(|v| if on { *v |= place } else { *v &= !place });
        if let Some(running) = self.timer.get_untracked() {
            running.cancel();
            self.timer.set(None);
        }
        let inside = self.inside.get_untracked();
        let up = self.open.get_untracked();
        let (wait, to) = if inside != 0 {
            if up {
                return;
            }
            let wait = if inside & FOCUSED != 0 {
                Duration::ZERO
            } else {
                self.options.show_after
            };
            (wait, true)
        } else {
            if !up {
                return;
            }
            (self.options.hide_after, false)
        };
        if wait.is_zero() {
            self.open.set(to);
        } else {
            let open = self.open;
            self.timer
                .set(Some(set_timeout(wait, move || open.set(to))));
        }
    }

    /// Goes at once, as when the target is pressed.
    fn close(self) {
        if let Some(running) = self.timer.get_untracked() {
            running.cancel();
            self.timer.set(None);
        }
        self.open.set(false);
    }
}

type Part = Rc<dyn Fn(Div, TooltipState) -> Div>;

/// Changes to the parts of the standard tooltip. Each gets the part with its standard styles on and
/// returns it; what it says goes above the theme.
#[derive(Clone, Default)]
pub struct TooltipParts {
    bubble: Option<Part>,
}

impl TooltipParts {
    pub fn new() -> Self {
        Self::default()
    }

    /// The box with the text in it.
    pub fn bubble(mut self, f: impl Fn(Div, TooltipState) -> Div + 'static) -> Self {
        self.bubble = Some(Rc::new(f));
        self
    }
}

/// Shows a short text above `target` while the pointer rests on it or the focus from the keyboard
/// is on it. `target` is what comes back, and can be styled further.
pub fn tooltip(target: Div, label: impl Into<String>) -> Div {
    styled_tooltip(target, label, TooltipParts::new())
}

/// A tooltip with the standard look, with its parts changed by `parts`.
pub fn styled_tooltip(target: Div, label: impl Into<String>, parts: TooltipParts) -> Div {
    let label = label.into();
    tooltip_with(target, TooltipOptions::new(), move |tip| {
        let shown = follow(move || if tip.open() { 1.0f32 } else { 0.0 }, QUICK);
        tip.hold_while(move || shown.get() > 0.0);
        move || {
            let bubble = div()
                .max_w(280.0)
                .px(8.0)
                .py(4.0)
                .rounded(4.0)
                .text_size(12.0)
                .sheet(|| themed(tip_sheet))
                .dyn_style(move |s| s.opacity(shown.get().clamp(0.0, 1.0)))
                .child(text(label.clone()));
            match &parts.bubble {
                Some(f) => f(bubble, tip),
                None => bubble,
            }
        }
    })
}

/// A tooltip that looks like whatever is built. `setup` runs once with the state, and returns
/// what builds the tooltip each time it shows; it goes in a popup placed as `options` say. The
/// pointer resting on `target` or the focus from the keyboard on it shows it, and pressing
/// `target` hides it. `target` is what comes back.
///
/// ```ignore
/// tooltip_with(target, TooltipOptions::new(), |tip| {
///     let fade = follow(move || if tip.open() { 1.0 } else { 0.0 }, Transition::ms(150));
///     tip.hold_while(move || fade.get() > 0.0);
///     move || div().dyn_style(move |s| s.opacity(fade.get())).child("Saved")
/// })
/// ```
pub fn tooltip_with<F>(
    target: Div,
    options: TooltipOptions,
    setup: impl FnOnce(TooltipState) -> F,
) -> Div
where
    F: Fn() -> Div + 'static,
{
    let state = TooltipState {
        open: signal(false),
        inside: signal(0),
        hold: signal(false),
        timer: signal(None),
        options,
    };
    let content = setup(state);
    let target = target
        .on_hover(move |on| state.track(ON_TARGET, on))
        .on_pointer_down(move |_| state.close());
    let target = if options.on_focus {
        target.on_focus_visible(move |on| state.track(FOCUSED, on))
    } else {
        target
    };
    target.child(show(
        move || state.open.get() || state.hold.get(),
        move || {
            popup()
                .kind(options.kind)
                .placement(options.placement)
                .gap(options.gap)
                .child(content().access(|a| a.role(Role::Tooltip)))
        },
    ))
}
