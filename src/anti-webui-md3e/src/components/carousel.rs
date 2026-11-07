//! The multi-browse carousel: items that get smaller toward its end as they scroll. Sizes and timing
//! are those of Jetpack Compose; there, items are masked by whatever shape the caller gives, and
//! here 28 rounds them. See `docs/design/20-md3e-components.md`.

use anti_webui::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

const MIN_SMALL: f32 = 40.0;
const MAX_SMALL: f32 = 56.0;
const ANCHOR: f32 = 10.0;
const CORNER: f32 = 28.0;
const GAP: f32 = 8.0;

/// Sizes of what is at each place of the carousel, from its start.
#[derive(Clone, Debug, PartialEq)]
pub struct Keylines {
    /// The size of the full items.
    pub large: f32,
    /// The size at each place, with an item that has almost gone at both ends.
    pub sizes: Vec<f32>,
}

/// Works out how many items of which sizes fit, as close to `preferred` as possible.
pub fn multi_browse_keylines(main: f32, preferred: f32, count: usize) -> Keylines {
    let target_large = preferred.min(main);
    let target_small = (target_large / 3.0).clamp(MIN_SMALL, MAX_SMALL);
    let target_medium = (target_large + target_small) / 2.0;
    let small_counts: &[usize] = if main < 2.0 * MIN_SMALL { &[0] } else { &[1] };
    let min_available = main - target_medium - MAX_SMALL * small_counts[0] as f32;
    let min_large = (min_available / target_large).floor().max(1.0) as usize;
    let max_large = (main / target_large).ceil().max(1.0) as usize;

    let mut best: Option<(f32, usize, usize, usize, f32, f32, f32)> = None;
    let mut priority = 1.0;
    for &small_count in small_counts {
        for medium_count in [1usize, 0] {
            for large_count in (min_large..=max_large).rev() {
                let total = large_count + medium_count + small_count;
                let available = main - GAP * (total.max(1) - 1) as f32;
                let mut small = target_small.clamp(MIN_SMALL, MAX_SMALL);
                let delta = available
                    - (target_large * large_count as f32
                        + target_medium * medium_count as f32
                        + small * small_count as f32);
                if small_count > 0 {
                    if delta > 0.0 {
                        small += (delta / small_count as f32).min(MAX_SMALL - small);
                    } else {
                        small += (delta / small_count as f32).max(MIN_SMALL - small);
                    }
                } else {
                    small = 0.0;
                }
                let large = (available - (small_count as f32 + medium_count as f32 / 2.0) * small)
                    / (large_count as f32 + medium_count as f32 / 2.0);
                let medium = (large + small) / 2.0;
                let valid = large > 0.0
                    && (medium_count == 0 || large > medium)
                    && (small_count == 0 || medium_count == 0 || medium > small);
                let cost = if valid {
                    (target_large - large).abs() * priority
                } else {
                    f32::MAX
                };
                if best.as_ref().is_none_or(|b| cost < b.0) {
                    best = Some((
                        cost,
                        large_count,
                        medium_count,
                        small_count,
                        large,
                        medium,
                        small,
                    ));
                }
                priority += 1.0;
            }
        }
    }
    let (_, mut large_count, mut medium_count, mut small_count, mut large, mut medium, mut small) =
        best.expect("at least one way");
    // With few items there is nothing to show at the smaller places.
    while large_count + medium_count + small_count > count.max(1) {
        if small_count > 0 {
            small_count -= 1;
            small = 0.0;
        } else if medium_count > 0 {
            medium_count -= 1;
            medium = 0.0;
        } else if large_count > 1 {
            large_count -= 1;
        } else {
            break;
        }
        let total = large_count + medium_count + small_count;
        let available = main - GAP * (total.max(1) - 1) as f32;
        large = (available - (small_count as f32 + medium_count as f32 / 2.0) * small)
            / (large_count as f32 + medium_count as f32 / 2.0);
        if medium_count > 0 {
            medium = (large + small) / 2.0;
        }
    }
    let mut sizes = vec![large; large_count];
    sizes.extend(std::iter::repeat_n(medium, medium_count));
    sizes.extend(std::iter::repeat_n(small, small_count));
    Keylines { large, sizes }
}

