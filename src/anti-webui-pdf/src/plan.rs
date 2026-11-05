//! Where pages go and which of them to draw: nothing here touches a PDF or a thread.

use anti_webui_core::geometry::RectI;
use std::collections::HashMap;
use std::ops::Range;

/// How far pages are turned clockwise, in quarter turns. Text and places keep to the points of the
/// page as the document has it, so that they stay the same when this changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Turn(pub u8);

impl Turn {
    pub fn degrees(degrees: i32) -> Self {
        Self((degrees.div_euclid(90)).rem_euclid(4) as u8)
    }

    /// This much further, in quarter turns.
    pub fn by(self, quarters: i32) -> Self {
        Self((self.0 as i32 + quarters).rem_euclid(4) as u8)
    }

    /// How far it is turned from `other`, in quarter turns clockwise.
    fn since(self, other: Self) -> Self {
        self.by(-(other.0 as i32))
    }

    /// How big something this size looks.
    pub fn size<T>(self, (w, h): (T, T)) -> (T, T) {
        if self.0 % 2 == 1 { (h, w) } else { (w, h) }
    }

    /// Where a point of a page this size shows.
    pub fn point(self, (x, y): (f64, f64), (w, h): (f64, f64)) -> (f64, f64) {
        match self.0 {
            1 => (h - y, x),
            2 => (w - x, h - y),
            3 => (y, w - x),
            _ => (x, y),
        }
    }

    /// What of a page this size shows at a point.
    pub fn back(self, (x, y): (f64, f64), (w, h): (f64, f64)) -> (f64, f64) {
        match self.0 {
            1 => (y, h - x),
            2 => (w - x, h - y),
            3 => (w - y, x),
            _ => (x, y),
        }
    }

    /// What [`Self::point`] does as a matrix, in the order `kurbo` has it.
    pub fn matrix(self, (w, h): (f64, f64)) -> [f64; 6] {
        match self.0 {
            1 => [0.0, 1.0, -1.0, 0.0, h, 0.0],
            2 => [-1.0, 0.0, 0.0, -1.0, w, h],
            3 => [0.0, -1.0, 1.0, 0.0, 0.0, w],
            _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        }
    }
}

/// A picture to draw of a page, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Job {
    pub page: usize,
    /// How big all of the page is drawn, as it shows when turned.
    pub width: u32,
    pub height: u32,
    /// The part of that to make a picture of, or `None` for all of it.
    pub area: Option<RectI>,
    pub turn: Turn,
}

impl Job {
    /// How many pixels the picture has.
    pub fn size(&self) -> (u32, u32) {
        match self.area {
            Some(area) => (area.width as u32, area.height as u32),
            None => (self.width, self.height),
        }
    }

    pub fn bytes(&self) -> usize {
        let (width, height) = self.size();
        width as usize * height as usize * 4
    }

    /// Where the part it is of goes on a page that is `size` big now.
    pub fn area_at(&self, size: (i32, i32)) -> RectI {
        let area = self
            .area
            .unwrap_or(RectI::new(0, 0, self.width as i32, self.height as i32));
        let (kx, ky) = (
            size.0 as f64 / self.width as f64,
            size.1 as f64 / self.height as f64,
        );
        let (left, top) = (area.x as f64 * kx, area.y as f64 * ky);
        let (right, bottom) = (area.right() as f64 * kx, area.bottom() as f64 * ky);
        RectI::new(
            left.round() as i32,
            top.round() as i32,
            (right.round() - left.round()) as i32,
            (bottom.round() - top.round()) as i32,
        )
    }
}

