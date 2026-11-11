//! Drawing shapes: what a shape covers is worked out once on the CPU, as bands of coverage that
//! only keep the pixels along its edges, and the GPU paints them. See `docs/design/25-svg.md`.

use anti_webui_core::geometry::{Affine, RectI};
use anti_webui_core::scene::{PathDraw, PathGeometry, Primitive, StrokeStyle};
use anti_webui_core::style::{
    FillRule, Gradient, GradientShape, GradientSpread, GradientUnits, LineCap, LineJoin,
};
use bytemuck::{Pod, Zeroable};
use kurbo::{BezPath, Cap, Join, PathEl, Point, Stroke, StrokeOpts};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// How far a curve can be from the lines that stand in for it, in pixels.
const FLATTENING: f64 = 0.05;
/// Rows of a band: the pixels along the edges are kept per band.
const BAND: i32 = 4;
/// What fits in this many pixels each way is kept as one block.
const BLOCK: i32 = 40;
/// Pixels of the same coverage between two edges that aren't worth a rectangle of their own.
const JOIN_GAP: i32 = 4;
/// A shape over this many pixels each way is only made for what shows of it.
const WHOLE_LIMIT: f64 = 8192.0;
/// How many new shapes in a frame make it worth using more than this thread.
const MANY_SHAPES: usize = 48;
const MAX_THREADS: usize = 8;

/// Says that a run has one coverage, which is in the low bits, instead of pixels in the store.
const SOLID: u32 = 1 << 31;

const STORE_WIDTH: u32 = 2048;
const STORE_ROWS: u32 = 256;
/// How much there can be: a bit more than a screen full of edges at a time.
const STORE_MAX_ROWS: u32 = 4096;
const TABLE_WIDTH: u32 = 1024;
/// Rows of a draw in the table: the first four are read for each of its runs.
const DRAW_ROWS: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Run {
    x: i32,
    y: i32,
    w: u16,
    h: u16,
    /// Where its pixels start in the store, or [`SOLID`] and its coverage.
    alpha: u32,
}

/// A rectangle of a shape on its way to the screen.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct PathInstance {
    pos: [i32; 2],
    size: [u16; 2],
    alpha: u32,
    draw: u32,
}

#[derive(Clone, Copy)]
struct Cell {
    y: i32,
    x: i32,
    /// How much of the row the edges here take away or add for what is to the right of them.
    cover: f32,
    /// How much of that is in this pixel.
    area: f32,
}

