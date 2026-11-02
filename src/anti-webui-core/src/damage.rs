//! Damage tracking: which pixels differ between two frames (see `docs/design/05-rendering.md`).

use crate::geometry::RectI;
use crate::scene::{ChunkKey, GlyphRun, Primitive, Scene};
use parley::FontData;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

/// More rectangles than this are merged; each one costs a scissored pass on the GPU.
pub const MAX_RECTS: usize = 8;

/// A set of changed rectangles in physical pixels. Rectangles may overlap.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Damage {
    rects: Vec<RectI>,
}

impl Damage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn full(width: u32, height: u32) -> Self {
        let mut d = Self::new();
        d.add(RectI::new(0, 0, width as i32, height as i32));
        d
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    pub fn rects(&self) -> &[RectI] {
        &self.rects
    }

    /// The bounding box of all rectangles.
    pub fn bounds(&self) -> RectI {
        self.rects
            .iter()
            .fold(RectI::default(), |acc, r| acc.union(r))
    }

    /// Sum of the rectangle areas (overlaps counted twice).
    pub fn area(&self) -> i64 {
        self.rects.iter().map(RectI::area).sum()
    }

    pub fn add(&mut self, rect: RectI) {
        if rect.is_empty() {
            return;
        }
        let mut rect = rect;
        // Absorb rectangles that merge without much wasted area; the result may now merge with
        // others, so repeat.
        while let Some(i) = self.rects.iter().position(|r| cheap_to_merge(r, &rect)) {
            rect = self.rects.swap_remove(i).union(&rect);
        }
        self.rects.push(rect);
        while self.rects.len() > MAX_RECTS {
            self.merge_cheapest_pair();
        }
    }

    pub fn extend(&mut self, other: &Damage) {
        for r in &other.rects {
            self.add(*r);
        }
    }

    /// Restricts the damage to `bounds`.
    pub fn clip(&mut self, bounds: RectI) {
        for r in &mut self.rects {
            *r = r.intersect(&bounds);
        }
        self.rects.retain(|r| !r.is_empty());
    }

    fn merge_cheapest_pair(&mut self) {
        let mut best = (i64::MAX, 0, 1);
        for i in 0..self.rects.len() {
            for j in i + 1..self.rects.len() {
                let w = waste(&self.rects[i], &self.rects[j]);
                if w < best.0 {
                    best = (w, i, j);
                }
            }
        }
        let (_, i, j) = best;
        let b = self.rects.swap_remove(j);
        self.rects[i] = self.rects[i].union(&b);
    }
}

/// Area the union covers beyond the two rectangles.
fn waste(a: &RectI, b: &RectI) -> i64 {
    a.union(b).area() - (a.area() + b.area() - a.intersect(b).area())
}

fn cheap_to_merge(a: &RectI, b: &RectI) -> bool {
    waste(a, b) * 4 <= a.area() + b.area()
}

/// Damage of recent frames, for repainting buffers that are several frames old.
#[derive(Debug, Default)]
pub struct DamageHistory {
    frames: VecDeque<Damage>,
}

impl DamageHistory {
    const LEN: usize = 8;

    pub fn push(&mut self, damage: Damage) {
        if self.frames.len() == Self::LEN {
            self.frames.pop_front();
        }
        self.frames.push_back(damage);
    }

    /// The union of the last `frames` pushed damages, or `None` if they are not all known.
    pub fn last(&self, frames: usize) -> Option<Damage> {
        if frames == 0 || frames > self.frames.len() {
            return None;
        }
        let mut d = Damage::new();
        for f in self.frames.iter().rev().take(frames) {
            d.extend(f);
        }
        Some(d)
    }

    pub fn clear(&mut self) {
        self.frames.clear();
    }
}

