//! What is too long to build all of: lists, trees and tables, of which only the rows that show
//! exist. See `docs/design/21-widgets.md`.

use crate::theme::{Theme, ring, shared, theme};
use anti_webui_core::prelude::*;
use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::hash::Hash;
use std::rc::Rc;

/// Rows beyond those that show, on each side, so that a little scrolling doesn't build any.
const SPARE: i64 = 8;
/// The first row built is a multiple of this, so that rows get built in small groups.
const GROUP: i64 = 4;

/// The rows of a list from where it is scrolled to.
fn range(top: i32, height: i32, row: i32, count: usize) -> (usize, usize) {
    let row = row.max(1) as i64;
    let first = ((top as i64 / row) - SPARE).max(0) / GROUP * GROUP;
    let last = ((top as i64 + height as i64) / row + 1 + SPARE).min(count as i64);
    (first as usize, last.max(first) as usize)
}

/// A scroller with its rows. `version` says which rows have to be built again.
fn rows_in<V: IntoView>(
    count: impl Fn() -> usize + 'static,
    version: impl Fn() -> u64 + 'static,
    row_height: f32,
    row: impl Fn(usize) -> V + 'static,
    extra: impl Fn(Style) -> Style + 'static,
    on_x: impl Fn(f32) + 'static,
) -> Div {
    let offset = signal(0.0f32);
    let height = signal(0.0f32);
    let count = Rc::new(count);
    let total = count.clone();
    let visible = memo(move || {
        let scale = window_scale();
        let (first, last) = range(
            scale.px(offset.get()),
            scale.px(height.get()),
            scale.px(row_height),
            total(),
        );
        (first, last, version())
    });
    div()
        .overflow_y_scroll()
        .flex_1()
        .min_h(0.0)
        .on_scrolled(move |at| {
            offset.set_if_changed(at.y);
            on_x(at.x);
        })
        .on_resized(move |_, h| height.set_if_changed(h))
        .child(
            div()
                .relative()
                .w_full()
                .flex_none()
                .dyn_style(move |s| {
                    let each = window_scale().px(row_height) as i64;
                    let all = (each * count() as i64).min(i32::MAX as i64) as i32;
                    extra(s).h(Length::Device(all))
                })
                .child(for_each(
                    move || {
                        let (first, last, version) = visible.get();
                        (first..last).map(|i| (i, version)).collect::<Vec<_>>()
                    },
                    |item| *item,
                    move |(i, _)| {
                        div()
                            .absolute()
                            .left(0.0)
                            .right(0.0)
                            .h(row_height)
                            .dyn_style(move |s| {
                                let each = window_scale().px(row_height) as i64;
                                s.top(Length::Device((each * i as i64).min(i32::MAX as i64) as i32))
                            })
                            .child(row(i))
                    },
                )),
        )
}

/// A list of `count()` rows of the same height, of which only those that show exist. `row` builds
/// one, and runs again when it comes back into view.
pub fn virtual_list<V: IntoView>(
    count: impl Fn() -> usize + 'static,
    row_height: f32,
    row: impl Fn(usize) -> V + 'static,
) -> Div {
    rows_in(count, || 0, row_height, row, |s| s, |_| {})
}

/// Scrolls a [`virtual_list`] just far enough to show the row at `index`.
pub fn scroll_row_into_view(list: NodeRef, index: usize, row_height: f32) {
    let scale = untrack(window_scale);
    let each = scale.px(row_height) as f32 / scale.as_f32();
    let at = list.scroll_offset();
    let Some(rect) = list.rect() else {
        return;
    };
    let shown = rect.height as f32 / scale.as_f32();
    let top = each * index as f32;
    let y = if top < at.y {
        top
    } else if top + each > at.y + shown {
        top + each - shown
    } else {
        return;
    };
    list.scroll_to(at.x, y);
}

/// Something in a [`tree_view`].
#[derive(Clone)]
pub struct TreeItem<K> {
    key: K,
    label: String,
    children: Vec<TreeItem<K>>,
}