/// Turns lines into what they cover: for each pixel they go through, by how much. The cells of the
/// pixels between them are not there, so that what it takes goes with the length of the outline
/// rather than with the area.
struct Cells {
    cells: Vec<Cell>,
    /// What to make: from `x0, y0` up to `x1, y1`.
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl Cells {
    fn new(region: [f32; 4]) -> Self {
        Self {
            cells: Vec::new(),
            x0: region[0],
            y0: region[1],
            x1: region[2],
            y1: region[3],
        }
    }

    fn add(&mut self, y: i32, x: i32, cover: f32, area: f32) {
        if let Some(last) = self.cells.last_mut()
            && last.y == y
            && last.x == x
        {
            last.cover += cover;
            last.area += area;
            return;
        }
        self.cells.push(Cell { y, x, cover, area });
    }

    fn line(&mut self, a: Point, b: Point) {
        let (a, b) = ((a.x as f32, a.y as f32), (b.x as f32, b.y as f32));
        if a.1 == b.1 {
            return;
        }
        let (sign, top, bottom) = if a.1 < b.1 { (1.0, a, b) } else { (-1.0, b, a) };
        let from = top.1.max(self.y0);
        let to = bottom.1.min(self.y1);
        if from >= to {
            return;
        }
        let slope = (bottom.0 - top.0) / (bottom.1 - top.1);
        let mut x = top.0 + (from - top.1) * slope;
        let mut row = floor(from);
        let end = -floor(-to);
        while row < end {
            let y_from = from.max(row as f32);
            let y_to = to.min((row + 1) as f32);
            let dy = y_to - y_from;
            let next = x + dy * slope;
            self.in_row(row, x, next, dy * sign);
            x = next;
            row += 1;
        }
    }

    /// `d` is how far down the line goes in this row, with the sign of its direction.
    fn in_row(&mut self, row: i32, a: f32, b: f32, d: f32) {
        // What is left of what is made counts all the same, and what is right of it ends what is
        // covered just outside of it.
        let left = a.min(b).clamp(self.x0, self.x1);
        let right = a.max(b).clamp(self.x0, self.x1);
        let first = floor(left);
        let last = (-floor(-right) - 1).max(first);
        if first == last {
            let middle = (left + right) * 0.5 - first as f32;
            self.add(row, first, d, d * (1.0 - middle));
            return;
        }
        let per_x = d / (right - left);
        let start = (first + 1) as f32 - left;
        let share = per_x * start;
        self.add(row, first, share, share * start * 0.5);
        for x in first + 1..last {
            self.add(row, x, per_x, per_x * 0.5);
        }
        let end = right - last as f32;
        let share = per_x * end;
        self.add(row, last, share, share * (1.0 - end * 0.5));
    }
}

fn coverage(v: f32, rule: FillRule) -> u8 {
    let c = match rule {
        FillRule::NonZero => v.abs().min(1.0),
        FillRule::EvenOdd => {
            let t = v.abs() % 2.0;
            if t > 1.0 { 2.0 - t } else { t }
        }
    };
    (c * 255.0 + 0.5) as u8
}

/// `f32::floor` without going through a call on CPUs that aren't assumed to have an instruction for it.
#[inline]
fn floor(v: f32) -> i32 {
    let i = v as i32;
    if (i as f32) > v { i - 1 } else { i }
}

/// What covering takes, ready for the GPU.
#[derive(Default, Debug, PartialEq)]
pub(crate) struct Strips {
    runs: Vec<Run>,
    alpha: Vec<u8>,
}

/// The cells by row, which lines tend to have just a few of, and from left to right.
fn in_order(cells: Vec<Cell>) -> Vec<Cell> {
    let Some(first) = cells.first() else {
        return cells;
    };
    let (mut top, mut bottom) = (first.y, first.y);
    for c in &cells {
        top = top.min(c.y);
        bottom = bottom.max(c.y);
    }
    let mut starts = vec![0u32; (bottom - top + 2) as usize];
    for c in &cells {
        starts[(c.y - top) as usize + 1] += 1;
    }
    for i in 1..starts.len() {
        starts[i] += starts[i - 1];
    }
    let mut out = vec![cells[0]; cells.len()];
    let mut at = starts.clone();
    for c in &cells {
        let i = &mut at[(c.y - top) as usize];
        out[*i as usize] = *c;
        *i += 1;
    }
    for pair in starts.windows(2) {
        out[pair[0] as usize..pair[1] as usize].sort_unstable_by_key(|c| c.x);
    }
    out
}

/// Makes runs out of the cells: each band says where its edges are, with their pixels, and what is
/// between them has one coverage for each row.
fn strips_of(cells: Vec<Cell>, band: i32, rule: FillRule) -> Strips {
    let cells = in_order(cells);
    let mut out = Strips::default();
    // Where the rows of the band start in `cells`, and how far each got.
    let mut intervals: Vec<(i32, i32)> = Vec::new();
    let mut xs: Vec<i32> = Vec::new();
    let mut rows: Vec<&[Cell]> = Vec::new();
    // The coverage of each row after each interval.
    let mut gaps: Vec<u8> = Vec::new();
    let mut last_solids = 0..0;
    let mut i = 0;
    while i < cells.len() {
        let top = cells[i].y.div_euclid(band) * band;
        rows.clear();
        xs.clear();
        for y in top..top + band {
            let start = i;
            while i < cells.len() && cells[i].y == y {
                xs.push(cells[i].x);
                i += 1;
            }
            rows.push(&cells[start..i]);
        }
        xs.sort_unstable();
        intervals.clear();
        for &x in &xs {
            match intervals.last_mut() {
                Some((_, end)) if x < *end + JOIN_GAP => *end = (*end).max(x + 1),
                _ => intervals.push((x, x + 1)),
            }
        }
        let height = band as usize;
        let starts: Vec<usize> = intervals
            .iter()
            .scan(out.alpha.len(), |at, (a, b)| {
                let start = *at;
                *at += (b - a) as usize * height;
                Some(start)
            })
            .collect();
        let total: usize = intervals
            .iter()
            .map(|(a, b)| (b - a) as usize * height)
            .sum();
        out.alpha.resize(out.alpha.len() + total, 0);
        gaps.clear();
        gaps.resize(intervals.len() * height, 0);
        for (r, row) in rows.iter().enumerate() {
            let mut acc = 0.0;
            let mut next = 0;
            for (n, (a, b)) in intervals.iter().enumerate() {
                let width = (b - a) as usize;
                let at = starts[n] + r * width;
                for (offset, x) in (*a..*b).enumerate() {
                    let mut v = acc;
                    // Lines that come by again make cells of their own.
                    while let Some(c) = row.get(next).filter(|c| c.x == x) {
                        v += c.area;
                        acc += c.cover;
                        next += 1;
                    }
                    out.alpha[at + offset] = coverage(v, rule);
                }
                gaps[n * height + r] = coverage(acc, rule);
            }
        }
        let solids_from = out.runs.len();
        for (n, (a, b)) in intervals.iter().enumerate() {
            out.runs.push(Run {
                x: *a,
                y: top,
                w: (b - a) as u16,
                h: band as u16,
                alpha: starts[n] as u32,
            });
            let Some((next, _)) = intervals.get(n + 1) else {
                continue;
            };
            // Rows of the same coverage go together; they differ where a level edge goes through.
            let mut r = 0;
            while r < height {
                let value = gaps[n * height + r];
                let mut end = r + 1;
                while end < height && gaps[n * height + end] == value {
                    end += 1;
                }
                if value > 0 {
                    let run = Run {
                        x: *b,
                        y: top + r as i32,
                        w: (next - b) as u16,
                        h: (end - r) as u16,
                        alpha: SOLID | value as u32,
                    };
                    // What goes on from the band above is one rectangle.
                    let longer = out.runs[last_solids.clone()].iter_mut().find(|p| {
                        p.alpha == run.alpha
                            && p.x == run.x
                            && p.w == run.w
                            && p.y + p.h as i32 == run.y
                    });
                    match longer {
                        Some(p) => p.h += run.h,
                        None => out.runs.push(run),
                    }
                }
                r = end;
            }
        }
        last_solids = solids_from..out.runs.len();
    }
    out
}

/// What of a closed outline is on the inner side of a line from `a` to `b`. An outline that isn't
/// convex gets edges along the line, whose coverage comes to nothing.
fn cut_by_edge(points: &mut Vec<Point>, spare: &mut Vec<Point>, a: Point, b: Point, sign: f64) {
    spare.clear();
    let side = |p: Point| sign * ((b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x));
    let Some(&last) = points.last() else {
        return;
    };
    let mut previous = last;
    let mut previous_side = side(previous);
    for &p in points.iter() {
        let this_side = side(p);
        if (this_side >= 0.0) != (previous_side >= 0.0) {
            let t = previous_side / (previous_side - this_side);
            spare.push(previous.lerp(p, t));
        }
        if this_side >= 0.0 {
            spare.push(p);
        }
        previous = p;
        previous_side = this_side;
    }
    std::mem::swap(points, spare);
}

/// Sends a closed outline on, after cutting it to `quads`.
fn cut_outline(cells: &mut Cells, points: &mut Vec<Point>, quads: &[LocalQuad]) {
    let mut spare = Vec::new();
    for quad in quads {
        let area: f64 = (0..4)
            .map(|i| {
                let (p, q) = (quad[i], quad[(i + 1) % 4]);
                p.x * q.y - q.x * p.y
            })
            .sum();
        let sign = area.signum();
        for i in 0..4 {
            cut_by_edge(points, &mut spare, quad[i], quad[(i + 1) % 4], sign);
        }
    }
    let mut at = points.last().copied();
    for &p in points.iter() {
        if let Some(from) = at {
            cells.line(from, p);
        }
        at = Some(p);
    }
    points.clear();
}

type LocalQuad = [Point; 4];

/// What is covered by what `path` goes along, through `m`. `region` is what to make of it. `quads`
/// are in the same coordinates as what comes out, and cut it.
fn rasterize(
    path: &BezPath,
    m: kurbo::Affine,
    region: [f32; 4],
    rule: FillRule,
    quads: &[LocalQuad],
) -> Strips {
    let mut cells = Cells::new(region);
    let mut start = Point::ZERO;
    let mut at = Point::ZERO;
    let mut open = false;
    let mut points: Vec<Point> = Vec::new();
    let transformed = path.elements().iter().map(|e| m * *e);
    let cut = !quads.is_empty();
    kurbo::flatten(transformed, FLATTENING, |el| match el {
        PathEl::MoveTo(p) => {
            if cut {
                cut_outline(&mut cells, &mut points, quads);
                points.push(p);
            } else {
                if open {
                    cells.line(at, start);
                }
                start = p;
                at = p;
                open = true;
            }
        }
        PathEl::LineTo(p) => {
            if cut {
                points.push(p);
            } else {
                cells.line(at, p);
                at = p;
            }
        }
        PathEl::ClosePath if !cut => {
            cells.line(at, start);
            at = start;
        }
        _ => {}
    });
    if cut {
        cut_outline(&mut cells, &mut points, quads);
    } else if open {
        cells.line(at, start);
    }
    if cells.cells.is_empty() {
        return Strips::default();
    }
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for c in &cells.cells {
        x0 = x0.min(c.x);
        y0 = y0.min(c.y);
        x1 = x1.max(c.x);
        y1 = y1.max(c.y);
    }
    let band = if x1 - x0 < BLOCK && y1 - y0 < BLOCK {
        BLOCK
    } else {
        BAND
    };
    strips_of(cells.cells, band, rule)
}

/// What `d` covers through `local`, as far as `area`.
fn make_strips(d: &PathDraw, local: kurbo::Affine, area: RectI, quads: &[LocalQuad]) -> Strips {
    let area = [
        area.x as f32,
        area.y as f32,
        area.right() as f32,
        area.bottom() as f32,
    ];
    match &d.stroke {
        Some(stroke) => {
            let scale = local.determinant().abs().sqrt();
            let path = outline(d.geometry.path(), stroke, scale);
            rasterize(&path, local, area, FillRule::NonZero, quads)
        }
        None => rasterize(d.geometry.path(), local, area, d.rule, quads),
    }
}

/// The outline of `path` as a shape.
fn outline(path: &BezPath, style: &StrokeStyle, scale: f64) -> BezPath {
    let mut stroke = Stroke::new(style.width as f64)
        .with_caps(match style.cap {
            LineCap::Butt => Cap::Butt,
            LineCap::Round => Cap::Round,
            LineCap::Square => Cap::Square,
        })
        .with_join(match style.join {
            LineJoin::Miter => Join::Miter,
            LineJoin::Round => Join::Round,
            LineJoin::Bevel => Join::Bevel,
        })
        .with_miter_limit(style.miter_limit as f64);
    if let Some((dashes, shift)) = style
        .dashes
        .as_deref()
        .and_then(|d| dashes_to_stroke(d, style.cap))
    {
        let total: f64 = dashes.iter().sum();
        let offset = (style.dash_offset as f64 - shift).rem_euclid(total);
        stroke = stroke.with_dashes(offset, dashes);
    }
    kurbo::stroke(
        path.elements().iter().copied(),
        &stroke,
        &StrokeOpts::default(),
        FLATTENING / scale.max(1e-3),
    )
}

/// The dashes to stroke with, as SVG means them, with how much further into them the line starts,
/// or `None` for a line without gaps. A pattern
/// whose gaps are all empty is a whole line (Mermaid draws its edges with `0 0 65 0`), and the
/// stroker makes a mess of dashes that are empty, which only show with caps that stick out.
fn dashes_to_stroke(dashes: &[f32], cap: LineCap) -> Option<(Vec<f64>, f64)> {
    if dashes.iter().any(|d| !d.is_finite() || *d < 0.0) || dashes.iter().sum::<f32>() <= 0.0 {
        return None;
    }
    // An odd number of them is said twice.
    let mut list: Vec<f64> = dashes.iter().map(|d| *d as f64).collect();
    if list.len() % 2 == 1 {
        list.extend_from_within(..);
    }
    if list.iter().skip(1).step_by(2).all(|gap| *gap == 0.0) {
        return None;
    }
    if cap != LineCap::Butt {
        return Some((list, 0.0));
    }
    // An empty dash adds its room to the gap before it.
    let mut out: Vec<f64> = Vec::with_capacity(list.len());
    let mut lead = 0.0;
    for pair in list.chunks_exact(2) {
        let (dash, gap) = (pair[0], pair[1]);
        if dash == 0.0 {
            match out.last_mut() {
                Some(last) => *last += gap,
                None => lead += gap,
            }
        } else {
            out.push(dash);
            out.push(gap);
        }
    }
    if out.is_empty() {
        return None;
    }
    // What came before the first dash goes after the last, where the pattern starts over, and the
    // line starts that much into it.
    if let Some(last) = out.last_mut() {
        *last += lead;
    }
    Some((out, -lead))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Key {
    geometry: u64,
    rule: FillRule,
    /// The numbers of the matrix but for its whole pixels.
    matrix: [u32; 6],
    /// A hash of how the outline goes.
    stroke: u64,
    /// A hash of what it is cut to.
    cut: u64,
    region: Option<[i32; 4]>,
}

struct Entry {
    runs: Vec<Run>,
    /// Bytes it has in the store.
    bytes: u32,
    /// The last frame that drew it.
    frame: u64,
}

/// A row of the table for what is drawn in a frame.
pub type TableRow = [f32; 4];

fn stroke_key(stroke: &StrokeStyle) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    stroke.width.to_bits().hash(&mut h);
    stroke.cap.hash(&mut h);
    stroke.join.hash(&mut h);
    stroke.miter_limit.to_bits().hash(&mut h);
    stroke.dash_offset.to_bits().hash(&mut h);
    if let Some(dashes) = &stroke.dashes {
        for d in dashes.iter() {
            d.to_bits().hash(&mut h);
        }
    }
    // Never what a shape without an outline has.
    h.finish() | 1
}

/// What `d` is cut to in the coordinates of what is made of it: placed by `k`, `dx` and `dy`, less
/// the whole pixels `ox` and `oy` it is kept without. And a hash of them.
fn local_quads(d: &PathDraw, k: i32, dx: i32, dy: i32, ox: f64, oy: f64) -> (Vec<LocalQuad>, u64) {
    let Some(quads) = &d.quads else {
        return (Vec::new(), 0);
    };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let local: Vec<LocalQuad> = quads
        .iter()
        .map(|q| {
            q.map(|(x, y)| {
                let p = Point::new(x * k as f64 + dx as f64 - ox, y * k as f64 + dy as f64 - oy);
                (p.x as f32).to_bits().hash(&mut h);
                (p.y as f32).to_bits().hash(&mut h);
                p
            })
        })
        .collect();
    (local, h.finish() | 1)
}

fn to_kurbo(m: Affine) -> kurbo::Affine {
    kurbo::Affine::new([m.a, m.b, m.c, m.d, m.e, m.f])
}

/// From where it shows to the coordinates of a gradient in which what it says is simple: `x` of a
/// linear one goes from 0 to 1 along it; a radial one has the unit circle.
fn gradient_rows(g: &Gradient, geometry: &PathGeometry, m: Affine) -> Option<([f64; 6], [f32; 2])> {
    let user = to_kurbo(m);
    let units = match g.units {
        GradientUnits::UserSpace => kurbo::Affine::IDENTITY,
        GradientUnits::ObjectBoundingBox => {
            let b = geometry.bounds();
            kurbo::Affine::new([b.width(), 0.0, 0.0, b.height(), b.x0, b.y0])
        }
    };
    let inner = kurbo::Affine::new(g.transform);
    let to_gradient = (user * units * inner).inverse();
    if !to_gradient.determinant().is_finite() {
        return None;
    }
    match g.shape {
        GradientShape::Linear { x1, y1, x2, y2 } => {
            let (dx, dy) = ((x2 - x1) as f64, (y2 - y1) as f64);
            let len = dx * dx + dy * dy;
            if len == 0.0 {
                return None;
            }
            let along = kurbo::Affine::new([
                dx / len,
                -dy / len,
                dy / len,
                dx / len,
                -(x1 as f64 * dx + y1 as f64 * dy) / len,
                (x1 as f64 * dy - y1 as f64 * dx) / len,
            ]);
            Some(((along * to_gradient).as_coeffs(), [0.0, 0.0]))
        }
        GradientShape::Radial { cx, cy, r, fx, fy } => {
            if r <= 0.0 {
                return None;
            }
            let r = r as f64;
            let circle =
                kurbo::Affine::new([1.0 / r, 0.0, 0.0, 1.0 / r, -cx as f64 / r, -cy as f64 / r]);
            // The first stop stays inside of the circle.
            let (mut qx, mut qy) = ((fx - cx) as f64 / r, (fy - cy) as f64 / r);
            let d = (qx * qx + qy * qy).sqrt();
            if d > 0.999 {
                qx *= 0.999 / d;
                qy *= 0.999 / d;
            }
            Some(((circle * to_gradient).as_coeffs(), [qx as f32, qy as f32]))
        }
    }
}

/// A shape to make.
type Job<'a> = (Key, &'a PathDraw, kurbo::Affine, RectI, Vec<LocalQuad>);

