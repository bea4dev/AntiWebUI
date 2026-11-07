//! Lists whose items can be put in another order by moving them. While that goes on only transforms
//! change: the list is laid out once, when it is let go. See `docs/design/23-drag-and-drop.md`.

use crate::theme::{QUICK, focus_ring, theme};
use anti_webui_core::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;

/// Which way a list goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortAxis {
    Vertical,
    Horizontal,
}

/// What moving items looks like.
#[derive(Clone)]
pub struct SortLook {
    /// What an item looks like while it is held.
    pub held: Rc<dyn Fn(Style) -> Style>,
    /// What an item looks like with the focus from the keyboard.
    pub focused: Rc<dyn Fn(Style) -> Style>,
    /// Sends an item where it goes to make room.
    pub make_room: Rc<dyn Fn(Animated<f32>, f32)>,
    /// Sends what was let go to its place.
    pub settle: Rc<dyn Fn(Animated<f32>)>,
}

/// What a row has to do with being moved.
pub struct SortRow {
    held: Memo<bool>,
    grip: Rc<dyn Fn(&GrabEvent)>,
    has_grip: Rc<Cell<bool>>,
}

impl SortRow {
    /// Whether it is being moved. Reading it tracks it.
    pub fn grabbed(&self) -> bool {
        self.held.get()
    }

    /// Makes `el` what it is moved by, instead of all of it.
    pub fn grip(&self, el: Div) -> Div {
        self.has_grip.set(true);
        let grab = self.grip.clone();
        el.cursor(CursorIcon::Grab)
            .select_none()
            .grab_autoscroll()
            .on_grab(move |g| grab(g))
    }
}

struct RowHandle {
    node: NodeRef,
    at: Animated<f32>,
}

/// The list as it was when an item was grabbed, in logical pixels of the window.
struct Moving<K> {
    keys: Vec<K>,
    start: Vec<f32>,
    size: Vec<f32>,
    gap: f32,
    from: usize,
    to: usize,
    list_start: f32,
    offset: f32,
}

impl<K> Moving<K> {
    fn room(&self) -> f32 {
        self.size[self.from] + self.gap
    }

    /// Where an item goes to make room, with what is held going to `to`.
    fn shift(&self, i: usize, to: usize) -> f32 {
        if i > self.from && i <= to {
            -self.room()
        } else if i < self.from && i >= to {
            self.room()
        } else {
            0.0
        }
    }

    /// Where what is held would be, from where it was.
    fn place_of_held(&self) -> f32 {
        let (a, b) = (self.from, self.to);
        if b > a {
            (a + 1..=b).map(|i| self.size[i] + self.gap).sum()
        } else {
            -(b..a).map(|i| self.size[i] + self.gap).sum::<f32>()
        }
    }

    /// Where it would go with its middle at `middle`.
    fn index_for(&self, middle: f32) -> usize {
        let mid = |i: usize| self.start[i] + self.size[i] / 2.0;
        let after = (self.from + 1..self.keys.len())
            .take_while(|&i| middle > mid(i))
            .last();
        let before = (0..self.from).rev().take_while(|&i| middle < mid(i)).last();
        after.or(before).unwrap_or(self.from)
    }
}

struct Sorting<K> {
    rows: RefCell<HashMap<K, RowHandle>>,
    moving: RefCell<Option<Moving<K>>>,
    list: NodeRef,
    axis: SortAxis,
    look: SortLook,
}