/// Something in a [`tree_view`], which `selected` knows it by.
pub fn tree_item<K>(key: K, label: impl Into<String>) -> TreeItem<K> {
    TreeItem {
        key,
        label: label.into(),
        children: Vec::new(),
    }
}

impl<K> TreeItem<K> {
    pub fn child(mut self, item: TreeItem<K>) -> Self {
        self.children.push(item);
        self
    }

    pub fn children(mut self, items: impl IntoIterator<Item = TreeItem<K>>) -> Self {
        self.children.extend(items);
        self
    }
}

/// A row of a tree as it shows.
#[derive(Clone)]
struct TreeRow<K> {
    key: K,
    label: String,
    depth: usize,
    parent: Option<usize>,
    opens: bool,
}

fn flatten<K: Clone + Eq + Hash>(
    items: &[TreeItem<K>],
    open: &HashSet<K>,
    depth: usize,
    parent: Option<usize>,
    out: &mut Vec<TreeRow<K>>,
) {
    for item in items {
        let at = out.len();
        out.push(TreeRow {
            key: item.key.clone(),
            label: item.label.clone(),
            depth,
            parent,
            opens: !item.children.is_empty(),
        });
        if open.contains(&item.key) {
            flatten(&item.children, open, depth + 1, Some(at), out);
        }
    }
}

fn long_box_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(t.surface).border_color(t.border))
        .focus_visible(|s| ring(s, t))
        .disabled(|s| s.bg(t.surface_muted).text_color(t.text_disabled))
}

fn row_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.cursor(CursorIcon::Pointer))
        .hover(|s| s.bg(t.hover))
        .disabled(|s| s.bg(Color::TRANSPARENT).cursor(CursorIcon::Default))
}

fn picked_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.bg(t.selected).cursor(CursorIcon::Pointer))
        .disabled(|s| s.bg(t.surface_muted).cursor(CursorIcon::Default))
}

const TREE_ROW: f32 = 26.0;

