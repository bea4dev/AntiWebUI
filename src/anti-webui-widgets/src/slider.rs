//! Sliders.

use crate::theme::{Theme, ring, shared};
use anti_webui_core::prelude::*;

fn slider_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.cursor(CursorIcon::Pointer).rounded(10.0))
        .focus_visible(|s| ring(s, t))
        .disabled(|s| s.cursor(CursorIcon::Default))
}

fn track_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(t.border))
}

fn fill_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(t.accent))
        .disabled(|s| s.bg(t.text_disabled))
        .accent_background()
}

fn thumb_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(Color::WHITE).border_color(t.border_strong))
        .hover(|s| s.border_color(t.accent))
        .active(|s| s.border_color(t.accent))
        .disabled(|s| s.bg(t.surface_muted).border_color(t.border))
        .accent_border()
}

/// A horizontal slider bound to `value` in `min..=max`, which goes in twentieths with the keys.
pub fn slider(value: Signal<f32>, min: f32, max: f32) -> Div {
    frame(value, min, max, None)
}

/// A slider that only stops at `min`, `min + step`, and so on.
pub fn stepped_slider(value: Signal<f32>, min: f32, max: f32, step: f32) -> Div {
    frame(value, min, max, Some(step))
}

fn frame(value: Signal<f32>, min: f32, max: f32, step: Option<f32>) -> Div {
    let snap = move |v: f32| {
        let v = v.clamp(min, max);
        match step.filter(|s| *s > 0.0) {
            Some(s) => (min + ((v - min) / s).round() * s).clamp(min, max),
            None => v,
        }
    };
    let fraction = move || ((value.get() - min) / (max - min)).clamp(0.0, 1.0);
    div()
        .relative()
        .h(20.0)
        .min_w(80.0)
        .flex()
        .items_center()
        .select_none()
        .focusable()
        .sheet(|| shared(slider_sheet))
        .grab_threshold(0.0)
        .on_grab(move |g| {
            if matches!(g.phase, GrabPhase::Start | GrabPhase::Move) {
                let t = (g.local.x / g.rect.width.max(1) as f32).clamp(0.0, 1.0);
                value.set(snap(min + (max - min) * t));
            }
        })
        .on_key_down(move |e| {
            let small = step.unwrap_or((max - min) / 20.0);
            let big = (small * 5.0).max((max - min) / 10.0);
            let now = value.get_untracked();
            let next = match e.key {
                Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowDown) => now - small,
                Key::Named(NamedKey::ArrowRight | NamedKey::ArrowUp) => now + small,
                Key::Named(NamedKey::PageDown) => now - big,
                Key::Named(NamedKey::PageUp) => now + big,
                Key::Named(NamedKey::Home) => min,
                Key::Named(NamedKey::End) => max,
                _ => return,
            };
            e.prevent_default();
            value.set(snap(next));
        })
        .child(
            div()
                .w_full()
                .h(4.0)
                .rounded(2.0)
                .sheet(|| shared(track_sheet))
                .child(
                    div()
                        .h(4.0)
                        .rounded(2.0)
                        .sheet(|| shared(fill_sheet))
                        .dyn_style(move |s| s.w(pct(fraction() * 100.0))),
                ),
        )
        .child(
            div()
                .absolute()
                .top(2.0)
                .w(16.0)
                .h(16.0)
                .rounded(8.0)
                .border(1.0)
                .shadow_sm()
                .sheet(|| shared(thumb_sheet))
                // Keep the thumb inside the track: left = fraction × (width − thumb).
                .dyn_style(move |s| s.left(pct(fraction() * 100.0)).ml(-16.0 * fraction())),
        )
}
