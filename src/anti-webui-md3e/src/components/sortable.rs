//! Lists to put in another order, lifted and sprung as things are here. See
//! `docs/design/23-drag-and-drop.md`.

use super::surface::{elevation_shadows, focus_ring};
use crate::color::color_scheme;
use crate::motion::{default_spatial, fast_spatial};
use anti_webui::prelude::*;
use anti_webui::widgets::{SortAxis, SortLook, SortRow, sortable_with};
use std::hash::Hash;
use std::rc::Rc;

/// The elevation level of what is held: level 4 is 8 dp, what cards get when dragged in Compose.
const HELD: f32 = 4.0;

fn look() -> SortLook {
    SortLook {
        held: Rc::new(|s| {
            s.bg(color_scheme().surface_container_high)
                .shadows(elevation_shadows(HELD))
        }),
        focused: Rc::new(|s| focus_ring(s, true, &color_scheme())),
        make_room: Rc::new(|at, to| at.spring_to(to, fast_spatial())),
        settle: Rc::new(|at| at.spring_to(0.0, default_spatial())),
    }
}

/// A column of items that can be put in another order by moving them, or with Alt and the arrow
/// keys. `items` changes when an item is let go.
pub fn sortable_list<T, K, V>(
    items: Signal<Vec<T>>,
    key: impl Fn(&T) -> K + 'static,
    row: impl Fn(T, SortRow) -> V + 'static,
) -> Div
where
    T: Clone + 'static,
    K: Eq + Hash + Clone + 'static,
    V: IntoView,
{
    crate::ensure_ready();
    sortable_with(items, key, SortAxis::Vertical, look(), row)
}

/// The same across.
pub fn sortable_row<T, K, V>(
    items: Signal<Vec<T>>,
    key: impl Fn(&T) -> K + 'static,
    row: impl Fn(T, SortRow) -> V + 'static,
) -> Div
where
    T: Clone + 'static,
    K: Eq + Hash + Clone + 'static,
    V: IntoView,
{
    crate::ensure_ready();
    sortable_with(items, key, SortAxis::Horizontal, look(), row)
}
