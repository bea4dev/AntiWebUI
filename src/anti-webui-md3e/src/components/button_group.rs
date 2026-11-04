//! Groups of buttons. See `docs/design/19-buttons.md`.

use super::button::{ButtonStyle, Joined, styled_toggle_button};
use crate::motion::fast_spatial;
use anti_webui::core::geometry::Scale;
use anti_webui::core::tree::with_doc;
use anti_webui::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

const GAP: f32 = 12.0;
const CONNECTED_GAP: f32 = 2.0;
/// How much of its width a pressed button takes from its neighbors.
const EXPANDED_RATIO: f32 = 0.15;
/// The most a neighbor gives up.
const COMPRESSION_LIMIT: f32 = 24.0;

struct Item {
    /// How far it is pressed, from 0 to 1.
    pressed: Animated<f32>,
    /// What it takes from each neighbor when it is pressed all the way.
    reach: Cell<f32>,
}

/// How far the boundary after item `i` has moved from where it usually is, in physical pixels.
/// Everything follows from these whole numbers, so that what an item gains is exactly what its
/// neighbors lose and nothing around the group moves.
fn boundary_shift(items: &[Item], i: Option<usize>, scale: Scale) -> i32 {
    let Some(i) = i.filter(|i| i + 1 < items.len()) else {
        return 0;
    };
    let takes = |j: usize| items[j].pressed.get() * items[j].reach.get();
    scale.px(takes(i) - takes(i + 1))
}

/// How far the left and right edges of the content of item `i` are from where they usually are,
/// in physical pixels: what the boundaries gave, half on each side. The label stays centered, and
/// keeps still as long as both of its boundaries move apart evenly.
fn padding_change(items: &[Item], i: usize, scale: Scale) -> (i32, i32) {
    let before = boundary_shift(items, i.checked_sub(1), scale);
    let after = boundary_shift(items, Some(i), scale);
    let growth = after - before;
    let left = growth.div_euclid(2);
    (left, growth - left)
}

/// Buttons in a row, of which the one being pressed gets wider at the expense of its neighbors.
pub fn button_group(items: impl IntoIterator<Item = Div>) -> Div {
    let buttons: Vec<Div> = items.into_iter().collect();
    let count = buttons.len();
    let items: Rc<Vec<Item>> = Rc::new(
        (0..count)
            .map(|_| Item {
                pressed: animated(0.0),
                reach: Cell::new(0.0),
            })
            .collect(),
    );
    let mut group = div().flex().flex_row().flex_none().items_center().gap(GAP);
    for (i, mut button) in buttons.into_iter().enumerate() {
        let left = button.style_mut().padding_left.unwrap_or(0.0);
        let right = button.style_mut().padding_right.unwrap_or(0.0);
        let place = node_ref();
        let press = {
            let items = items.clone();
            Rc::new(move |pressed: bool| {
                // A press on the way back keeps what it had: the width has changed by then.
                if pressed && items[i].pressed.get_untracked() <= 0.0 {
                    let width = place
                        .rect()
                        .map_or(0.0, |r| with_doc(|d| d.scale.to_logical(r.width as f32)));
                    // Between two neighbors, each of them gives half.
                    let neighbors = (i > 0) as usize + (i + 1 < count) as usize;
                    let reach = if neighbors == 0 {
                        0.0
                    } else {
                        (EXPANDED_RATIO * width / neighbors as f32).min(COMPRESSION_LIMIT)
                    };
                    items[i].reach.set(reach);
                }
                let target = if pressed { 1.0 } else { 0.0 };
                items[i].pressed.spring_to(target, fast_spatial());
            })
        };
        let by_pointer = press.clone();
        let by_key = press.clone();
        let key_up = press.clone();
        let focus_lost = press;
        let shown = items.clone();
        let button = button
            .on_active(move |on| by_pointer(on))
            .on_key_down(move |e| {
                if !e.repeat && matches!(e.key, Key::Named(NamedKey::Enter | NamedKey::Space)) {
                    by_key(true);
                }
            })
            .on_key_up(move |e| {
                if matches!(e.key, Key::Named(NamedKey::Enter | NamedKey::Space)) {
                    key_up(false);
                }
            })
            // A key is released somewhere else if the focus moved on.
            .on_focus_change(move |on| {
                if !on {
                    focus_lost(false);
                }
            })
            .dyn_style(move |s| {
                let scale = with_doc(|d| d.scale);
                let (more_left, more_right) = padding_change(&shown, i, scale);
                // Whole physical pixels, which layout rounds back to the same.
                let side =
                    |base: f32, more: i32| scale.to_logical((scale.px(base) + more).max(0) as f32);
                s.pl(side(left, more_left)).pr(side(right, more_right))
            });
        // A box of its own for reading the width: a node has one `NodeRef`, which stays the
        // caller's.
        group = group.child(div().flex().flex_none().node_ref(place).child(button));
    }
    group
}

/// Toggle buttons that touch, of which what is on takes a shape of its own. Whether an item is on
/// comes from `is_checked` and what a click means from `on_select`, both by position.
pub fn connected_button_group<L: IntoTextSource>(
    labels: impl IntoIterator<Item = L>,
    style: ButtonStyle,
    is_checked: impl Fn(usize) -> bool + 'static,
    on_select: impl Fn(usize) + 'static,
) -> Div {
    let labels: Vec<L> = labels.into_iter().collect();
    let count = labels.len();
    let is_checked = Rc::new(is_checked);
    let on_select = Rc::new(on_select);
    let mut group = div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(CONNECTED_GAP);
    for (i, label) in labels.into_iter().enumerate() {
        let joined = match (i == 0, i + 1 == count) {
            (true, true) => Joined::Alone,
            (true, false) => Joined::First,
            (false, true) => Joined::Last,
            (false, false) => Joined::Middle,
        };
        let checked = is_checked.clone();
        let select = on_select.clone();
        group = group.child(
            styled_toggle_button(label, move || checked(i), style.clone().joined(joined))
                .on_click(move |_| select(i)),
        );
    }
    group
}