/// Where each item is at a scroll position: its left edge and its width.
fn place_items(lines: &Keylines, scroll: f32, count: usize, main: f32) -> Vec<(f32, f32)> {
    let step = lines.large + GAP;
    let places = lines.sizes.len();
    // Past the place where the last item is at the first place, the keylines turn around: the end
    // of the content has to fit at the end of the carousel.
    let end_scroll = (count as f32 * step - GAP - main).max(0.0);
    let mut out = Vec::with_capacity(count);
    // The left edge of each place, then one more for what has almost gone.
    let mut lefts = Vec::with_capacity(places + 2);
    let mut x = -(ANCHOR + GAP);
    lefts.push((x, ANCHOR));
    x = 0.0;
    for size in &lines.sizes {
        lefts.push((x, *size));
        x += size + GAP;
    }
    lefts.push((x, ANCHOR));
    let near_end = lines.sizes.iter().rev().copied().collect::<Vec<_>>();
    let mut turned = Vec::with_capacity(places + 2);
    let mut x = -(ANCHOR + GAP);
    turned.push((x, ANCHOR));
    x = 0.0;
    for size in &near_end {
        turned.push((x, *size));
        x += size + GAP;
    }
    turned.push((x, ANCHOR));
    let turn_over = (lines.large - lines.sizes.last().copied().unwrap_or(lines.large)).max(1.0)
        * (places as f32 - 1.0).max(1.0);
    let t = if end_scroll <= 0.0 {
        0.0
    } else {
        ((scroll - (end_scroll - turn_over)) / turn_over).clamp(0.0, 1.0)
    };
    for i in 0..count {
        let position = (i as f32 * step - scroll) / step + 1.0;
        let low = position.floor().clamp(0.0, (lefts.len() - 1) as f32) as usize;
        let high = (low + 1).min(lefts.len() - 1);
        let f = (position - low as f32).clamp(0.0, 1.0);
        let at = |list: &Vec<(f32, f32)>| {
            let (a, b) = (list[low], list[high]);
            (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f)
        };
        let (a, b) = (at(&lefts), at(&turned));
        let mut left = a.0 + (b.0 - a.0) * t;
        let width = a.1 + (b.1 - a.1) * t;
        if position < 0.0 {
            left = -(ANCHOR + GAP) + position * step;
        } else if position > (lefts.len() - 1) as f32 {
            left = lefts[lefts.len() - 1].0 + (position - (lefts.len() - 1) as f32) * step;
        }
        out.push((left, width));
    }
    out
}

/// A carousel of `count` items as high as it is. Items as wide as `preferred` go to the start of it
/// when dragged or scrolled, and it comes to rest on one. Each item is what `item` makes of its index,
/// and is kept at the width of a full item: the carousel only shows part of it near its end.
pub fn carousel<V: IntoView>(count: usize, preferred: f32, item: impl Fn(usize) -> V) -> Div {
    crate::ensure_ready();
    let main = signal(0.0f32);
    let scroll = animated(0.0f32);
    let lines = memo(move || multi_browse_keylines(main.get().max(1.0), preferred, count));
    let max_scroll = move || {
        let large = lines.with(|l| l.large);
        (count as f32 * (large + GAP) - GAP - main.get()).max(0.0)
    };
    let settle = move || {
        let large = lines.with(|l| l.large);
        let step = large + GAP;
        let to = ((scroll.get_untracked() / step).round() * step).clamp(0.0, untrack(max_scroll));
        scroll.spring_to(to, Spring::from_ratio(400.0, 1.0));
    };
    let waiting: Rc<Cell<Option<Timeout>>> = Rc::default();
    let from = Cell::new(0.0f32);
    let items: Vec<Div> = (0..count)
        .map(|i| {
            let content = item(i).into_view();
            div()
                .absolute()
                .top(0.0)
                .bottom(0.0)
                .overflow_hidden()
                .rounded(CORNER)
                .dyn_style(move |s| {
                    let places = lines.with(|l| place_items(l, scroll.get(), count, main.get()));
                    let (left, width) = places[i];
                    s.left(left).w(width.max(0.0))
                })
                .child(
                    // The middle of the item stays in the middle of what shows, as with the mask of
                    // Compose: a picture keeps showing what it is of as it gets narrow.
                    div()
                        .absolute()
                        .top(0.0)
                        .bottom(0.0)
                        .flex()
                        .flex_col()
                        .items_stretch()
                        .dyn_style(move |s| {
                            let large = lines.with(|l| l.large);
                            let places =
                                lines.with(|l| place_items(l, scroll.get(), count, main.get()));
                            s.w(large).left((places[i].1 - large) * 0.5)
                        })
                        .child(content),
                )
        })
        .collect();
    div()
        .relative()
        .w_full()
        .h(200.0)
        .overflow_hidden()
        .select_none()
        .cursor(CursorIcon::Grab)
        .on_resized(move |w, _| main.set(w))
        .on_scroll(move |e| {
            let delta = if e.delta_x != 0.0 {
                e.delta_x
            } else {
                e.delta_y
            };
            let scale = untrack(window_scale).as_f64() as f32;
            scroll.set((scroll.get_untracked() + delta / scale).clamp(0.0, untrack(max_scroll)));
            if let Some(running) = waiting.take() {
                running.cancel();
            }
            waiting.set(Some(set_timeout(Duration::from_millis(120), settle)));
            e.stop_propagation();
        })
        .on_grab(move |g| match g.phase {
            GrabPhase::Start => from.set(scroll.get_untracked()),
            GrabPhase::Move => {
                let to = from.get() - g.total.x;
                scroll.set(to.clamp(0.0, untrack(max_scroll)));
            }
            GrabPhase::End | GrabPhase::Cancel => settle(),
        })
        .children(items)
}
