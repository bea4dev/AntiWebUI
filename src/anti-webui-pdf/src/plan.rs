//! Where pages go and which of them to draw: nothing here touches a PDF or a thread.

use std::ops::Range;

/// A picture to draw of a page, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Job {
    pub page: usize,
    pub width: u32,
    pub height: u32,
}

/// The most pixels in a picture of a whole page. A bigger page shows a stretched one.
const MAX_PICTURE_PIXELS: f64 = 16_000_000.0;
/// The longest side of a picture, which every GPU we run on can hold.
const MAX_PICTURE_SIDE: f64 = 8192.0;

/// How many pixels a page that takes `box_size` gets, which is the same unless that is too many.
pub(crate) fn picture_size(box_size: (i32, i32)) -> (u32, u32) {
    let (w, h) = (box_size.0.max(1) as f64, box_size.1.max(1) as f64);
    let shrink = (MAX_PICTURE_PIXELS / (w * h))
        .sqrt()
        .min(MAX_PICTURE_SIDE / w)
        .min(MAX_PICTURE_SIDE / h)
        .min(1.0);
    (
        ((w * shrink).round() as u32).max(1),
        ((h * shrink).round() as u32).max(1),
    )
}

/// Where the pages of a document go at some zoom, in physical pixels.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PageTable {
    /// Where each page starts.
    tops: Vec<i32>,
    sizes: Vec<(i32, i32)>,
    /// All of it, with the gaps around the pages.
    pub width: i32,
    pub height: i32,
}

impl PageTable {
    /// `pages` are in points, and `px_per_pt` says how many pixels each takes. `viewport_width`
    /// keeps narrow pages in the middle of it.
    pub fn new(pages: &[(f32, f32)], px_per_pt: f64, gap: i32, viewport_width: i32) -> Self {
        let mut tops = Vec::with_capacity(pages.len());
        let mut sizes = Vec::with_capacity(pages.len());
        let mut y = gap as i64;
        let mut widest = 0;
        for &(w, h) in pages {
            let size = (
                ((w as f64 * px_per_pt).round() as i32).max(1),
                ((h as f64 * px_per_pt).round() as i32).max(1),
            );
            tops.push(y.min(i32::MAX as i64) as i32);
            sizes.push(size);
            widest = widest.max(size.0);
            y += size.1 as i64 + gap as i64;
        }
        Self {
            tops,
            sizes,
            width: (widest + 2 * gap).max(viewport_width),
            height: y.min(i32::MAX as i64) as i32,
        }
    }

    pub fn len(&self) -> usize {
        self.tops.len()
    }

    pub fn top(&self, page: usize) -> i32 {
        self.tops[page]
    }

    pub fn left(&self, page: usize) -> i32 {
        (self.width - self.sizes[page].0) / 2
    }

    pub fn size(&self, page: usize) -> (i32, i32) {
        self.sizes[page]
    }