/// What pixels of a page on white look like without the white, inside of `boxes` of the picture and
/// nothing outside of them, for when it shows on something else. It goes by how much the darkest of
/// the three colors takes away from white, which gets text back as it was drawn.
pub(crate) fn cut_out(pixels: &[u8], width: u32, boxes: &[RectI]) -> Vec<u8> {
    let width = width as usize;
    let mut out = vec![0; pixels.len()];
    for area in boxes {
        for y in area.y.max(0) as usize..area.bottom().max(0) as usize {
            let row = y * width * 4;
            let (from, to) = (
                row + area.x.max(0) as usize * 4,
                row + area.right().clamp(0, width as i32) as usize * 4,
            );
            let (Some(pixels), Some(out)) = (pixels.get(from..to), out.get_mut(from..to)) else {
                break;
            };
            for (from, to) in pixels.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
                let white = from[0].min(from[1]).min(from[2]);
                to.copy_from_slice(&[from[0] - white, from[1] - white, from[2] - white, !white]);
            }
        }
    }
    out
}

/// The most pixels in a picture of a whole page. A bigger page shows a stretched one.
const MAX_PICTURE_PIXELS: f64 = 16_000_000.0;
/// The longest side of a picture, which every GPU we run on can hold.
const MAX_PICTURE_SIDE: f64 = 8192.0;
/// How much more than what shows a picture of part of a page takes on each side, as a part of the
/// box. Less if that makes it too many pixels, which only a big screen would.
const PART_MARGIN: f64 = 0.5;

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

/// A whole picture doesn't do for this page: part of it gets one of its own.
pub(crate) fn needs_part(box_size: (i32, i32)) -> bool {
    picture_size(box_size) != (box_size.0.max(1) as u32, box_size.1.max(1) as u32)
}

/// Whether a picture of a whole page is as good as it gets. A stretched one can be a pixel off,
/// since the rounding changes with the zoom, which isn't worth drawing it again.
pub(crate) fn is_right_size(box_size: (i32, i32), have: (u32, u32)) -> bool {
    let want = picture_size(box_size);
    have == want
        || (needs_part(box_size) && have.0.abs_diff(want.0) <= 2 && have.1.abs_diff(want.1) <= 2)
}

/// How many times bigger or smaller a picture is than it should be, which is 1 or more.
pub(crate) fn size_is_off(have: (u32, u32), want: (u32, u32)) -> f64 {
    let ratio = have.0.max(1) as f64 / want.0.max(1) as f64;
    ratio.max(1.0 / ratio)
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
    pub turn: Turn,
}

impl PageTable {
    /// `pages` are in points, and `px_per_pt` says how many pixels each takes. `viewport_width`
    /// keeps narrow pages in the middle of it.
    pub fn new(pages: &[(f32, f32)], px_per_pt: f64, gap: i32, viewport_width: i32) -> Self {
        Self::turned(pages, Turn::default(), px_per_pt, gap, viewport_width)
    }

