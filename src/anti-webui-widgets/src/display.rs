//! What only shows something.

use crate::theme::{Theme, theme, themed};
use anti_webui_core::prelude::*;

/// A vertically scrolling container.
pub fn scroll_view() -> Div {
    div().overflow_y_scroll()
}

fn card_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(t.surface).border_color(t.border))
}

/// A bordered surface.
pub fn card() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(12.0)
        .p(16.0)
        .rounded(8.0)
        .border(1.0)
        .sheet(|| themed(card_sheet))
}

pub fn heading(label: impl IntoTextSource) -> Text {
    text(label)
        .text_size(20.0)
        .font_weight(FontWeight::SEMIBOLD)
}

pub fn label(label: impl IntoTextSource) -> Text {
    text(label)
        .text_size(12.0)
        .dyn_style(|s| s.text_color(theme().text_muted))
}

fn divider_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(t.border))
}

/// A thin horizontal line.
pub fn divider() -> Div {
    div().h(1.0).flex_none().sheet(|| themed(divider_sheet))
}

fn link_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.text_color(t.accent_text).cursor(CursorIcon::Pointer))
        .hover(|s| s.underline())
        .focus_visible(|s| crate::theme::focus_ring(s, t).rounded(2.0))
        .disabled(|s| s.text_color(t.text_disabled).cursor(CursorIcon::Default))
        .accent_text()
}

/// A text that does something when it is pressed.
pub fn link(label: impl IntoTextSource) -> Div {
    div()
        .focusable()
        .select_none()
        .sheet(|| themed(link_sheet))
        .child(text(label))
}

/// What a badge means.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
}

impl Tone {
    /// The background and the text of it.
    pub(crate) fn colors(self, t: &Theme) -> (Color, Color) {
        match self {
            Tone::Neutral => (t.surface_muted, t.text_muted),
            Tone::Accent => (t.accent_muted, t.accent_text),
            Tone::Success => (t.success_muted, t.success_text),
            Tone::Warning => (t.warning_muted, t.warning_text),
            Tone::Danger => (t.danger_muted, t.danger_text),
        }
    }
}

/// A small label, like a count or a state.
pub fn badge(label: impl IntoTextSource) -> Div {
    toned_badge(label, Tone::Neutral)
}

/// A badge that says what it means with its color.
pub fn toned_badge(label: impl IntoTextSource, tone: Tone) -> Div {
    div()
        .flex()
        .items_center()
        .px(8.0)
        .py(1.0)
        .rounded(999.0)
        .border(1.0)
        .text_size(12.0)
        .font_weight(FontWeight::MEDIUM)
        .select_none()
        .sheet(move || {
            themed(match tone {
                Tone::Neutral => |t| badge_sheet(t, Tone::Neutral),
                Tone::Accent => |t| badge_sheet(t, Tone::Accent),
                Tone::Success => |t| badge_sheet(t, Tone::Success),
                Tone::Warning => |t| badge_sheet(t, Tone::Warning),
                Tone::Danger => |t| badge_sheet(t, Tone::Danger),
            })
        })
        .child(text(label))
}

fn badge_sheet(t: &Theme, tone: Tone) -> Sheet {
    let (bg, fg) = tone.colors(t);
    Sheet::new(|s| s.bg(bg).text_color(fg).border_color(fg.with_alpha(0.3)))
}

fn track_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(t.border))
}

fn bar_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(t.accent))
        .disabled(|s| s.bg(t.text_disabled))
        .accent_background()
}

fn track() -> Div {
    div()
        .relative()
        .h(6.0)
        .min_w(40.0)
        .rounded(3.0)
        .overflow_clip()
        .sheet(|| themed(track_sheet))
}

/// How far something has come, from `0.0` to `1.0`.
pub fn progress_bar(value: impl Fn() -> f32 + 'static) -> Div {
    let value = std::rc::Rc::new(value);
    let told = value.clone();
    track()
        .access(move |a| {
            a.role(Role::ProgressBar)
                .value(told().clamp(0.0, 1.0) as f64, 0.0, 1.0)
        })
        .child(
            div()
                .h_full()
                .rounded(3.0)
                .sheet(|| themed(bar_sheet))
                .transition_of([Prop::Width], Transition::ms(200).ease_out())
                .dyn_style(move |s| s.w(pct(value().clamp(0.0, 1.0) * 100.0))),
        )
}

/// That something is going on, without knowing how far it is.
pub fn indeterminate_progress_bar() -> Div {
    let time = clock();
    const WIDTH: f32 = 30.0;
    track().access(|a| a.role(Role::ProgressBar)).child(
        div()
            .absolute()
            .top(0.0)
            .bottom(0.0)
            .w(pct(WIDTH))
            .rounded(3.0)
            .sheet(|| themed(bar_sheet))
            .dyn_style(move |s| {
                let t = (time.get() / 1.4).fract();
                s.left(pct(-WIDTH + t * (100.0 + WIDTH)))
            }),
    )
}

/// That something is going on, in a circle of 16 pixels.
pub fn spinner() -> Div {
    sized_spinner(16.0)
}

/// That something is going on, in a circle of this size. A ring is turned with a quarter of it
/// shown stronger, as a transform of one layer: nothing is laid out or drawn again.
pub fn sized_spinner(size: f32) -> Div {
    let time = clock();
    let width = (size / 8.0).max(2.0);
    let ring = move || {
        div()
            .absolute()
            .left(0.0)
            .top(0.0)
            .w(size)
            .h(size)
            .rounded(size / 2.0)
            .border(width)
    };
    div()
        .relative()
        .flex_none()
        .w(size)
        .h(size)
        .dyn_style(move |s| s.rotate((time.get() * 360.0 / 0.9) % 360.0))
        .child(ring().dyn_style(|s| s.border_color(theme().border)))
        .child(
            div()
                .absolute()
                .left(0.0)
                .top(0.0)
                .w(size / 2.0)
                .h(size / 2.0)
                .overflow_clip()
                .child(ring().sheet(|| themed(arc_sheet))),
        )
}

fn arc_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.border_color(t.accent)).accent_border()
}