impl<K: Eq + Hash + Clone + 'static> Sorting<K> {
    fn along(&self, x: f32, y: f32) -> f32 {
        match self.axis {
            SortAxis::Vertical => y,
            SortAxis::Horizontal => x,
        }
    }

    fn list_start(&self) -> f32 {
        let scale = window_scale_untracked();
        self.list
            .rect()
            .map_or(0.0, |r| self.along(r.x as f32 / scale, r.y as f32 / scale))
    }

    /// Measures what is there.
    fn begin(&self, keys: Vec<K>, key: &K) -> bool {
        let scale = window_scale_untracked();
        let rows = self.rows.borrow();
        let mut start = Vec::with_capacity(keys.len());
        let mut size = Vec::with_capacity(keys.len());
        for k in &keys {
            let Some(r) = rows.get(k).and_then(|row| row.node.rect()) else {
                return false;
            };
            start.push(self.along(r.x as f32, r.y as f32) / scale);
            size.push(self.along(r.width as f32, r.height as f32) / scale);
        }
        let Some(from) = keys.iter().position(|k| k == key) else {
            return false;
        };
        let gap = if keys.len() > 1 {
            start[1] - start[0] - size[0]
        } else {
            0.0
        };
        *self.moving.borrow_mut() = Some(Moving {
            keys,
            start,
            size,
            gap,
            from,
            to: from,
            list_start: self.list_start(),
            offset: 0.0,
        });
        true
    }

    /// Follows the pointer, and has the others make room.
    fn follow(&self, total: f32) {
        let list_now = self.list_start();
        let mut moving = self.moving.borrow_mut();
        let Some(m) = moving.as_mut() else {
            return;
        };
        // What contains it may have scrolled.
        m.offset = total - (list_now - m.list_start);
        let rows = self.rows.borrow();
        if let Some(row) = rows.get(&m.keys[m.from]) {
            row.at.set(m.offset);
        }
        let middle = m.start[m.from] + m.size[m.from] / 2.0 + m.offset;
        let to = m.index_for(middle);
        if to == m.to {
            return;
        }
        let (lo, hi) = (to.min(m.to).min(m.from), to.max(m.to).max(m.from));
        for i in (lo..=hi).filter(|&i| i != m.from) {
            let (was, now) = (m.shift(i, m.to), m.shift(i, to));
            if was != now
                && let Some(row) = rows.get(&m.keys[i])
            {
                (self.look.make_room)(row.at, now);
            }
        }
        m.to = to;
    }

    /// Lets go of it, and says what it moved from and to if that isn't where it was.
    fn end(&self, keep: bool) -> Option<(K, usize)> {
        let m = self.moving.borrow_mut().take()?;
        let rows = self.rows.borrow();
        let held = rows.get(&m.keys[m.from]);
        if keep {
            // In the same batch as the new order, so that nothing shows in between.
            for k in &m.keys {
                if *k != m.keys[m.from]
                    && let Some(row) = rows.get(k)
                {
                    row.at.set(0.0);
                }
            }
            if let Some(row) = held {
                row.at.set(m.offset - m.place_of_held());
                (self.look.settle)(row.at);
            }
            (m.to != m.from).then(|| (m.keys[m.from].clone(), m.to))
        } else {
            for k in &m.keys {
                if let Some(row) = rows.get(k) {
                    (self.look.settle)(row.at);
                }
            }
            None
        }
    }
}

fn window_scale_untracked() -> f32 {
    untrack(window_scale).as_f32()
}