pub struct PathPass {
    globals_layout: wgpu::BindGroupLayout,
    shader: wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
    max_rows: u32,
    pipeline: Option<(wgpu::RenderPipeline, wgpu::BindGroupLayout)>,
    /// Where compiled shaders are kept for the next launch.
    shader_cache: Option<std::sync::Arc<crate::shader_cache::ShaderCache>>,
    entries: HashMap<Key, Entry>,
    /// How much of the store is taken.
    used: u32,
    /// What isn't in the store yet, starting at this index.
    pending: Vec<u8>,
    pending_at: u32,
    store: Option<(wgpu::Texture, u32)>,
    table: Option<(wgpu::Texture, u32)>,
    bind: Option<wgpu::BindGroup>,
    /// The store has no room for what is asked.
    full: bool,
    /// Rows it may take: more once what a single frame needs didn't fit.
    limit: u32,
    /// What a frame needs doesn't fit whatever the store is like: the rest isn't shown.
    gave_up: bool,
    frame: u64,
    /// What this frame draws takes this many bytes of the store.
    live: u64,
}

impl PathPass {
    pub fn new(
        globals_layout: wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        blend: Option<wgpu::BlendState>,
        shader: wgpu::ShaderModule,
        max_texture: u32,
    ) -> Self {
        Self {
            globals_layout,
            shader,
            format,
            blend,
            max_rows: max_texture,
            pipeline: None,
            shader_cache: None,
            entries: HashMap::new(),
            used: 0,
            pending: Vec::new(),
            pending_at: 0,
            store: None,
            table: None,
            bind: None,
            full: false,
            limit: STORE_MAX_ROWS.min(max_texture),
            gave_up: false,
            frame: 0,
            live: 0,
        }
    }