    /// The same with the pages turned, which `pages` aren't yet.
    pub fn turned(
        pages: &[(f32, f32)],
        turn: Turn,
        px_per_pt: f64,
        gap: i32,
        viewport_width: i32,
    ) -> Self {
        let mut tops = Vec::with_capacity(pages.len());
        let mut sizes = Vec::with_capacity(pages.len());
        let mut y = gap as i64;
        let mut widest = 0;
        for &page in pages {
            let (w, h) = turn.size(page);
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
            turn,
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

    /// What of a page shows in `view`, in the pixels of the page.
    pub fn shown(&self, page: usize, view: RectI) -> RectI {
        let (left, top) = (self.left(page), self.tops[page]);
        let (width, height) = self.sizes[page];
        RectI::new(left, top, width, height)
            .intersect(&view)
            .translate(-left, -top)
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

    /// The page at a row, or the next one if it is between pages.
    pub fn page_near(&self, y: i32) -> usize {
        self.first_below(y).min(self.len().saturating_sub(1))
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
        let (fx, fy) = other.turn.since(self.turn).point(
            (
                (x - self.left(page)) as f64 / w as f64,
                (y - self.tops[page]) as f64 / h as f64,
            ),
            (1.0, 1.0),
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

/// `rect` with `x` more on the left and right and `y` more above and below, as far as `size` goes.
fn grown(rect: RectI, x: i32, y: i32, size: (i32, i32)) -> RectI {
    let (left, top) = ((rect.x - x).max(0), (rect.y - y).max(0));
    RectI::new(
        left,
        top,
        (rect.right() + x).min(size.0) - left,
        (rect.bottom() + y).min(size.1) - top,
    )
}

/// How far past what shows a picture of part of a page goes, in pixels of a view that big.
fn margins(view: (i32, i32)) -> (i32, i32) {
    let (w, h) = (view.0.max(1) as f64, view.1.max(1) as f64);
    let most = MAX_PICTURE_PIXELS.max(w * h);
    let margin = PART_MARGIN.min(((most / (w * h)).sqrt() - 1.0) / 2.0);
    ((w * margin) as i32, (h * margin) as i32)
}

/// Whether `area` is a good enough picture of what shows: what shows and half of the margin is in
/// there, so that there is time to draw the next one.
fn area_will_do(area: RectI, shown: RectI, view: (i32, i32), size: (i32, i32)) -> bool {
    let (x, y) = margins(view);
    let needed = grown(shown, x / 2, y / 2, size);
    area.x <= needed.x
        && area.y <= needed.y
        && area.right() >= needed.right()
        && area.bottom() >= needed.bottom()
}

/// The part of a page to draw when `shown` shows of it. It goes no further than a texture can.
fn part_for(shown: RectI, view: (i32, i32), size: (i32, i32)) -> RectI {
    let (x, y) = margins(view);
    let mut area = grown(shown, x, y, size);
    area.width = area.width.min(MAX_PICTURE_SIDE as i32);
    area.height = area.height.min(MAX_PICTURE_SIDE as i32);
    area
}

/// Whether a picture of part of a page that has just come goes in place of the one there, if any:
/// not if the zoom has moved on, and not if less of what shows is in it.
pub(crate) fn takes_over(new: &Job, current: Option<&Job>, size: (i32, i32), shown: RectI) -> bool {
    let right = |job: &Job| (job.width as i32, job.height as i32) == size;
    let seen = |job: &Job| {
        let part = job.area_at(size).intersect(&shown);
        part.width.max(0) as i64 * part.height.max(0) as i64
    };
    right(new) && current.is_none_or(|current| !right(current) || seen(new) >= seen(current))
}

/// Whether a picture of part of a page is any use where the view is now.
pub(crate) fn part_is_of_use(table: &PageTable, job: &Job, view: RectI) -> bool {
    let size = table.size(job.page);
    needs_part(size) && job.area_at(size).intersects(&table.shown(job.page, view))
}

/// What there is to show of the pages.
pub(crate) struct Have<W, P, G> {
    /// The size of the picture of a whole page. One that can't be drawn counts as right.
    pub whole: W,
    /// The picture of part of a page.
    pub part: P,
    /// A picture of part of a page couldn't be drawn, so no more are asked for.
    pub gave_up: G,
}

/// What to draw, and what to hold on to.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Plan {
    /// The most urgent first.
    pub jobs: Vec<Job>,
    /// The pages that should have a picture, which fit in the budget unless what shows doesn't.
    pub keep: Vec<usize>,
}

/// What is wanted, leaving out what is there: the whole of what shows, the most of it first, then
/// the parts of what shows that are too big for that, and then what comes next as far as `budget`
/// bytes go. `goals` remembers the part each page is after, so that one being drawn isn't replaced
/// for every step of scrolling.
pub(crate) fn plan<W, P, G>(
    table: &PageTable,
    view: RectI,
    direction: Direction,
    budget: usize,
    have: Have<W, P, G>,
    goals: &mut HashMap<usize, Job>,
) -> Plan
where
    W: Fn(usize) -> Option<(u32, u32)>,
    P: Fn(usize) -> Option<Job>,
    G: Fn(usize) -> bool,
{
    if table.is_empty() || view.height <= 0 {
        goals.clear();
        return Plan::default();
    }
    let visible = table.visible(view.y, view.height);
    goals.retain(|page, _| visible.contains(page));
    let shown_area = |page: usize| {
        let shown = table.shown(page, view);
        shown.width.max(0) as i64 * shown.height.max(0) as i64
    };
    let mut pages: Vec<usize> = visible.clone().collect();
    pages.sort_by_key(|&page| std::cmp::Reverse(shown_area(page)));

    let whole = |page: usize| {
        let (width, height) = picture_size(table.size(page));
        Job {
            page,
            width,
            height,
            area: None,
            turn: table.turn,
        }
    };
    let wanted = |job: &Job| {
        (have.whole)(job.page).is_none_or(|size| !is_right_size(table.size(job.page), size))
    };

    let mut jobs: Vec<Job> = pages
        .iter()
        .map(|&page| whole(page))
        .filter(wanted)
        .collect();
    let mut used: usize = pages.iter().map(|&page| whole(page).bytes()).sum();

    let view_size = (view.width, view.height);
    for &page in &pages {
        let size = table.size(page);
        let shown = table.shown(page, view);
        if !needs_part(size) || (have.gave_up)(page) || shown.is_empty() {
            goals.remove(&page);
            continue;
        }
        let good = |job: &Job| {
            (job.width as i32, job.height as i32) == size
                && job.turn == table.turn
                && job
                    .area
                    .is_some_and(|area| area_will_do(area, shown, view_size, size))
        };
        let there = (have.part)(page);
        let goal = there
            .filter(good)
            .or_else(|| goals.get(&page).copied().filter(good))
            .unwrap_or(Job {
                page,
                width: size.0 as u32,
                height: size.1 as u32,
                area: Some(part_for(shown, view_size, size)),
                turn: table.turn,
            });
        goals.insert(page, goal);
        used += goal.bytes();
        if there != Some(goal) {
            jobs.push(goal);
        }
    }

    let before = (0..visible.start).rev().take(match direction {
        Direction::Down => BEHIND,
        Direction::Up => AHEAD,
    });
    let after = (visible.end..table.len()).take(match direction {
        Direction::Down => AHEAD,
        Direction::Up => BEHIND,
    });
    let next: Vec<usize> = match direction {
        Direction::Down => after.chain(before).collect(),
        Direction::Up => before.chain(after).collect(),
    };
    for page in next {
        let job = whole(page);
        used += job.bytes();
        // Drawing what would have to go again right away would never end.
        if used > budget {
            break;
        }
        pages.push(page);
        if wanted(&job) {
            jobs.push(job);
        }
    }
    Plan { jobs, keep: pages }
}

/// A picture of a whole page, as far as dropping it goes.
pub(crate) struct Held {
    pub page: usize,
    pub bytes: usize,
    pub right_size: bool,
}

/// Which of the pictures to drop to stay within `budget` bytes: those that aren't wanted, the
/// furthest from `center` first, and then those at a size that is about to be replaced. What shows
/// stays, or there would be nothing to stretch, and so does what is wanted, or it would be drawn
/// again.
pub(crate) fn evict(
    pictures: impl IntoIterator<Item = Held>,
    visible: &Range<usize>,
    keep: &[usize],
    center: usize,
    budget: usize,
) -> Vec<usize> {
    let mut total = 0;
    let mut droppable = Vec::new();
    for picture in pictures {
        total += picture.bytes;
        let wanted = keep.contains(&picture.page);
        if !visible.contains(&picture.page) && (!wanted || !picture.right_size) {
            droppable.push((wanted, picture));
        }
    }
    droppable.sort_by_key(|(wanted, p)| (*wanted, std::cmp::Reverse(p.page.abs_diff(center))));
    let mut dropped = Vec::new();
    for (_, picture) in droppable {
        if total <= budget {
            break;
        }
        total -= picture.bytes;
        dropped.push(picture.page);
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
    fn turning_a_page_keeps_what_is_where_it_was_on_it() {
        let size = (100.0, 200.0);
        let point = (10.0, 30.0);
        // A quarter turn clockwise takes the top left to the top right.
        assert_eq!(Turn(1).point((0.0, 0.0), size), (200.0, 0.0));
        assert_eq!(Turn(2).point((0.0, 0.0), size), (100.0, 200.0));
        assert_eq!(Turn(3).point((0.0, 0.0), size), (0.0, 100.0));
        for turn in (0..4).map(Turn) {
            assert_eq!(turn.back(turn.point(point, size), size), point, "{turn:?}");
            let [a, b, c, d, e, f] = turn.matrix(size);
            let by_matrix = (a * point.0 + c * point.1 + e, b * point.0 + d * point.1 + f);
            assert_eq!(by_matrix, turn.point(point, size), "{turn:?}");
        }
        assert_eq!(Turn(1).size((100, 200)), (200, 100));
        assert_eq!(Turn(2).size((100, 200)), (100, 200));
        assert_eq!(Turn::degrees(-90), Turn(3));
        assert_eq!(Turn::degrees(450), Turn(1));
        assert_eq!(Turn(3).by(1), Turn(0));
    }

    #[test]
    fn turning_keeps_the_same_place_on_the_same_page() {
        let pages = [(100.0, 200.0), (100.0, 200.0)];
        let upright = PageTable::new(&pages, 1.0, 10, 500);
        let turned = PageTable::turned(&pages, Turn(1), 1.0, 10, 500);
        assert_eq!(turned.size(1), (200, 100));
        // A tenth across and a quarter down the second page goes a tenth down from the right.
        let (x, y) = (upright.left(1) + 10, upright.top(1) + 50);
        let (a, b) = upright.move_point(&turned, x, y);
        assert_eq!((a - turned.left(1), b - turned.top(1)), (150, 10));
        assert_eq!(turned.move_point(&upright, a, b), (x, y));
    }

    #[test]
    fn what_is_white_goes_when_cut_out() {
        let box_of_all = [RectI::new(0, 0, 3, 1)];
        // White, black, half black and a color on white.
        let pixels = [
            255, 255, 255, 255, 0, 0, 0, 255, 128, 128, 128, 255, 255, 100, 100, 255,
        ];
        let cut = cut_out(&pixels[..12], 3, &box_of_all);
        assert_eq!(cut, [0, 0, 0, 0, 0, 0, 0, 255, 0, 0, 0, 127]);
        let cut = cut_out(&pixels[12..], 1, &[RectI::new(0, 0, 1, 1)]);
        assert_eq!(cut, [155, 0, 0, 155]);
    }

    #[test]
    fn only_what_is_in_the_boxes_is_cut_out() {
        let black = [0, 0, 0, 255].repeat(4);
        let cut = cut_out(&black, 2, &[RectI::new(1, 0, 1, 1), RectI::new(0, 1, 5, 5)]);
        assert_eq!(cut[..4], [0, 0, 0, 0]);
        assert_eq!(cut[4..], [0, 0, 0, 255].repeat(3)[..]);
    }

    const LOTS: usize = 1 << 40;

    type NoPictures =
        Have<fn(usize) -> Option<(u32, u32)>, fn(usize) -> Option<Job>, fn(usize) -> bool>;

    fn nothing() -> NoPictures {
        Have {
            whole: |_| None,
            part: |_| None,
            gave_up: |_| false,
        }
    }

    fn view(top: i32, height: i32) -> RectI {
        RectI::new(0, top, 200, height)
    }

    fn pages_of(plan: &Plan) -> Vec<usize> {
        plan.jobs.iter().map(|j| j.page).collect()
    }

    #[test]
    fn what_shows_comes_first_then_where_the_view_is_going() {
        let t = table(10);
        let plan = |top, direction| {
            pages_of(&plan(
                &t,
                view(top, 150),
                direction,
                LOTS,
                nothing(),
                &mut HashMap::new(),
            ))
        };
        // From 300 to 450 it shows 100 rows of page 3, which starts at 340, and 30 of page 2.
        assert_eq!(plan(300, Direction::Down), [3, 2, 4, 5, 1]);
        assert_eq!(plan(300, Direction::Up), [3, 2, 1, 0, 4]);
    }

    #[test]
    fn a_picture_at_the_right_size_is_not_drawn_again() {
        let t = table(3);
        let jobs = |size: fn(usize) -> Option<(u32, u32)>| {
            pages_of(&plan(
                &t,
                view(0, 50),
                Direction::Down,
                LOTS,
                Have {
                    whole: size,
                    part: |_| None,
                    gave_up: |_| false,
                },
                &mut HashMap::new(),
            ))
        };
        assert_eq!(jobs(|page| (page == 0).then_some((100, 100)))[0], 1);
        // Any other size is a reason to draw it.
        assert_eq!(jobs(|_| Some((50, 50)))[0], 0);
    }

    #[test]
    fn nothing_is_wanted_of_a_viewport_with_no_height() {
        let plan_of = |t: &PageTable, height| {
            plan(
                t,
                view(0, height),
                Direction::Down,
                LOTS,
                nothing(),
                &mut HashMap::new(),
            )
        };
        assert_eq!(plan_of(&table(3), 0), Plan::default());
        assert_eq!(plan_of(&PageTable::default(), 100), Plan::default());
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
    fn a_stretched_picture_a_pixel_off_is_not_drawn_again() {
        let (w, h) = picture_size((11424, 16166));
        assert!(is_right_size((11424, 16166), (w + 1, h - 1)));
        assert!(!is_right_size((11424, 16166), (w / 2, h / 2)));
        // One that isn't stretched has to meet the screen exactly.
        assert!(!is_right_size((1429, 2021), (1430, 2021)));
    }

    /// A page much bigger than a picture of all of it can be, of which 2000 by 1500 show.
    fn big() -> PageTable {
        PageTable::new(&[(10000.0, 20000.0), (10000.0, 20000.0)], 1.0, 0, 2000)
    }

    const BIG_VIEW: RectI = RectI::new(4000, 8000, 2000, 1500);

    #[test]
    fn a_page_too_big_for_one_picture_gets_one_of_what_shows_and_around_it() {
        let t = big();
        let mut goals = HashMap::new();
        let plan = plan(&t, BIG_VIEW, Direction::Down, LOTS, nothing(), &mut goals);
        // The stretched one comes first, then the part, with half of the view around what shows.
        assert_eq!(plan.jobs[0].area, None);
        assert_eq!(plan.jobs[1].area, Some(RectI::new(3000, 7250, 4000, 3000)));
        assert_eq!((plan.jobs[1].width, plan.jobs[1].height), (10000, 20000));
        assert_eq!(goals[&0], plan.jobs[1]);
    }

    #[test]
    fn a_page_that_fits_gets_no_part() {
        let t = table(1);
        let plan = plan(
            &t,
            view(0, 50),
            Direction::Down,
            LOTS,
            nothing(),
            &mut HashMap::new(),
        );
        assert!(plan.jobs.iter().all(|j| j.area.is_none()));
    }

    #[test]
    fn a_big_screen_gets_less_around_it_rather_than_too_many_pixels() {
        let (x, y) = margins((3840, 2160));
        let (w, h) = ((3840 + 2 * x) as f64, (2160 + 2 * y) as f64);
        assert!(w * h <= 16_100_000.0, "{w} by {h}");
        assert!(x > 0 && y > 0);
        // One bigger than that much still has what shows.
        assert_eq!(margins((8000, 4000)), (0, 0));
    }

    #[test]
    fn a_part_is_asked_for_once_while_scrolling_stays_within_its_margin() {
        let t = big();
        let mut goals = HashMap::new();
        let first = plan(&t, BIG_VIEW, Direction::Down, LOTS, nothing(), &mut goals).jobs[1];
        // A quarter of the view is half of the margin, which is still enough.
        let near = BIG_VIEW.translate(0, 375);
        let again = plan(&t, near, Direction::Down, LOTS, nothing(), &mut goals);
        assert_eq!(again.jobs[1], first);
        // Past that it needs another, centered on what shows then.
        let far = BIG_VIEW.translate(0, 376);
        let next = plan(&t, far, Direction::Down, LOTS, nothing(), &mut goals);
        assert_eq!(next.jobs[1].area, Some(RectI::new(3000, 7626, 4000, 3000)));
    }

    #[test]
    fn a_part_that_is_there_and_enough_is_not_drawn_again() {
        let t = big();
        let mut goals = HashMap::new();
        let part = plan(&t, BIG_VIEW, Direction::Down, LOTS, nothing(), &mut goals).jobs[1];
        let have = Have {
            whole: |_| Some(picture_size((10000, 20000))),
            part: |page| (page == 0).then_some(part),
            gave_up: |_| false,
        };
        let plan = plan(&t, BIG_VIEW, Direction::Down, LOTS, have, &mut goals);
        assert!(plan.jobs.iter().all(|j| j.page != 0), "{:?}", plan.jobs);
    }

    #[test]
    fn a_part_is_not_asked_for_again_after_it_failed() {
        let t = big();
        let have = Have {
            whole: |_| None,
            part: |_| None,
            gave_up: |page| page == 0,
        };
        let plan = plan(
            &t,
            BIG_VIEW,
            Direction::Down,
            LOTS,
            have,
            &mut HashMap::new(),
        );
        assert!(plan.jobs.iter().all(|j| j.area.is_none()));
    }

    #[test]
    fn a_part_for_a_page_going_out_of_view_is_forgotten() {
        let t = big();
        let mut goals = HashMap::new();
        plan(&t, BIG_VIEW, Direction::Down, LOTS, nothing(), &mut goals);
        assert!(goals.contains_key(&0));
        let lower = RectI::new(4000, 25000, 2000, 1500);
        plan(&t, lower, Direction::Down, LOTS, nothing(), &mut goals);
        assert!(!goals.contains_key(&0));
        assert!(goals.contains_key(&1));
    }

    #[test]
    fn a_part_goes_once_nothing_of_it_shows_or_it_is_not_needed() {
        let t = big();
        let job = plan(
            &t,
            BIG_VIEW,
            Direction::Down,
            LOTS,
            nothing(),
            &mut HashMap::new(),
        )
        .jobs[1];
        assert!(part_is_of_use(&t, &job, BIG_VIEW));
        assert!(!part_is_of_use(&t, &job, BIG_VIEW.translate(0, 5000)));
        assert!(!part_is_of_use(
            &t,
            &job,
            RectI::new(4000, 25000, 2000, 1500)
        ));
        // Everything fits in one picture at this zoom.
        let small = PageTable::new(&[(1000.0, 2000.0), (1000.0, 2000.0)], 1.0, 0, 2000);
        assert!(!part_is_of_use(&small, &job, BIG_VIEW));
    }

    fn part(area: RectI) -> Job {
        Job {
            page: 0,
            width: 1000,
            height: 1000,
            area: Some(area),
            turn: Turn::default(),
        }
    }

    #[test]
    fn a_part_keeps_its_place_on_the_page_at_another_zoom() {
        let job = part(RectI::new(100, 200, 300, 400));
        assert_eq!(job.area_at((2000, 2000)), RectI::new(200, 400, 600, 800));
        assert_eq!(job.area_at((1000, 1000)), RectI::new(100, 200, 300, 400));
    }

    #[test]
    fn a_part_that_came_goes_in_unless_it_is_late() {
        let size = (1000, 1000);
        let shown = RectI::new(100, 100, 100, 100);
        let good = part(RectI::new(0, 0, 300, 300));
        let off = part(RectI::new(150, 150, 300, 300));
        assert!(takes_over(&good, None, size, shown));
        assert!(takes_over(&good, Some(&off), size, shown));
        // What is there has more of what shows.
        assert!(!takes_over(&off, Some(&good), size, shown));
        // The zoom has changed while it was being drawn.
        assert!(!takes_over(&good, None, (2000, 2000), shown));
        // One at the old zoom gives way to one at the new.
        let old = Job {
            width: 500,
            height: 500,
            ..good
        };
        assert!(takes_over(&off, Some(&old), size, shown));
    }

    fn held(page: usize, right_size: bool) -> Held {
        Held {
            page,
            bytes: 10,
            right_size,
        }
    }

    #[test]
    fn eviction_goes_from_the_furthest_and_spares_what_shows() {
        let pictures = || [0, 1, 2, 3, 9].map(|page| held(page, true));
        let drop = |visible: Range<usize>, budget| evict(pictures(), &visible, &[], 2, budget);
        assert_eq!(drop(2..4, 50), Vec::<usize>::new());
        assert_eq!(drop(2..4, 30), [9, 0]);
        // What shows stays, even over the budget.
        assert_eq!(drop(0..10, 0), Vec::<usize>::new());
    }

    #[test]
    fn what_is_wanted_stays_unless_it_is_at_the_wrong_size() {
        // Page 9 is furthest, but it is where the view is going.
        let pictures = [held(2, true), held(0, true), held(9, true)];
        assert_eq!(evict(pictures, &(2..3), &[2, 9], 2, 20), [0]);
        let pictures = [held(2, true), held(0, true), held(9, true)];
        assert_eq!(
            evict(pictures, &(2..3), &[2, 0, 9], 2, 0),
            Vec::<usize>::new()
        );
        // What was drawn before the zoom changed goes after what isn't wanted at all.
        let pictures = [held(2, true), held(3, false), held(9, true)];
        assert_eq!(evict(pictures, &(2..3), &[2, 3], 2, 20), [9]);
        let pictures = [held(2, true), held(3, false), held(9, true)];
        assert_eq!(evict(pictures, &(2..3), &[2, 3], 2, 10), [9, 3]);
    }

    /// As the viewer does it: what is drawn goes in and what is over the budget goes out, until
    /// nothing more is wanted. This must end with whatever budget it has.
    fn run_to_the_end(budget: usize) -> usize {
        let t = table(10);
        let mut there = HashMap::<usize, usize>::new();
        let mut goals = HashMap::new();
        let v = view(300, 150);
        let (visible, center) = (t.visible(300, 150), t.page_at_middle(300, 150));
        for rounds in 0.. {
            let plan = plan(
                &t,
                v,
                Direction::Down,
                budget,
                Have {
                    whole: |page| there.get(&page).map(|_| (100, 100)),
                    part: |_| None,
                    gave_up: |_| false,
                },
                &mut goals,
            );
            let Some(job) = plan.jobs.first() else {
                return rounds;
            };
            there.insert(job.page, job.bytes());
            let held = there.iter().map(|(&page, &bytes)| Held {
                page,
                bytes,
                right_size: true,
            });
            for page in evict(held, &visible, &plan.keep, center, budget) {
                there.remove(&page);
            }
            assert!(rounds < 100, "it never ends with a budget of {budget}");
        }
        unreachable!()
    }

    #[test]
    fn a_small_budget_does_not_draw_the_same_page_forever() {
        // Each picture takes 40000 bytes, and two show.
        for pages in 0..8 {
            let drawn = run_to_the_end(pages * 40_000);
            assert!((2..=5).contains(&drawn), "{pages}: {drawn}");
        }
        assert_eq!(run_to_the_end(LOTS), 5);
        assert_eq!(run_to_the_end(0), 2);
        assert_eq!(run_to_the_end(3 * 40_000), 3);
    }
}