    /// The first page that reaches below `y`.
    fn first_below(&self, y: i32) -> usize {
        let mut low = 0;
        let mut high = self.tops.len();
        while low < high {
            let mid = (low + high) / 2;
            if self.tops[mid] as i64 + self.sizes[mid].1 as i64 <= y as i64 {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        low
    }

    /// The pages that touch the rows from `top`, `height` of them.
    pub fn visible(&self, top: i32, height: i32) -> Range<usize> {
        let first = self.first_below(top);
        let bottom = top as i64 + height as i64;
        let mut last = first;
        while last < self.tops.len() && (self.tops[last] as i64) < bottom {
            last += 1;
        }
        first..last
    }

    /// The page in the middle of the rows from `top`, `height` of them.
    pub fn page_at_middle(&self, top: i32, height: i32) -> usize {
        let middle = (top as i64 + height as i64 / 2).min(i32::MAX as i64) as i32;
        self.first_below(middle).min(self.len().saturating_sub(1))
    }

    /// Where the same place on the same page is in `other`, for a point in this table.
    pub fn move_point(&self, other: &PageTable, x: i32, y: i32) -> (i32, i32) {
        if self.is_empty() || other.len() != self.len() {
            return (x, y);
        }
        let page = self.first_below(y).min(self.len() - 1);
        let (w, h) = self.sizes[page];
        let (fx, fy) = (
            (x - self.left(page)) as f64 / w as f64,
            (y - self.tops[page]) as f64 / h as f64,
        );
        let (w, h) = other.sizes[page];
        (
            other.left(page) + (fx * w as f64).round() as i32,
            other.tops[page] + (fy * h as f64).round() as i32,
        )
    }

    pub fn is_empty(&self) -> bool {
        self.tops.is_empty()
    }
}

/// Which way the view last moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    Down,
    Up,
}

/// How many pages are drawn ahead, and behind, of what shows.
const AHEAD: usize = 2;
const BEHIND: usize = 1;

/// The pictures that are wanted, most urgent first, leaving out those that are there: what shows,
/// the most of it first, and then what comes next. `have` says what size a page has.
pub(crate) fn plan(
    table: &PageTable,
    top: i32,
    height: i32,
    direction: Direction,
    have: impl Fn(usize) -> Option<(u32, u32)>,
) -> Vec<Job> {
    if table.is_empty() || height <= 0 {
        return Vec::new();
    }
    let visible = table.visible(top, height);
    let shown = |page: usize| {
        let start = table.top(page) as i64;
        let end = start + table.size(page).1 as i64;
        (end.min(top as i64 + height as i64) - start.max(top as i64)).max(0)
    };
    let mut pages: Vec<usize> = visible.clone().collect();
    pages.sort_by_key(|&page| std::cmp::Reverse(shown(page)));
    let before = (0..visible.start).rev().take(match direction {
        Direction::Down => BEHIND,
        Direction::Up => AHEAD,
    });
    let after = (visible.end..table.len()).take(match direction {
        Direction::Down => AHEAD,
        Direction::Up => BEHIND,
    });
    match direction {
        Direction::Down => pages.extend(after.chain(before)),
        Direction::Up => pages.extend(before.chain(after)),
    }
    pages
        .into_iter()
        .filter_map(|page| {
            let (width, height) = picture_size(table.size(page));
            (have(page) != Some((width, height))).then_some(Job {
                page,
                width,
                height,
            })
        })
        .collect()
}

/// Which of the pictures to drop to stay within `budget` bytes: the furthest from `center` first,
/// and never those in `keep`.
pub(crate) fn evict(
    pictures: impl IntoIterator<Item = (usize, usize)>,
    keep: &Range<usize>,
    center: usize,
    budget: usize,
) -> Vec<usize> {
    let mut pictures: Vec<(usize, usize)> = pictures.into_iter().collect();
    let mut total: usize = pictures.iter().map(|p| p.1).sum();
    pictures.sort_by_key(|p| std::cmp::Reverse(p.0.abs_diff(center)));
    let mut dropped = Vec::new();
    for (page, bytes) in pictures {
        if total <= budget {
            break;
        }
        if !keep.contains(&page) {
            total -= bytes;
            dropped.push(page);
        }
    }
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(count: usize) -> PageTable {
        // Each takes 100 rows, with 10 before it: pages start at 10, 120, 230 and so on.
        PageTable::new(&vec![(100.0, 100.0); count], 1.0, 10, 200)
    }

    #[test]
    fn pages_go_one_below_the_other_in_the_middle() {
        let t = PageTable::new(&[(100.0, 200.0), (50.0, 100.0)], 2.0, 8, 0);
        assert_eq!(t.top(0), 8);
        assert_eq!(t.top(1), 8 + 400 + 8);
        assert_eq!(t.size(1), (100, 200));
        assert_eq!(t.width, 216);
        assert_eq!(t.left(0), 8);
        assert_eq!(t.left(1), 58);
        assert_eq!(t.height, 8 + 400 + 8 + 200 + 8);
    }

    #[test]
    fn a_wide_viewport_keeps_pages_in_its_middle() {
        let t = PageTable::new(&[(100.0, 100.0)], 1.0, 8, 500);
        assert_eq!(t.width, 500);
        assert_eq!(t.left(0), 200);
    }

    #[test]
    fn only_what_touches_the_rows_is_visible() {
        let t = table(10);
        assert_eq!(t.visible(0, 10), 0..0);
        assert_eq!(t.visible(0, 11), 0..1);
        assert_eq!(t.visible(109, 2), 0..1);
        assert_eq!(t.visible(110, 10), 1..1);
        assert_eq!(t.visible(100, 200), 0..3);
        assert_eq!(t.visible(100_000, 100), 10..10);
    }

    #[test]
    fn the_page_in_the_middle_is_the_one_that_is_reading() {
        let t = table(10);
        assert_eq!(t.page_at_middle(0, 100), 0);
        assert_eq!(t.page_at_middle(120, 100), 1);
        assert_eq!(t.page_at_middle(100_000, 100), 9);
    }

    #[test]
    fn a_place_on_a_page_stays_the_same_place_at_another_zoom() {
        let small = table(10);
        let big = PageTable::new(&[(100.0, 100.0); 10], 3.0, 10, 200);
        // A quarter into the page below the first, in both directions.
        let (x, y) = small.move_point(&big, 75, 145);
        assert_eq!(y, big.top(1) + 75);
        assert_eq!(x, big.left(1) + 75);
        assert_eq!(big.move_point(&small, x, y), (75, 145));
    }

    #[test]
    fn what_shows_comes_first_then_where_the_view_is_going() {
        let t = table(10);
        let plan = |top, direction| {
            plan(&t, top, 150, direction, |_| None)
                .iter()
                .map(|j| j.page)
                .collect::<Vec<_>>()
        };
        // From 300 to 450 it shows 100 rows of page 3, which starts at 340, and 30 of page 2.
        assert_eq!(plan(300, Direction::Down), [3, 2, 4, 5, 1]);
        assert_eq!(plan(300, Direction::Up), [3, 2, 1, 0, 4]);
    }

    #[test]
    fn a_picture_at_the_right_size_is_not_drawn_again() {
        let t = table(3);
        let jobs = plan(&t, 0, 50, Direction::Down, |page| {
            (page == 0).then_some((100, 100))
        });
        assert_eq!(jobs[0].page, 1);
        // Any other size is a reason to draw it.
        let jobs = plan(&t, 0, 50, Direction::Down, |_| Some((50, 50)));
        assert_eq!(jobs[0].page, 0);
    }

    #[test]
    fn nothing_is_wanted_of_a_viewport_with_no_height() {
        assert!(plan(&table(3), 0, 0, Direction::Down, |_| None).is_empty());
        assert!(plan(&PageTable::default(), 0, 100, Direction::Down, |_| None).is_empty());
    }

    #[test]
    fn very_big_pages_get_a_smaller_picture() {
        assert_eq!(picture_size((1429, 2021)), (1429, 2021));
        let (w, h) = picture_size((11424, 16166));
        assert!(w as u64 * h as u64 <= 16_000_000 + 10_000);
        assert!((w as f64 / h as f64 - 11424.0 / 16166.0).abs() < 0.01);
        assert_eq!(picture_size((20000, 10)).0, 8192);
    }

    #[test]
    fn eviction_goes_from_the_furthest_and_spares_what_shows() {
        let pictures = [(0, 10), (1, 10), (2, 10), (3, 10), (9, 10)];
        assert_eq!(evict(pictures, &(2..4), 2, 50), Vec::<usize>::new());
        assert_eq!(evict(pictures, &(2..4), 2, 30), [9, 0]);
        // What shows stays, even over the budget.
        assert_eq!(evict(pictures, &(0..10), 2, 0), Vec::<usize>::new());
    }
}