    pub(crate) fn with_shader_cache(
        mut self,
        cache: Option<std::sync::Arc<crate::shader_cache::ShaderCache>>,
    ) -> Self {
        self.shader_cache = cache;
        self
    }

    fn ensure_pipeline(&mut self, device: &wgpu::Device) {
        if self.pipeline.is_some() {
            return;
        }
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("paths"),
            entries: &[texture(0), texture(1)],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("paths"),
            bind_group_layouts: &[Some(&self.globals_layout), Some(&layout)],
            immediate_size: 0,
        });
        let attrs = wgpu::vertex_attr_array![
            0 => Sint32x2, 1 => Uint16x2, 2 => Uint32, 3 => Uint32
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("paths"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some("vs_path"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<PathInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attrs,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &self.shader,
                entry_point: Some("fs_path"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: self.format,
                    blend: self.blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: self.shader_cache.as_ref().map(|c| &c.cache),
        });
        self.pipeline = Some((pipeline, layout));
        if let Some(c) = &self.shader_cache {
            c.save_soon();
        }
    }

    /// Starts over with an empty store, after it got full. `cleared`: it already did for this frame,
    /// so that the frame needs more than there was room for.
    pub fn take_full(&mut self, cleared: bool) -> bool {
        if !std::mem::take(&mut self.full) {
            return false;
        }
        if cleared {
            if self.limit >= self.max_rows {
                log::warn!("too many shapes for what the GPU can hold");
                self.gave_up = true;
            }
            self.limit = (self.limit * 2).min(self.max_rows);
        }
        self.entries.clear();
        self.used = 0;
        self.pending.clear();
        self.pending_at = 0;
        true
    }

    /// Call once per frame, before anything is added.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        self.live = 0;
    }

    fn capacity(&self) -> u64 {
        STORE_WIDTH as u64 * self.limit as u64
    }

    /// Adds what shows `d`, moved by whole pixels and made `k` times bigger. `None` means the store
    /// is full.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        device: &wgpu::Device,
        d: &PathDraw,
        k: i32,
        dx: i32,
        dy: i32,
        target: RectI,
        clip: ([f32; 4], [f32; 4]),
        instances: &mut Vec<PathInstance>,
        table: &mut Vec<TableRow>,
    ) -> Option<bool> {
        let shows = d
            .bounds
            .scaled(k)
            .translate(dx, dy)
            .intersect(&d.clip.rect.scaled(k).translate(dx, dy))
            .intersect(&target);
        if shows.is_empty() {
            return Some(false);
        }
        self.ensure_pipeline(device);
        let m = Affine::translation(dx as f64, dy as f64)
            .then_apply(&Affine::scaling(k as f64, k as f64))
            .then_apply(&d.transform);
        let (ox, oy) = (m.e.floor(), m.f.floor());
        let local = kurbo::Affine::new([m.a, m.b, m.c, m.d, m.e - ox, m.f - oy]);
        let bounds = d.bounds.scaled(k).translate(dx, dy);
        // What is too big to make all of is made as far as it shows, which depends on where it is.
        let region = (bounds.width as f64 > WHOLE_LIMIT || bounds.height as f64 > WHOLE_LIMIT)
            .then_some(shows.translate(-ox as i32, -oy as i32).outset(64));
        let (quads, cut) = local_quads(d, k, dx, dy, ox, oy);
        let key = Key {
            geometry: d.geometry.id(),
            rule: d.rule,
            matrix: local.as_coeffs().map(|v| (v as f32).to_bits()),
            stroke: d.stroke.as_deref().map_or(0, stroke_key),
            cut,
            region: region.map(|r| [r.x, r.y, r.width, r.height]),
        };
        if !self.entries.contains_key(&key) {
            let area = match region {
                Some(r) => r,
                None => bounds.translate(-ox as i32, -oy as i32).outset(2),
            };
            let strips = make_strips(d, local, area, &quads);
            if !self.store_strips(key, strips) {
                if self.gave_up {
                    return Some(false);
                }
                self.full = true;
                return None;
            }
        }
        let entry = self.entries.get_mut(&key).expect("put there above");
        if entry.frame != self.frame {
            entry.frame = self.frame;
            self.live += entry.bytes as u64;
        }
        let draw = table.len() as u32;
        push_rows(d, m, clip, table);
        let (ox, oy) = (ox as i32, oy as i32);
        for run in &self.entries[&key].runs {
            let (x, y) = (run.x + ox, run.y + oy);
            if x < shows.right()
                && y < shows.bottom()
                && x + run.w as i32 > shows.x
                && y + run.h as i32 > shows.y
            {
                instances.push(PathInstance {
                    pos: [x, y],
                    size: [run.w, run.h],
                    alpha: run.alpha,
                    draw,
                });
            }
        }
        Some(true)
    }

    /// Puts what was made in the store; `false` if there is no room.
    fn store_strips(&mut self, key: Key, strips: Strips) -> bool {
        let needed = self.used as u64 + strips.alpha.len() as u64;
        if needed > self.capacity() {
            // Going on is better than making all of it again, if most of it is for this frame.
            while needed > self.capacity()
                && self.limit < self.max_rows
                && self.live * 2 >= self.capacity()
            {
                self.limit = (self.limit * 2).min(self.max_rows);
            }
            if needed > self.capacity() {
                return false;
            }
        }
        let base = self.used;
        self.used += strips.alpha.len() as u32;
        if self.pending.is_empty() {
            self.pending_at = base;
        }
        self.pending.extend_from_slice(&strips.alpha);
        let bytes = strips.alpha.len() as u32;
        let mut runs = strips.runs;
        for run in &mut runs {
            if run.alpha & SOLID == 0 {
                run.alpha += base;
            }
        }
        self.entries.insert(
            key,
            Entry {
                runs,
                bytes,
                frame: 0,
            },
        );
        true
    }

    /// Makes at once, on all CPUs, the shapes of a scene that aren't there yet. It is only worth it
    /// for many: when something big comes up or changes size.
    pub fn prepare_many(&mut self, device: &wgpu::Device, primitives: &[Primitive]) {
        let mut jobs: Vec<Job<'_>> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        self.collect_jobs(primitives, &mut jobs, &mut seen);
        if jobs.len() < MANY_SHAPES {
            return;
        }
        self.ensure_pipeline(device);
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(MAX_THREADS);
        let next = std::sync::atomic::AtomicUsize::new(0);
        let made: Vec<Vec<(usize, Strips)>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..threads)
                .map(|_| {
                    scope.spawn(|| {
                        let mut out = Vec::new();
                        loop {
                            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let Some((_, d, local, area, quads)) = jobs.get(i) else {
                                return out;
                            };
                            out.push((i, make_strips(d, *local, *area, quads)));
                        }
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_default())
                .collect()
        });
        for (i, strips) in made.into_iter().flatten() {
            // What doesn't fit is left for when it is drawn.
            let key = jobs[i].0;
            if !self.store_strips(key, strips) {
                return;
            }
            let frame = self.frame;
            let entry = self.entries.get_mut(&key).expect("put there just now");
            entry.frame = frame;
            self.live += entry.bytes as u64;
        }
    }

    fn collect_jobs<'a>(
        &self,
        primitives: &'a [Primitive],
        jobs: &mut Vec<Job<'a>>,
        seen: &mut std::collections::HashSet<Key>,
    ) {
        for p in primitives {
            match p {
                Primitive::Path(d) => {
                    let m = d.transform;
                    let (ox, oy) = (m.e.floor(), m.f.floor());
                    let local = kurbo::Affine::new([m.a, m.b, m.c, m.d, m.e - ox, m.f - oy]);
                    if d.bounds.width as f64 > WHOLE_LIMIT || d.bounds.height as f64 > WHOLE_LIMIT {
                        continue;
                    }
                    let (quads, cut) = local_quads(d, 1, 0, 0, ox, oy);
                    let key = Key {
                        geometry: d.geometry.id(),
                        rule: d.rule,
                        matrix: local.as_coeffs().map(|v| (v as f32).to_bits()),
                        stroke: d.stroke.as_deref().map_or(0, stroke_key),
                        cut,
                        region: None,
                    };
                    if !self.entries.contains_key(&key) && seen.insert(key) {
                        let area = d.bounds.translate(-ox as i32, -oy as i32).outset(2);
                        jobs.push((key, d, local, area, quads));
                    }
                }
                Primitive::Layer(layer) => {
                    self.collect_jobs(&layer.content.primitives, jobs, seen);
                }
                _ => {}
            }
        }
    }

    /// Hands to the GPU what this frame added. Call before anything is drawn.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, table: &[TableRow]) {
        if self.pipeline.is_none() {
            return;
        }
        let mut changed = false;
        let rows = self
            .used
            .div_ceil(STORE_WIDTH)
            .max(1)
            .next_power_of_two()
            .max(STORE_ROWS);
        if self.store.as_ref().is_none_or(|(_, have)| *have < rows) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("path store"),
                size: wgpu::Extent3d {
                    width: STORE_WIDTH,
                    height: rows,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            if let Some((old, have)) = &self.store {
                let mut encoder = device.create_command_encoder(&Default::default());
                encoder.copy_texture_to_texture(
                    old.as_image_copy(),
                    texture.as_image_copy(),
                    wgpu::Extent3d {
                        width: STORE_WIDTH,
                        height: *have,
                        depth_or_array_layers: 1,
                    },
                );
                queue.submit([encoder.finish()]);
            }
            self.store = Some((texture, rows));
            changed = true;
        }
        let (store, _) = self.store.as_ref().expect("made above");
        write_store(queue, store, self.pending_at, &self.pending);
        self.pending.clear();

        let rows = (table.len() as u32).div_ceil(TABLE_WIDTH).max(1);
        if self.table.as_ref().is_none_or(|(_, have)| *have < rows) {
            let rows = rows.next_power_of_two();
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("path table"),
                size: wgpu::Extent3d {
                    width: TABLE_WIDTH,
                    height: rows,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.table = Some((texture, rows));
            changed = true;
        }
        if !table.is_empty() {
            let (texture, _) = self.table.as_ref().expect("made above");
            let height = (table.len() as u32).div_ceil(TABLE_WIDTH);
            let mut bytes: Vec<TableRow> = Vec::with_capacity((height * TABLE_WIDTH) as usize);
            bytes.extend_from_slice(table);
            bytes.resize((height * TABLE_WIDTH) as usize, [0.0; 4]);
            queue.write_texture(
                texture.as_image_copy(),
                bytemuck::cast_slice(&bytes),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(TABLE_WIDTH * 16),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: TABLE_WIDTH,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
        if changed || self.bind.is_none() {
            let (_, layout) = self.pipeline.as_ref().expect("checked above");
            let store = self
                .store
                .as_ref()
                .expect("made above")
                .0
                .create_view(&Default::default());
            let table = self
                .table
                .as_ref()
                .expect("made above")
                .0
                .create_view(&Default::default());
            self.bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("paths"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&store),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&table),
                    },
                ],
            }));
        }
    }

    /// How much of the store is taken, in bytes, for measuring.
    pub fn store_used(&self) -> u32 {
        self.used
    }

    pub fn draw_state(&self) -> Option<(&wgpu::RenderPipeline, &wgpu::BindGroup)> {
        Some((&self.pipeline.as_ref()?.0, self.bind.as_ref()?))
    }
}

