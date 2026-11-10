//! Pointers, scrolling and pinching: AppKit's events to the core's (see "入力" in
//! `docs/design/38-platform-macos.md`).

use anti_webui_core::event::{PinchPhase, PointerButton, ScrollPhase, ScrollSource};
use objc2_app_kit::NSEventPhase;

pub(crate) fn button_of(number: isize) -> PointerButton {
    match number {
        0 => PointerButton::Primary,
        1 => PointerButton::Secondary,
        2 => PointerButton::Middle,
        n => PointerButton::Other(n as u32),
    }
}

/// What a scroll event of AppKit is to the core: deltas, where it comes from and where it is in a
/// gesture. `None` for what says nothing, like the fingers staying still.
///
/// AppKit's deltas say which way the content goes, natural scrolling included; the core's positive
/// deltas move the content up and left, so they are turned around. Touchpads give points, which
/// `scale` makes physical pixels; wheels give lines, which count as notches.
pub(crate) fn scroll_of(
    precise: bool,
    phase: NSEventPhase,
    momentum: NSEventPhase,
    delta: (f64, f64),
    scale: f64,
) -> Option<(f32, f32, ScrollSource, ScrollPhase)> {
    if !precise {
        let (dx, dy) = (-delta.0 as f32, -delta.1 as f32);
        return (dx != 0.0 || dy != 0.0).then_some((
            dx,
            dy,
            ScrollSource::Wheel,
            ScrollPhase::Single,
        ));
    }
    let (dx, dy) = ((-delta.0 * scale) as f32, (-delta.1 * scale) as f32);
    if momentum != NSEventPhase::None {
        let p = if momentum.contains(NSEventPhase::Ended)
            || momentum.contains(NSEventPhase::Cancelled)
        {
            ScrollPhase::MomentumEnded
        } else {
            ScrollPhase::Momentum
        };
        return Some((dx, dy, ScrollSource::Touchpad, p));
    }
    let p = if phase == NSEventPhase::None {
        // Precise but without a gesture, like some mice.
        return (dx != 0.0 || dy != 0.0).then_some((
            dx,
            dy,
            ScrollSource::Unknown,
            ScrollPhase::Single,
        ));
    } else if phase.contains(NSEventPhase::MayBegin) {
        ScrollPhase::Held
    } else if phase.contains(NSEventPhase::Began) {
        ScrollPhase::Began
    } else if phase.contains(NSEventPhase::Changed) {
        ScrollPhase::Moved
    } else if phase.contains(NSEventPhase::Ended) || phase.contains(NSEventPhase::Cancelled) {
        // AppKit sends inertia of its own after this.
        ScrollPhase::Lifted { fling: false }
    } else {
        return None;
    };
    Some((dx, dy, ScrollSource::Touchpad, p))
}

pub(crate) fn pinch_phase(phase: NSEventPhase) -> Option<PinchPhase> {
    if phase.contains(NSEventPhase::Began) {
        Some(PinchPhase::Began)
    } else if phase.contains(NSEventPhase::Changed) {
        Some(PinchPhase::Moved)
    } else if phase.contains(NSEventPhase::Ended) {
        Some(PinchPhase::Ended)
    } else if phase.contains(NSEventPhase::Cancelled) {
        Some(PinchPhase::Cancelled)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: NSEventPhase = NSEventPhase::None;

    #[test]
    fn wheels_scroll_by_notches_the_other_way() {
        let s = scroll_of(false, NONE, NONE, (0.0, 1.0), 2.0).unwrap();
        assert_eq!(s, (0.0, -1.0, ScrollSource::Wheel, ScrollPhase::Single));
        assert_eq!(scroll_of(false, NONE, NONE, (0.0, 0.0), 2.0), None);
    }

    #[test]
    fn touchpads_scroll_by_physical_pixels_in_gestures() {
        let changed = scroll_of(true, NSEventPhase::Changed, NONE, (3.0, -4.5), 2.0).unwrap();
        assert_eq!(
            changed,
            (-6.0, 9.0, ScrollSource::Touchpad, ScrollPhase::Moved)
        );
        let phases = [
            (NSEventPhase::MayBegin, NONE, ScrollPhase::Held),
            (NSEventPhase::Began, NONE, ScrollPhase::Began),
            (
                NSEventPhase::Ended,
                NONE,
                ScrollPhase::Lifted { fling: false },
            ),
            (
                NSEventPhase::Cancelled,
                NONE,
                ScrollPhase::Lifted { fling: false },
            ),
            (NONE, NSEventPhase::Began, ScrollPhase::Momentum),
            (NONE, NSEventPhase::Changed, ScrollPhase::Momentum),
            (NONE, NSEventPhase::Ended, ScrollPhase::MomentumEnded),
        ];
        for (phase, momentum, want) in phases {
            let got = scroll_of(true, phase, momentum, (0.0, 1.0), 1.0).map(|s| s.3);
            assert_eq!(got, Some(want), "{phase:?} {momentum:?}");
        }
        assert_eq!(
            scroll_of(true, NSEventPhase::Stationary, NONE, (0.0, 0.0), 1.0),
            None
        );
    }

    #[test]
    fn precise_scrolls_without_gestures_are_single() {
        let s = scroll_of(true, NONE, NONE, (0.0, 2.0), 1.0).unwrap();
        assert_eq!(s, (0.0, -2.0, ScrollSource::Unknown, ScrollPhase::Single));
    }

    #[test]
    fn buttons_and_pinches() {
        assert_eq!(button_of(0), PointerButton::Primary);
        assert_eq!(button_of(1), PointerButton::Secondary);
        assert_eq!(button_of(2), PointerButton::Middle);
        assert_eq!(button_of(4), PointerButton::Other(4));
        assert_eq!(pinch_phase(NSEventPhase::Began), Some(PinchPhase::Began));
        assert_eq!(pinch_phase(NSEventPhase::Changed), Some(PinchPhase::Moved));
        assert_eq!(pinch_phase(NSEventPhase::Ended), Some(PinchPhase::Ended));
        assert_eq!(
            pinch_phase(NSEventPhase::Cancelled),
            Some(PinchPhase::Cancelled)
        );
        assert_eq!(pinch_phase(NONE), None);
    }
}