/// A list with its look given. See [`sortable_list`].
pub fn sortable_with<T, K, V>(
    items: Signal<Vec<T>>,
    key: impl Fn(&T) -> K + 'static,
    axis: SortAxis,
    look: SortLook,
    row: impl Fn(T, SortRow) -> V + 'static,
) -> Div
where
    T: Clone + 'static,
    K: Eq + Hash + Clone + 'static,
    V: IntoView,
{
    let key: Rc<dyn Fn(&T) -> K> = Rc::new(key);
    let list = node_ref();
    let sorting = Rc::new(Sorting {
        rows: RefCell::new(HashMap::new()),
        moving: RefCell::new(None),
        list,
        axis,
        look: look.clone(),
    });
    let held = signal(None::<K>);
    let row_key = key.clone();
    let view = move |item: T| {
        let k = row_key(&item);
        let at = animated(0.0f32);
        let node = node_ref();
        sorting
            .rows
            .borrow_mut()
            .insert(k.clone(), RowHandle { node, at });
        {
            let (sorting, k) = (sorting.clone(), k.clone());
            on_cleanup(move || {
                sorting.rows.borrow_mut().remove(&k);
            });
        }
        let is_held = {
            let k = k.clone();
            memo(move || held.with(|h| h.as_ref() == Some(&k)))
        };
        let grab: Rc<dyn Fn(&GrabEvent)> = {
            let (sorting, key, k) = (sorting.clone(), row_key.clone(), k.clone());
            Rc::new(move |g: &GrabEvent| match g.phase {
                GrabPhase::Start => {
                    let keys = items.with_untracked(|v| v.iter().map(|t| key(t)).collect());
                    if sorting.begin(keys, &k) {
                        held.set(Some(k.clone()));
                    }
                }
                GrabPhase::Move => sorting.follow(sorting.along(g.total.x, g.total.y)),
                GrabPhase::End => {
                    held.set(None);
                    if let Some((moved, to)) = sorting.end(true) {
                        items.update(|v| {
                            if let Some(i) = v.iter().position(|t| key(t) == moved) {
                                let t = v.remove(i);
                                v.insert(to.min(v.len()), t);
                            }
                        });
                    }
                }
                GrabPhase::Cancel => {
                    held.set(None);
                    sorting.end(false);
                }
            })
        };
        let has_grip = Rc::new(Cell::new(false));
        let content = row(
            item,
            SortRow {
                held: is_held,
                grip: grab.clone(),
                has_grip: has_grip.clone(),
            },
        );
        let lifted = look.held.clone();
        let ringed = look.focused.clone();
        let focus_shown = signal(false);
        let mut el = div()
            .relative()
            .focusable()
            .node_ref(node)
            .on_focus_visible(move |on| focus_shown.set(on))
            .dyn_style(move |s| {
                let s = if focus_shown.get() { ringed(s) } else { s };
                let by = at.get();
                let s = match axis {
                    SortAxis::Vertical => s.translate_y(by),
                    SortAxis::Horizontal => s.translate_x(by),
                };
                let s = if by != 0.0 || is_held.get() {
                    s.z_index(1)
                } else {
                    s
                };
                if is_held.get() { lifted(s) } else { s }
            })
            .on_key_down({
                let (key, k) = (row_key.clone(), k.clone());
                move |e| {
                    if !e.modifiers.alt {
                        return;
                    }
                    let by: isize = match (&e.key, axis) {
                        (Key::Named(NamedKey::ArrowUp), SortAxis::Vertical)
                        | (Key::Named(NamedKey::ArrowLeft), SortAxis::Horizontal) => -1,
                        (Key::Named(NamedKey::ArrowDown), SortAxis::Vertical)
                        | (Key::Named(NamedKey::ArrowRight), SortAxis::Horizontal) => 1,
                        _ => return,
                    };
                    e.prevent_default();
                    e.stop_propagation();
                    items.update(|v| {
                        if let Some(i) = v.iter().position(|t| key(t) == k) {
                            let to = i as isize + by;
                            if to >= 0 && (to as usize) < v.len() {
                                v.swap(i, to as usize);
                            }
                        }
                    });
                    node.scroll_into_view();
                }
            })
            .child(content);
        if !has_grip.get() {
            el = el.select_none().grab_autoscroll().on_grab(move |g| grab(g));
        }
        el
    };
    let list_box = div().flex().node_ref(list);
    let list_box = match axis {
        SortAxis::Vertical => list_box.flex_col(),
        SortAxis::Horizontal => list_box.flex_row(),
    };
    let key = key.clone();
    list_box.child(for_each(move || items.get(), move |t| key(t), view))
}

fn look() -> SortLook {
    SortLook {
        held: Rc::new(|s| s.bg(theme().surface).shadow_md()),
        focused: Rc::new(|s| focus_ring(s, &theme())),
        make_room: Rc::new(|at, to| at.to(to, QUICK)),
        settle: Rc::new(|at| at.to(0.0, QUICK)),
    }
}

/// A column of items that can be put in another order by moving them, or with Alt and the arrow
/// keys. `items` changes when an item is let go. Gaps come from `.gap(...)` on what this returns.
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
    sortable_with(items, key, SortAxis::Horizontal, look(), row)
}