/// Bytes that go on from index `at` of the store, whose rows are all as wide.
fn write_store(queue: &wgpu::Queue, store: &wgpu::Texture, at: u32, bytes: &[u8]) {
    let mut at = at;
    let mut left = bytes;
    while !left.is_empty() {
        let x = at % STORE_WIDTH;
        let y = at / STORE_WIDTH;
        // A row at a time up to where a row starts, then all whole rows at once.
        let (width, height, take) = if x != 0 || (left.len() as u32) < STORE_WIDTH {
            let width = (STORE_WIDTH - x).min(left.len() as u32);
            (width, 1, width as usize)
        } else {
            let rows = left.len() as u32 / STORE_WIDTH;
            (STORE_WIDTH, rows, (rows * STORE_WIDTH) as usize)
        };
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: store,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &left[..take],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        left = &left[take..];
        at += take as u32;
    }
}

/// What the shader reads for a draw: its color, its clip, and how to work out a gradient.
fn push_rows(d: &PathDraw, m: Affine, clip: ([f32; 4], [f32; 4]), table: &mut Vec<TableRow>) {
    use anti_webui_core::scene::PathPaint;
    let (color, gradient) = match &d.paint {
        PathPaint::Color(c) => {
            let mut p = c.premultiplied();
            p.iter_mut().for_each(|v| *v *= d.opacity);
            (p, None)
        }
        PathPaint::Gradient(g) => {
            let rows = (g.stops.len() > 1)
                .then(|| gradient_rows(g, &d.geometry, m))
                .flatten();
            match rows {
                Some(rows) => ([0.0, 0.0, 0.0, d.opacity], Some((g, rows))),
                // Nothing to go between: the last stop, as SVG says.
                None => {
                    let last = g.stops.last().map_or([0.0; 4], |s| s.color.premultiplied());
                    (last.map(|v| v * d.opacity), None)
                }
            }
        }
    };
    let at = table.len();
    table.push(color);
    table.push(clip.0);
    table.push(clip.1);
    let Some((g, (matrix, focus))) = gradient else {
        table.push([0.0; 4]);
        table.push([0.0; 4]);
        table.push([0.0; 4]);
        return;
    };
    debug_assert_eq!(DRAW_ROWS, 6);
    let kind = match g.shape {
        GradientShape::Linear { .. } => 1.0,
        GradientShape::Radial { .. } => 2.0,
    };
    let spread = match g.spread {
        GradientSpread::Pad => 0.0,
        GradientSpread::Reflect => 1.0,
        GradientSpread::Repeat => 2.0,
    };
    let stops_at = at + DRAW_ROWS;
    table.push([kind, stops_at as f32, g.stops.len() as f32, spread]);
    table.push([
        matrix[0] as f32,
        matrix[1] as f32,
        matrix[2] as f32,
        matrix[3] as f32,
    ]);
    table.push([matrix[4] as f32, matrix[5] as f32, focus[0], focus[1]]);
    for stop in &g.stops {
        let c = stop.color;
        let p = c.premultiplied();
        let a = p[3];
        let straight = if a > 0.0 {
            [p[0] / a, p[1] / a, p[2] / a, a]
        } else {
            [0.0; 4]
        };
        table.push(straight);
        table.push([stop.offset, 0.0, 0.0, 0.0]);
    }
}