/// A tree of `items`, bound to the key of what is selected. What is closed costs nothing, and only
/// the rows that show exist.
pub fn tree_view<K: Clone + Eq + Hash + 'static>(
    items: Vec<TreeItem<K>>,
    selected: Signal<Option<K>>,
) -> Div {
    let open = signal(HashSet::<K>::new());
    let items = Rc::new(items);
    let rows = {
        let items = items.clone();
        let flat = signal(Vec::<TreeRow<K>>::new());
        effect(move || {
            let mut out = Vec::new();
            open.with(|o| flatten(&items, o, 0, None, &mut out));
            flat.set(out);
        });
        flat
    };
    let version = signal(0u64);
    effect(move || {
        rows.with(|_| ());
        untrack(|| version.update(|v| *v += 1));
    });
    let list = node_ref();
    let flip = move |key: &K| {
        open.update(|o| {
            if !o.remove(key) {
                o.insert(key.clone());
            }
        })
    };
    let row_key = selected;
    rows_in(
        move || rows.with(Vec::len),
        move || version.get(),
        TREE_ROW,
        move |i| {
            let Some(row) = rows.with_untracked(|r| r.get(i).cloned()) else {
                return div();
            };
            let (key, chevron_key, mine) = (row.key.clone(), row.key.clone(), row.key.clone());
            let is_open = open.with_untracked(|o| o.contains(&row.key));
            div()
                .h_full()
                .flex()
                .flex_row()
                .items_center()
                .pl(4.0 + row.depth as f32 * 16.0)
                .pr(8.0)
                .gap(2.0)
                .sheet(move || {
                    shared(if row_key.with(|s| s.as_ref() == Some(&mine)) {
                        picked_sheet
                    } else {
                        row_sheet
                    })
                })
                .on_click(move |_| selected.set(Some(key.clone())))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .w(16.0)
                        .h(16.0)
                        .text_size(13.0)
                        .dyn_style(|s| s.text_color(theme().text_muted))
                        .on_click(move |e| {
                            if row.opens {
                                e.stop_propagation();
                                flip(&chevron_key);
                            }
                        })
                        .child(text(match (row.opens, is_open) {
                            (false, _) => "",
                            (true, false) => "▸",
                            (true, true) => "▾",
                        })),
                )
                .child(text(row.label).whitespace_nowrap())
        },
        |s| s,
        |_| {},
    )
    .rounded(6.0)
    .border(1.0)
    .select_none()
    .focusable()
    .node_ref(list)
    .sheet(|| shared(long_box_sheet))
    .on_key_down(move |e| {
        let at = rows.with_untracked(|r| {
            selected.with_untracked(|s| {
                s.as_ref()
                    .and_then(|key| r.iter().position(|row| row.key == *key))
            })
        });
        let last = rows.with_untracked(Vec::len).saturating_sub(1);
        let row_at = |i: usize| rows.with_untracked(|r| r.get(i).cloned());
        let next = match e.key {
            Key::Named(NamedKey::ArrowDown) => at.map_or(0, |i| (i + 1).min(last)),
            Key::Named(NamedKey::ArrowUp) => at.map_or(0, |i| i.saturating_sub(1)),
            Key::Named(NamedKey::Home) => 0,
            Key::Named(NamedKey::End) => last,
            Key::Named(NamedKey::ArrowRight) => {
                e.prevent_default();
                let Some(row) = at.and_then(row_at) else {
                    return;
                };
                if !row.opens {
                    return;
                }
                if !open.with_untracked(|o| o.contains(&row.key)) {
                    flip(&row.key);
                    return;
                }
                at.map_or(0, |i| (i + 1).min(last))
            }
            Key::Named(NamedKey::ArrowLeft) => {
                e.prevent_default();
                let Some(row) = at.and_then(row_at) else {
                    return;
                };
                if row.opens && open.with_untracked(|o| o.contains(&row.key)) {
                    flip(&row.key);
                    return;
                }
                let Some(parent) = row.parent else {
                    return;
                };
                parent
            }
            Key::Named(NamedKey::Enter | NamedKey::Space) => {
                e.prevent_default();
                if let Some(row) = at.and_then(row_at).filter(|r| r.opens) {
                    flip(&row.key);
                }
                return;
            }
            _ => return,
        };
        e.prevent_default();
        if let Some(row) = row_at(next) {
            selected.set(Some(row.key));
            scroll_row_into_view(list, next, TREE_ROW);
        }
    })
}

type Compare<R> = Rc<dyn Fn(&R, &R) -> Ordering>;

/// A column of a [`table`].
pub struct Column<R> {
    title: String,
    width: f32,
    fill: bool,
    align_end: bool,
    cell: Rc<dyn Fn(&R) -> View>,
    compare: Option<Compare<R>>,
}

/// A column of text.
pub fn column<R: 'static>(
    title: impl Into<String>,
    value: impl Fn(&R) -> String + 'static,
) -> Column<R> {
    custom_column(title, move |row| {
        text(value(row)).whitespace_nowrap().into_view()
    })
}

/// A column of what `cell` builds.
pub fn custom_column<R: 'static>(
    title: impl Into<String>,
    cell: impl Fn(&R) -> View + 'static,
) -> Column<R> {
    Column {
        title: title.into(),
        width: 120.0,
        fill: false,
        align_end: false,
        cell: Rc::new(cell),
        compare: None,
    }
}

impl<R> Column<R> {
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Also takes what is left of the width of the table.
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }

    /// With what is in it at the right, as numbers go.
    pub fn align_end(mut self) -> Self {
        self.align_end = true;
        self
    }

    /// Pressing its title sorts the table with this.
    pub fn sort_by(mut self, compare: impl Fn(&R, &R) -> Ordering + 'static) -> Self {
        self.compare = Some(Rc::new(compare));
        self
    }
}

fn header_cell_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.text_color(t.text_muted))
        .hover(|s| s.bg(t.hover).text_color(t.text))
        .active(|s| s.bg(t.pressed))
}