/// The pixels that differ between `old` and `new`.
///
/// Chunks are matched by key. A pixel outside the result is covered only by primitives that
/// are identical and in the same relative order in both scenes, so it renders identically.
pub fn diff(old: &Scene, new: &Scene) -> Damage {
    if old.width != new.width || old.height != new.height || old.clear != new.clear {
        return Damage::full(new.width, new.height);
    }
    let mut damage = Damage::new();

    let old_pre = old.chunks.first().map_or(old.primitives.len(), |c| c.start);
    let new_pre = new.chunks.first().map_or(new.primitives.len(), |c| c.start);
    diff_runs(
        &old.primitives[..old_pre],
        &new.primitives[..new_pre],
        &mut damage,
    );

    let index: HashMap<ChunkKey, usize> = old
        .chunks
        .iter()
        .enumerate()
        .map(|(i, c)| (c.key, i))
        .collect();
    let mut used = vec![false; old.chunks.len()];
    // Matched chunks must keep their relative order; one that moved before an already matched
    // chunk is treated as changed.
    let mut last_matched = None;
    for (j, chunk) in new.chunks.iter().enumerate() {
        let new_prims = &new.primitives[new.chunk_range(j)];
        match index.get(&chunk.key) {
            Some(&i) if !used[i] => {
                used[i] = true;
                let old_prims = &old.primitives[old.chunk_range(i)];
                if last_matched.is_some_and(|l| i < l) {
                    damage.add(run_bounds(old_prims));
                    damage.add(run_bounds(new_prims));
                } else {
                    last_matched = Some(i);
                    diff_runs(old_prims, new_prims, &mut damage);
                }
            }
            _ => damage.add(run_bounds(new_prims)),
        }
    }
    for (i, used) in used.iter().enumerate() {
        if !used {
            damage.add(run_bounds(&old.primitives[old.chunk_range(i)]));
        }
    }
    damage.clip(RectI::new(0, 0, new.width as i32, new.height as i32));
    damage
}

fn diff_runs(old: &[Primitive], new: &[Primitive], damage: &mut Damage) {
    if old.len() == new.len() {
        for (a, b) in old.iter().zip(new) {
            if a != b {
                damage.add(primitive_bounds(a));
                damage.add(primitive_bounds(b));
            }
        }
    } else {
        damage.add(run_bounds(old));
        damage.add(run_bounds(new));
    }
}

fn run_bounds(prims: &[Primitive]) -> RectI {
    prims
        .iter()
        .fold(RectI::default(), |acc, p| acc.union(&primitive_bounds(p)))
}

/// A rectangle containing every pixel the primitive can touch.
pub fn primitive_bounds(p: &Primitive) -> RectI {
    match p {
        Primitive::Quad(q) => q.rect.intersect(&q.clip.rect),
        Primitive::Shadow(s) => {
            // The shader extends shadows by `blur * 1.5 + 1`.
            let grow = (s.blur * 1.5 + 1.0).ceil() as i32;
            s.rect.outset(grow).intersect(&s.clip.rect)
        }
        Primitive::Glyphs(run) => glyph_run_bounds(run).intersect(&run.clip.rect),
    }
}

fn glyph_run_bounds(run: &GlyphRun) -> RectI {
    let Some(first) = run.glyphs.first() else {
        return RectI::default();
    };
    let (mut x0, mut x1, mut y0, mut y1) = (first.x, first.x, first.y, first.y);
    for g in &run.glyphs[1..] {
        x0 = x0.min(g.x);
        x1 = x1.max(g.x);
        y0 = y0.min(g.y);
        y1 = y1.max(g.y);
    }
    let [ex0, ey0, ex1, ey1] = font_extents(&run.font).map(|e| e * run.size);
    // Rasterization snaps outward by a pixel and positions in quarter pixels.
    let mut pad = 2.0;
    if run.embolden {
        pad += run.size / 24.0 + 1.0;
    }
    if let Some(skew) = run.skew {
        pad += skew.to_radians().tan().abs() * ey1.max(-ey0);
    }
    let left = (x0 + ex0 - pad).floor() as i32;
    let right = (x1 + ex1 + pad).ceil() as i32;
    let top = (y0 - ey1 - pad).floor() as i32;
    let bottom = (y1 - ey0 + pad).ceil() as i32;
    RectI::new(left, top, right - left, bottom - top)
}

thread_local! {
    static EXTENTS: RefCell<HashMap<(u64, u32), [f32; 4]>> = RefCell::new(HashMap::new());
}

/// The box containing every glyph of the font, in ems with y up: `[x_min, y_min, x_max, y_max]`.
fn font_extents(font: &FontData) -> [f32; 4] {
    let key = (font.data.id(), font.index);
    if let Some(e) = EXTENTS.with(|c| c.borrow().get(&key).copied()) {
        return e;
    }
    let e = read_extents(font).unwrap_or([-0.5, -0.5, 1.5, 1.5]);
    EXTENTS.with(|c| c.borrow_mut().insert(key, e));
    e
}