/// Shared with tests: how much a pixel of `strips` is covered.
#[cfg(test)]
fn coverage_at(strips: &Strips, x: i32, y: i32) -> u8 {
    for run in &strips.runs {
        if x >= run.x && y >= run.y && x < run.x + run.w as i32 && y < run.y + run.h as i32 {
            if run.alpha & SOLID != 0 {
                return (run.alpha & 0xff) as u8;
            }
            let i =
                run.alpha as usize + (y - run.y) as usize * run.w as usize + (x - run.x) as usize;
            return strips.alpha[i];
        }
    }
    0
}

#[cfg(test)]
mod tests {
    #[test]
    fn dashes_that_are_empty_are_left_out() {
        use super::dashes_to_stroke;
        use anti_webui_core::style::LineCap;
        assert_eq!(
            dashes_to_stroke(&[0.0, 0.0, 65.0, 0.0], LineCap::Butt),
            None
        );
        assert_eq!(dashes_to_stroke(&[0.0, 0.0], LineCap::Butt), None);
        assert_eq!(
            dashes_to_stroke(&[4.0, 2.0], LineCap::Butt),
            Some((vec![4.0, 2.0], 0.0))
        );
        assert_eq!(
            dashes_to_stroke(&[3.0], LineCap::Butt),
            Some((vec![3.0, 3.0], 0.0))
        );
        // A gap first: the same pattern, starting into it.
        assert_eq!(
            dashes_to_stroke(&[0.0, 10.0, 5.0, 5.0], LineCap::Butt),
            Some((vec![5.0, 15.0], -10.0))
        );
        // Round caps make dots of empty dashes.
        assert_eq!(
            dashes_to_stroke(&[0.0, 4.0], LineCap::Round),
            Some((vec![0.0, 4.0], 0.0))
        );
    }