fn handle_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.cursor(CursorIcon::EwResize))
        .hover(|s| s.bg(t.accent))
        .active(|s| s.bg(t.accent))
        .accent_background()
}

fn table_row_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.border_color(t.pressed).cursor(CursorIcon::Default)).hover(|s| s.bg(t.hover))
}

fn table_picked_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        s.bg(t.selected)
            .border_color(t.pressed)
            .cursor(CursorIcon::Default)
    })
}

const TABLE_ROW: f32 = 28.0;
const MIN_COLUMN: f32 = 32.0;

/// Which column it is sorted by, and which way.
type Sorting = Option<(usize, bool)>;

/// A table of `rows`, of which only the rows that show exist. `selected` is the index in `rows` of
/// the row that is selected. Pressing a title with a way to sort sorts by it, pressing it again the
/// other way, and then not; dragging the edge of a title changes the width of the column. The
/// arrow keys, PageUp, PageDown, Home and End go through the rows.
pub fn table<R: Clone + 'static>(
    rows: Signal<Vec<R>>,
    columns: Vec<Column<R>>,
    selected: Signal<Option<usize>>,
) -> Div {
    let columns = Rc::new(columns);
    let widths = signal(columns.iter().map(|c| c.width).collect::<Vec<_>>());
    let sorting = signal(None::<(usize, bool)>);
    let scroll_x = signal(0.0f32);
    let list = node_ref();

    // The rows in the order they show.
    let order = signal(Vec::<usize>::new());
    let version = signal(0u64);
    {
        let columns = columns.clone();
        effect(move || {
            let by: Sorting = sorting.get();
            let sorted = rows.with(|all| {
                let mut order: Vec<usize> = (0..all.len()).collect();
                if let Some((c, ascending)) = by
                    && let Some(compare) = columns.get(c).and_then(|c| c.compare.as_ref())
                {
                    // Stable, so that rows that compare the same keep their order.
                    order.sort_by(|a, b| {
                        let o = compare(&all[*a], &all[*b]);
                        if ascending { o } else { o.reverse() }
                    });
                }
                order
            });
            batch(|| {
                order.set(sorted);
                untrack(|| version.update(|v| *v += 1));
            });
        });
    }
    let all_wide = memo(move || widths.with(|w| w.iter().sum::<f32>()));

    let cell_frame = move |c: usize, fill: bool, align_end: bool| {
        div()
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .h_full()
            .px(8.0)
            .overflow_clip()
            .dyn_style(move |s| {
                let w = widths.with(|w| w.get(c).copied().unwrap_or(MIN_COLUMN));
                let s = if fill {
                    s.flex_grow(1.0).flex_basis(w).min_w(w)
                } else {
                    s.flex_none().w(w)
                };
                if align_end { s.justify_end() } else { s }
            })
    };

    let head_cols = columns.clone();
    let header = div()
        .flex_none()
        .h(TABLE_ROW + 2.0)
        .overflow_clip()
        .border_b(1.0)
        .font_weight(FontWeight::SEMIBOLD)
        .select_none()
        .dyn_style(|s| s.border_color(theme().border).bg(theme().surface_muted))
        .child(
            div()
                .flex()
                .flex_row()
                .h_full()
                .w_full()
                .dyn_style(move |s| s.min_w(all_wide.get()).ml(-scroll_x.get()))
                .children(
                    head_cols
                        .iter()
                        .enumerate()
                        .map(|(c, col)| {
                            let sorts = col.compare.is_some();
                            let drag_from = Cell::new(MIN_COLUMN);
                            let mut head = cell_frame(c, col.fill, col.align_end)
                                .gap(4.0)
                                .sheet(|| shared(header_cell_sheet))
                                .child(text(col.title.clone()).whitespace_nowrap())
                                .child(
                                    text(move || match sorting.get() {
                                        Some((at, true)) if at == c => "▲".to_owned(),
                                        Some((at, false)) if at == c => "▼".to_owned(),
                                        _ => String::new(),
                                    })
                                    .text_size(9.0),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .right(0.0)
                                        .top(0.0)
                                        .bottom(0.0)
                                        .w(5.0)
                                        .sheet(|| shared(handle_sheet))
                                        .on_click(|e| e.stop_propagation())
                                        .grab_threshold(0.0)
                                        .on_grab(move |g| match g.phase {
                                            GrabPhase::Start => drag_from.set(
                                                widths
                                                    .with_untracked(|w| w.get(c).copied())
                                                    .unwrap_or(MIN_COLUMN),
                                            ),
                                            GrabPhase::Move => widths.update(|all| {
                                                if let Some(slot) = all.get_mut(c) {
                                                    *slot = (drag_from.get() + g.total.x)
                                                        .max(MIN_COLUMN);
                                                }
                                            }),
                                            GrabPhase::End | GrabPhase::Cancel => {}
                                        }),
                                );
                            if sorts {
                                head = head.cursor(CursorIcon::Pointer).on_click(move |_| {
                                    sorting.update(|s| {
                                        *s = match *s {
                                            Some((at, true)) if at == c => Some((c, false)),
                                            Some((at, false)) if at == c => None,
                                            _ => Some((c, true)),
                                        }
                                    });
                                });
                            }
                            head
                        })
                        .collect::<Vec<_>>(),
                ),
        );

    let body_cols = columns;
    let body = rows_in(
        move || order.with(Vec::len),
        move || version.get(),
        TABLE_ROW,
        move |i| {
            let Some(index) = order.with_untracked(|o| o.get(i).copied()) else {
                return div();
            };
            let Some(row) = rows.with_untracked(|r| r.get(index).cloned()) else {
                return div();
            };
            let cells = body_cols
                .iter()
                .enumerate()
                .map(|(c, col)| cell_frame(c, col.fill, col.align_end).child((col.cell)(&row)))
                .collect::<Vec<_>>();
            div()
                .flex()
                .flex_row()
                .h_full()
                .w_full()
                .border_b(1.0)
                .sheet(move || {
                    shared(if selected.get() == Some(index) {
                        table_picked_sheet
                    } else {
                        table_row_sheet
                    })
                })
                .on_click(move |_| selected.set(Some(index)))
                .children(cells)
        },
        move |s| s.min_w(all_wide.get()),
        move |x| scroll_x.set_if_changed(x),
    )
    .overflow_scroll()
    .focusable()
    .node_ref(list)
    .on_key_down(move |e| {
        let count = order.with_untracked(Vec::len);
        if count == 0 {
            return;
        }
        let last = count - 1;
        let at = selected
            .get_untracked()
            .and_then(|index| order.with_untracked(|o| o.iter().position(|v| *v == index)));
        let page = list
            .rect()
            .map_or(10, |r| {
                let s = untrack(window_scale);
                (r.height / s.px(TABLE_ROW).max(1)).max(1) as usize
            })
            .saturating_sub(1)
            .max(1);
        let next = match e.key {
            Key::Named(NamedKey::ArrowDown) => at.map_or(0, |i| (i + 1).min(last)),
            Key::Named(NamedKey::ArrowUp) => at.map_or(0, |i| i.saturating_sub(1)),
            Key::Named(NamedKey::PageDown) => at.map_or(0, |i| (i + page).min(last)),
            Key::Named(NamedKey::PageUp) => at.map_or(0, |i| i.saturating_sub(page)),
            Key::Named(NamedKey::Home) => 0,
            Key::Named(NamedKey::End) => last,
            _ => return,
        };
        e.prevent_default();
        selected.set(order.with_untracked(|o| o.get(next).copied()));
        scroll_row_into_view(list, next, TABLE_ROW);
    });

    div()
        .flex()
        .flex_col()
        .min_h(0.0)
        .rounded(6.0)
        .border(1.0)
        .overflow_clip()
        .select_none()
        .sheet(|| shared(long_box_sheet))
        .child(header)
        .child(body)
}