fn read_extents(font: &FontData) -> Option<[f32; 4]> {
    use read_fonts::TableProvider;
    let f = read_fonts::FontRef::from_index(font.data.data(), font.index).ok()?;
    let head = f.head().ok()?;
    let upem = head.units_per_em().max(1) as f32;
    // Include the em box (bitmap glyphs may not be in the outline bounds) and a margin for
    // variations, which can grow glyphs past the default instance's box.
    let m = 0.1;
    Some([
        (head.x_min() as f32 / upem).min(0.0) - m,
        (head.y_min() as f32 / upem).min(-0.3) - m,
        (head.x_max() as f32 / upem).max(1.0) + m,
        (head.y_max() as f32 / upem).max(1.0) + m,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Clip, Quad};
    use anti_webui_style::Color;

    fn quad(x: i32, y: i32, w: i32, h: i32, color: u32) -> Primitive {
        Primitive::Quad(Quad {
            rect: RectI::new(x, y, w, h),
            background: Color::from_rgb(color),
            border_widths: [0; 4],
            border_color: Color::TRANSPARENT,
            radii: [0.0; 4],
            clip: Clip::NONE,
        })
    }

    fn scene(chunks: &[(u64, Vec<Primitive>)]) -> Scene {
        let mut s = Scene {
            width: 200,
            height: 200,
            clear: Color::WHITE,
            ..Default::default()
        };
        for (node, prims) in chunks {
            s.begin_chunk(ChunkKey {
                node: *node,
                part: 0,
            });
            s.primitives.extend(prims.iter().cloned());
        }
        s
    }

    #[test]
    fn identical_scenes_have_no_damage() {
        let a = scene(&[(1, vec![quad(0, 0, 50, 50, 0xff0000)])]);
        assert!(diff(&a, &a.clone()).is_empty());
    }

    #[test]
    fn changed_primitive_damages_old_and_new_bounds() {
        let a = scene(&[
            (1, vec![quad(0, 0, 200, 200, 0xffffff)]),
            (2, vec![quad(10, 10, 20, 20, 0xff0000)]),
        ]);
        let b = scene(&[
            (1, vec![quad(0, 0, 200, 200, 0xffffff)]),
            (2, vec![quad(15, 10, 20, 20, 0xff0000)]),
        ]);
        let d = diff(&a, &b);
        assert_eq!(d.bounds(), RectI::new(10, 10, 25, 20));
    }

    #[test]
    fn inserted_and_removed_chunks_damage_their_bounds() {
        let a = scene(&[
            (1, vec![quad(0, 0, 10, 10, 0)]),
            (2, vec![quad(100, 100, 10, 10, 0)]),
        ]);
        let b = scene(&[
            (1, vec![quad(0, 0, 10, 10, 0)]),
            (3, vec![quad(50, 50, 10, 10, 0)]),
        ]);
        let d = diff(&a, &b);
        assert_eq!(d.rects().len(), 2);
        assert!(d.rects().contains(&RectI::new(50, 50, 10, 10)));
        assert!(d.rects().contains(&RectI::new(100, 100, 10, 10)));
    }

    #[test]
    fn reordered_chunks_are_damaged() {
        let a = scene(&[
            (1, vec![quad(0, 0, 20, 20, 0xff0000)]),
            (2, vec![quad(10, 10, 20, 20, 0x00ff00)]),
        ]);
        let b = scene(&[
            (2, vec![quad(10, 10, 20, 20, 0x00ff00)]),
            (1, vec![quad(0, 0, 20, 20, 0xff0000)]),
        ]);
        assert_eq!(diff(&a, &b).bounds(), RectI::new(0, 0, 20, 20));
    }

    #[test]
    fn size_or_clear_change_damages_everything() {
        let a = scene(&[(1, vec![quad(0, 0, 10, 10, 0)])]);
        let mut b = a.clone();
        b.clear = Color::BLACK;
        assert_eq!(diff(&a, &b).bounds(), RectI::new(0, 0, 200, 200));
    }

    #[test]
    fn damage_merges_and_stays_bounded() {
        let mut d = Damage::new();
        d.add(RectI::new(0, 0, 10, 10));
        d.add(RectI::new(10, 0, 10, 10));
        assert_eq!(d.rects(), &[RectI::new(0, 0, 20, 10)]);
        for i in 0..50 {
            d.add(RectI::new(i * 37 % 500, i * 53 % 500, 3, 3));
        }
        assert!(d.rects().len() <= MAX_RECTS);
    }

    #[test]
    fn history_unions_recent_frames() {
        let mut h = DamageHistory::default();
        assert!(h.last(1).is_none());
        let mut a = Damage::new();
        a.add(RectI::new(0, 0, 5, 5));
        let mut b = Damage::new();
        b.add(RectI::new(100, 100, 5, 5));
        h.push(a);
        h.push(b);
        assert_eq!(h.last(1).unwrap().rects(), &[RectI::new(100, 100, 5, 5)]);
        assert_eq!(h.last(2).unwrap().rects().len(), 2);
        assert!(h.last(3).is_none());
    }
}