    use super::*;
    use kurbo::Shape;

    fn made(path: &BezPath, rule: FillRule) -> Strips {
        rasterize(
            path,
            kurbo::Affine::IDENTITY,
            [-10.0, -10.0, 4000.0, 4000.0],
            rule,
            &[],
        )
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        kurbo::Rect::new(x0, y0, x1, y1).into_path(0.1)
    }

    #[test]
    fn a_rectangle_on_whole_pixels_covers_them_all() {
        let s = made(&rect(2.0, 3.0, 12.0, 9.0), FillRule::NonZero);
        for y in 0..14 {
            for x in 0..16 {
                let inside = (2..12).contains(&x) && (3..9).contains(&y);
                assert_eq!(
                    coverage_at(&s, x, y),
                    if inside { 255 } else { 0 },
                    "{x} {y}"
                );
            }
        }
    }

    #[test]
    fn half_covered_pixels_are_half_covered() {
        let s = made(&rect(1.5, 1.5, 3.5, 3.5), FillRule::NonZero);
        assert_eq!(coverage_at(&s, 1, 2), 128);
        assert_eq!(coverage_at(&s, 3, 2), 128);
        assert_eq!(coverage_at(&s, 2, 1), 128);
        assert_eq!(coverage_at(&s, 2, 3), 128);
        assert_eq!(coverage_at(&s, 1, 1), 64);
        assert_eq!(coverage_at(&s, 2, 2), 255);
    }

    #[test]
    fn a_big_rectangle_takes_next_to_nothing() {
        let s = made(&rect(0.0, 0.0, 3000.0, 3000.0), FillRule::NonZero);
        assert!(s.alpha.len() < 3000 * 4 * 2 * 2, "{}", s.alpha.len());
        assert!(s.runs.len() < 3000, "{}", s.runs.len());
        assert_eq!(coverage_at(&s, 1500, 1500), 255);
        assert_eq!(coverage_at(&s, 3001, 1500), 0);
    }

    #[test]
    fn a_long_diagonal_takes_as_much_as_it_is_long() {
        let path = kurbo::stroke(
            [
                PathEl::MoveTo((0.0, 0.0).into()),
                PathEl::LineTo((3000.0, 3000.0).into()),
            ],
            &Stroke::new(2.0),
            &StrokeOpts::default(),
            0.1,
        );
        let s = made(&path, FillRule::NonZero);
        // A mask of its box would be 9 MB.
        assert!(s.alpha.len() < 200_000, "{}", s.alpha.len());
    }

    #[test]
    fn the_rules_differ_where_outlines_overlap() {
        let mut path = rect(0.0, 0.0, 30.0, 30.0);
        path.extend(rect(10.0, 10.0, 20.0, 20.0).elements().iter().copied());
        let nonzero = made(&path, FillRule::NonZero);
        let evenodd = made(&path, FillRule::EvenOdd);
        assert_eq!(coverage_at(&nonzero, 15, 15), 255);
        assert_eq!(coverage_at(&evenodd, 15, 15), 0);
        assert_eq!(coverage_at(&evenodd, 5, 5), 255);
    }

    #[test]
    fn an_outline_the_other_way_makes_a_hole() {
        let mut path = rect(0.0, 0.0, 100.0, 100.0);
        path.extend(
            kurbo::Rect::new(20.0, 20.0, 80.0, 80.0)
                .into_path(0.1)
                .reverse_subpaths()
                .elements()
                .iter()
                .copied(),
        );
        let s = made(&path, FillRule::NonZero);
        assert_eq!(coverage_at(&s, 50, 50), 0);
        assert_eq!(coverage_at(&s, 10, 50), 255);
    }

    #[test]
    fn a_circle_has_the_area_of_a_circle() {
        let path = kurbo::Circle::new((100.0, 100.0), 80.0).into_path(0.01);
        let s = made(&path, FillRule::NonZero);
        let mut area = 0.0;
        for y in 0..200 {
            for x in 0..200 {
                area += coverage_at(&s, x, y) as f64 / 255.0;
            }
        }
        let want = std::f64::consts::PI * 80.0 * 80.0;
        assert!((area - want).abs() < want * 0.002, "{area} {want}");
    }

    #[test]
    fn what_is_left_of_what_is_made_still_counts() {
        let path = rect(-500.0, 0.0, 500.0, 10.0);
        let s = rasterize(
            &path,
            kurbo::Affine::IDENTITY,
            [0.0, 0.0, 100.0, 10.0],
            FillRule::NonZero,
            &[],
        );
        assert_eq!(coverage_at(&s, 50, 5), 255);
        assert_eq!(coverage_at(&s, 0, 5), 255);
        assert_eq!(coverage_at(&s, 99, 5), 255);
    }

    #[test]
    fn a_shape_is_cut_to_a_rectangle_at_any_angle() {
        let square = |x0: f64, y0: f64, x1: f64, y1: f64| {
            [
                Point::new(x0, y0),
                Point::new(x1, y0),
                Point::new(x1, y1),
                Point::new(x0, y1),
            ]
        };
        let path = rect(0.0, 0.0, 100.0, 100.0);
        let area = [-10.0, -10.0, 200.0, 200.0];
        let cut = rasterize(
            &path,
            kurbo::Affine::IDENTITY,
            area,
            FillRule::NonZero,
            &[square(10.0, 20.0, 30.0, 40.0)],
        );
        assert_eq!(coverage_at(&cut, 15, 25), 255);
        assert_eq!(coverage_at(&cut, 5, 25), 0);
        assert_eq!(coverage_at(&cut, 15, 45), 0);
        // The other way around makes no difference.
        let mut reversed = square(10.0, 20.0, 30.0, 40.0);
        reversed.reverse();
        let same = rasterize(
            &path,
            kurbo::Affine::IDENTITY,
            area,
            FillRule::NonZero,
            &[reversed],
        );
        assert_eq!(cut, same);
        // A diamond: half of the pixels on its edge.
        let diamond = [
            Point::new(50.0, 10.0),
            Point::new(90.0, 50.0),
            Point::new(50.0, 90.0),
            Point::new(10.0, 50.0),
        ];
        let cut = rasterize(
            &path,
            kurbo::Affine::IDENTITY,
            area,
            FillRule::NonZero,
            &[diamond],
        );
        assert_eq!(coverage_at(&cut, 50, 50), 255);
        assert_eq!(coverage_at(&cut, 12, 12), 0);
        let mut total = 0.0;
        for y in 0..100 {
            for x in 0..100 {
                total += coverage_at(&cut, x, y) as f64 / 255.0;
            }
        }
        assert!((total - 3200.0).abs() < 4.0, "{total}");
    }

    #[test]
    fn what_covers_nothing_makes_nothing() {
        assert_eq!(made(&BezPath::new(), FillRule::NonZero), Strips::default());
        let outside = [
            Point::new(50.0, 50.0),
            Point::new(60.0, 50.0),
            Point::new(60.0, 60.0),
            Point::new(50.0, 60.0),
        ];
        let cut = rasterize(
            &rect(0.0, 0.0, 10.0, 10.0),
            kurbo::Affine::IDENTITY,
            [-10.0, -10.0, 100.0, 100.0],
            FillRule::NonZero,
            &[outside],
        );
        assert_eq!(cut, Strips::default());
    }

    #[test]
    fn small_shapes_are_one_block() {
        let path = kurbo::Circle::new((12.0, 12.0), 10.0).into_path(0.1);
        let s = made(&path, FillRule::NonZero);
        assert!(s.runs.len() <= 3, "{:?}", s.runs.len());
    }
}
