//! The shapes of `MaterialShapes` in Jetpack Compose, as tables of how far the outline is from a
//! point inside in each direction, which are easy to go between. The outlines are made the way
//! `RoundedPolygon` makes them, from the same points and roundings. See `docs/design/15-md3e.md`.

use std::cell::RefCell;
use std::f32::consts::{PI, TAU};
use std::rc::Rc;

/// How many directions a table has.
pub const SAMPLES: usize = 256;

type Point = [f32; 2];

/// A shape of the Material 3 Expressive catalog: `MaterialShapes` in Jetpack Compose, with the
/// same points and roundings.
///
/// All of the catalog is here but `Puffy`. A shape is kept as how far its outline is in each
/// direction from a point inside, which is what lets any of them change into any other; `Puffy`
/// is the one shape that no point sees all of the outline of, as its bumps overhang a little.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MaterialShape {
    Circle,
    Square,
    Slanted,
    Arch,
    Fan,
    Arrow,
    SemiCircle,
    Oval,
    Pill,
    Triangle,
    Diamond,
    ClamShell,
    Pentagon,
    Gem,
    Sunny,
    VerySunny,
    Cookie4Sided,
    Cookie6Sided,
    Cookie7Sided,
    Cookie9Sided,
    Cookie12Sided,
    Ghostish,
    Clover4Leaf,
    Clover8Leaf,
    Burst,
    SoftBurst,
    Boom,
    SoftBoom,
    Flower,
    PuffyDiamond,
    PixelCircle,
    PixelTriangle,
    Bun,
    Heart,
}

impl MaterialShape {
    /// All of them, in the order of the catalog.
    pub const ALL: [MaterialShape; 34] = {
        use MaterialShape::*;
        [
            Circle,
            Square,
            Slanted,
            Arch,
            Fan,
            Arrow,
            SemiCircle,
            Oval,
            Pill,
            Triangle,
            Diamond,
            ClamShell,
            Pentagon,
            Gem,
            Sunny,
            VerySunny,
            Cookie4Sided,
            Cookie6Sided,
            Cookie7Sided,
            Cookie9Sided,
            Cookie12Sided,
            Ghostish,
            Clover4Leaf,
            Clover8Leaf,
            Burst,
            SoftBurst,
            Boom,
            SoftBoom,
            Flower,
            PuffyDiamond,
            PixelCircle,
            PixelTriangle,
            Bun,
            Heart,
        ]
    };

    fn index(self) -> usize {
        self as usize
    }
}

/// How a corner is rounded: the radius of the arc, and how much further along the sides the
/// curve starts, from 0 (a plain arc) to 1 (twice as far). `CornerRounding` in Compose.
#[derive(Clone, Copy)]
struct Rounding {
    radius: f32,
    smoothing: f32,
}

const SHARP: Rounding = Rounding {
    radius: 0.0,
    smoothing: 0.0,
};

const fn round(radius: f32) -> Rounding {
    Rounding {
        radius,
        smoothing: 0.0,
    }
}

const fn smooth(radius: f32, smoothing: f32) -> Rounding {
    Rounding { radius, smoothing }
}

/// A corner: where it is and how it is rounded.
#[derive(Clone, Copy)]
struct Vertex {
    at: Point,
    rounding: Rounding,
}

const fn v(x: f32, y: f32, rounding: Rounding) -> Vertex {
    Vertex {
        at: [x, y],
        rounding,
    }
}

type Cubic = [Point; 4];

fn add(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1]]
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn mul(a: Point, k: f32) -> Point {
    [a[0] * k, a[1] * k]
}

fn dot(a: Point, b: Point) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: Point, b: Point) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}

fn length(a: Point) -> f32 {
    a[0].hypot(a[1])
}

fn direction(a: Point) -> Point {
    let l = length(a);
    if l > 0.0 { mul(a, 1.0 / l) } else { a }
}

fn rotate90(a: Point) -> Point {
    [-a[1], a[0]]
}

fn line(a: Point, b: Point) -> Cubic {
    let d = sub(b, a);
    [a, add(a, mul(d, 1.0 / 3.0)), add(a, mul(d, 2.0 / 3.0)), b]
}

/// A cubic close to the arc around `center` from `p0` to `p1`, as `Cubic.circularArc` makes it.
fn circular_arc(center: Point, p0: Point, p1: Point) -> Cubic {
    let d0 = direction(sub(p0, center));
    let d1 = direction(sub(p1, center));
    let (r0, r1) = (rotate90(d0), rotate90(d1));
    let clockwise = dot(r0, sub(p1, center)) >= 0.0;
    let cos = dot(d0, d1);
    if cos > 0.999 {
        return line(p0, p1);
    }
    let k = length(sub(p0, center)) * 4.0 / 3.0
        * ((2.0 * (1.0 - cos)).sqrt() - (1.0 - cos * cos).sqrt())
        / (1.0 - cos)
        * if clockwise { 1.0 } else { -1.0 };
    [p0, add(p0, mul(r0, k)), sub(p1, mul(r1, k)), p1]
}

/// The rounding of one corner, as `RoundedCorner` in Compose works it out.
struct Corner {
    p0: Point,
    p1: Point,
    p2: Point,
    d1: Point,
    d2: Point,
    radius: f32,
    smoothing: f32,
    /// How far along each side the arc starts.
    round_cut: f32,
}

impl Corner {
    fn new(p0: Point, p1: Point, p2: Point, rounding: Rounding) -> Self {
        let (v01, v21) = (sub(p0, p1), sub(p2, p1));
        let (l01, l21) = (length(v01), length(v21));
        if l01 <= 0.0 || l21 <= 0.0 {
            return Self {
                p0,
                p1,
                p2,
                d1: [0.0; 2],
                d2: [0.0; 2],
                radius: 0.0,
                smoothing: 0.0,
                round_cut: 0.0,
            };
        }
        let (d1, d2) = (mul(v01, 1.0 / l01), mul(v21, 1.0 / l21));
        let cos = dot(d1, d2);
        let sin = (1.0 - cos * cos).max(0.0).sqrt();
        let round_cut = if sin > 1e-3 {
            rounding.radius * (cos + 1.0) / sin
        } else {
            0.0
        };
        Self {
            p0,
            p1,
            p2,
            d1,
            d2,
            radius: rounding.radius,
            smoothing: rounding.smoothing,
            round_cut,
        }
    }

    /// How far along each side the whole curve starts, smoothing included.
    fn cut(&self) -> f32 {
        (1.0 + self.smoothing) * self.round_cut
    }

    fn smoothing_for(&self, allowed: f32) -> f32 {
        if allowed > self.cut() {
            self.smoothing
        } else if allowed > self.round_cut {
            self.smoothing * (allowed - self.round_cut) / (self.cut() - self.round_cut)
        } else {
            0.0
        }
    }

    fn cubics(&self, allowed0: f32, allowed1: f32) -> Vec<Cubic> {
        const EPSILON: f32 = 1e-4;
        let allowed = allowed0.min(allowed1);
        if self.round_cut < EPSILON || allowed < EPSILON || self.radius < EPSILON {
            return vec![[self.p1; 4]];
        }
        let round_cut = allowed.min(self.round_cut);
        let radius = self.radius * round_cut / self.round_cut;
        let reach = radius.hypot(round_cut);
        let center = add(self.p1, mul(direction(add(self.d1, self.d2)), reach));
        let hit0 = add(self.p1, mul(self.d1, round_cut));
        let hit2 = add(self.p1, mul(self.d2, round_cut));
        let arc = Arc {
            round_cut,
            center,
            radius,
        };
        let first = self.flank(&arc, self.smoothing_for(allowed0), self.p0, hit0, hit2);
        let last = self.flank(&arc, self.smoothing_for(allowed1), self.p2, hit2, hit0);
        let last = [last[3], last[2], last[1], last[0]];
        vec![first, circular_arc(center, first[3], last[0]), last]
    }

    /// The curve from a side onto the arc, `computeFlankingCurve` in Compose.
    fn flank(
        &self,
        arc: &Arc,
        smoothing: f32,
        side_start: Point,
        hit: Point,
        other_hit: Point,
    ) -> Cubic {
        let side = direction(sub(side_start, self.p1));
        let start = add(self.p1, mul(side, arc.round_cut * (1.0 + smoothing)));
        let middle = mul(add(hit, other_hit), 0.5);
        let p = add(hit, mul(sub(middle, hit), smoothing));
        let end = add(arc.center, mul(direction(sub(p, arc.center)), arc.radius));
        let tangent = rotate90(sub(end, arc.center));
        let anchor_end = line_intersection(side_start, side, end, tangent).unwrap_or(hit);
        let anchor_start = mul(add(start, mul(anchor_end, 2.0)), 1.0 / 3.0);
        [start, anchor_start, anchor_end, end]
    }
}

/// The arc of a rounded corner.
struct Arc {
    round_cut: f32,
    center: Point,
    radius: f32,
}

fn line_intersection(p0: Point, d0: Point, p1: Point, d1: Point) -> Option<Point> {
    const EPSILON: f32 = 1e-4;
    let rotated = rotate90(d1);
    let den = dot(d0, rotated);
    if den.abs() < EPSILON {
        return None;
    }
    let num = dot(sub(p1, p0), rotated);
    if den.abs() < EPSILON * num.abs() {
        return None;
    }
    Some(add(p0, mul(d0, num / den)))
}

/// An outline made of cubics.
struct Polygon {
    cubics: Vec<Cubic>,
}

/// `RoundedPolygon(vertices, perVertexRounding)`: the corners rounded as far as the sides leave
/// room for, rounding first and smoothing with what is left.
fn polygon(vertices: &[Vertex]) -> Polygon {
    let n = vertices.len();
    let corners: Vec<Corner> = (0..n)
        .map(|i| {
            Corner::new(
                vertices[(i + n - 1) % n].at,
                vertices[i].at,
                vertices[(i + 1) % n].at,
                vertices[i].rounding,
            )
        })
        .collect();
    // How much of the round cut, and of the rest of the cut, each side has room for.
    let room: Vec<(f32, f32)> = (0..n)
        .map(|i| {
            let (a, b) = (&corners[i], &corners[(i + 1) % n]);
            let round_cut = a.round_cut + b.round_cut;
            let cut = a.cut() + b.cut();
            let side = length(sub(vertices[i].at, vertices[(i + 1) % n].at));
            if round_cut > side {
                (side / round_cut, 0.0)
            } else if cut > side {
                (1.0, (side - round_cut) / (cut - round_cut))
            } else {
                (1.0, 1.0)
            }
        })
        .collect();
    let rounded: Vec<Vec<Cubic>> = (0..n)
        .map(|i| {
            let c = &corners[i];
            let allowed =
                |(round, rest): (f32, f32)| c.round_cut * round + (c.cut() - c.round_cut) * rest;
            c.cubics(allowed(room[(i + n - 1) % n]), allowed(room[i]))
        })
        .collect();
    let mut cubics = Vec::new();
    for i in 0..n {
        cubics.extend_from_slice(&rounded[i]);
        let from = rounded[i].last().expect("a corner")[3];
        let to = rounded[(i + 1) % n][0][0];
        cubics.push(line(from, to));
    }
    Polygon { cubics }
}

impl Polygon {
    fn map(mut self, f: impl Fn(Point) -> Point) -> Self {
        for c in &mut self.cubics {
            *c = c.map(&f);
        }
        self
    }

    /// Turns it around the origin like `rotateZ` in Compose: positive degrees go clockwise on a
    /// screen.
    fn rotated(self, degrees: f32) -> Self {
        let (sin, cos) = degrees.to_radians().sin_cos();
        self.map(|[x, y]| [x * cos - y * sin, x * sin + y * cos])
    }

    fn scaled(self, x: f32, y: f32) -> Self {
        self.map(|p| [p[0] * x, p[1] * y])
    }

    /// Fits it into a square of size 1 around the origin, as `normalized()` does in Compose,
    /// which measures the curves by their control points.
    fn normalized(self) -> Self {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in self.cubics.iter().flatten() {
            (x0, y0) = (x0.min(p[0]), y0.min(p[1]));
            (x1, y1) = (x1.max(p[0]), y1.max(p[1]));
        }
        let size = (x1 - x0).max(y1 - y0);
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        self.map(|p| [(p[0] - cx) / size, (p[1] - cy) / size])
    }

    /// The outline as line segments.
    fn points(&self) -> Vec<Point> {
        let mut out: Vec<Point> = Vec::new();
        for c in &self.cubics {
            let straight = cross(sub(c[1], c[0]), sub(c[3], c[0])).abs() < 1e-7
                && cross(sub(c[2], c[0]), sub(c[3], c[0])).abs() < 1e-7;
            let steps = if straight { 1 } else { 16 };
            for step in 0..steps {
                let t = step as f32 / steps as f32;
                let u = 1.0 - t;
                let p = add(
                    add(mul(c[0], u * u * u), mul(c[1], 3.0 * u * u * t)),
                    add(mul(c[2], 3.0 * u * t * t), mul(c[3], t * t * t)),
                );
                if out.last().is_none_or(|q| length(sub(*q, p)) > 1e-6) {
                    out.push(p);
                }
            }
        }
        if out.len() > 1 && length(sub(out[0], out[out.len() - 1])) <= 1e-6 {
            out.pop();
        }
        out
    }
}

/// `RoundedPolygon(numVertices, radius)`: a regular polygon with a corner at angle 0.
fn regular(roundings: &[Rounding], radius: f32) -> Polygon {
    let n = roundings.len();
    let vertices: Vec<Vertex> = (0..n)
        .map(|i| {
            let angle = TAU * i as f32 / n as f32;
            v(angle.cos() * radius, angle.sin() * radius, roundings[i])
        })
        .collect();
    polygon(&vertices)
}

/// `RoundedPolygon.circle`: a polygon whose rounded corners make a circle of radius 1.
fn circle(sides: usize) -> Polygon {
    regular(&vec![round(1.0); sides], 1.0 / (PI / sides as f32).cos())
}

/// `RoundedPolygon.rectangle`, starting at the bottom right corner.
fn rectangle(width: f32, height: f32, roundings: [Rounding; 4]) -> Polygon {
    let (x, y) = (width / 2.0, height / 2.0);
    polygon(&[
        v(x, y, roundings[0]),
        v(-x, y, roundings[1]),
        v(-x, -y, roundings[2]),
        v(x, -y, roundings[3]),
    ])
}

/// `RoundedPolygon.star`: tips at a distance of 1, the first at angle 0.
fn star(tips: usize, inner: f32, rounding: Rounding) -> Polygon {
    let vertices: Vec<Vertex> = (0..tips * 2)
        .map(|i| {
            let angle = i as f32 * PI / tips as f32;
            let r = if i % 2 == 0 { 1.0 } else { inner };
            v(angle.cos() * r, angle.sin() * r, rounding)
        })
        .collect();
    polygon(&vertices)
}

/// `customPolygon` of `MaterialShapes`: the points repeated around the middle of the unit square,
/// every other time mirrored if `mirroring`.
fn custom(points: &[Vertex], reps: usize, mirroring: bool) -> Polygon {
    let center = [0.5, 0.5];
    let mut out = Vec::new();
    if mirroring {
        let polar: Vec<(f32, f32)> = points
            .iter()
            .map(|p| {
                let d = sub(p.at, center);
                (d[1].atan2(d[0]), length(d))
            })
            .collect();
        let section = TAU / (reps * 2) as f32;
        for turn in 0..reps * 2 {
            for index in 0..points.len() {
                let i = if turn % 2 == 0 {
                    index
                } else {
                    points.len() - 1 - index
                };
                if i > 0 || turn % 2 == 0 {
                    let angle = section * turn as f32
                        + if turn % 2 == 0 {
                            polar[i].0
                        } else {
                            section - polar[i].0 + 2.0 * polar[0].0
                        };
                    out.push(Vertex {
                        at: add(mul([angle.cos(), angle.sin()], polar[i].1), center),
                        rounding: points[i].rounding,
                    });
                }
            }
        }
    } else {
        for i in 0..points.len() * reps {
            let p = points[i % points.len()];
            let angle = (i / points.len()) as f32 * TAU / reps as f32;
            let (sin, cos) = angle.sin_cos();
            let d = sub(p.at, center);
            out.push(Vertex {
                at: add([d[0] * cos - d[1] * sin, d[0] * sin + d[1] * cos], center),
                rounding: p.rounding,
            });
        }
    }
    polygon(&out)
}

/// The outline of a shape before it is normalized: `MaterialShapes.kt`, value for value.
fn made(shape: MaterialShape) -> Polygon {
    use MaterialShape::*;
    match shape {
        Circle => circle(10),
        Square => rectangle(1.0, 1.0, [round(0.3); 4]),
        Slanted => custom(
            &[
                v(0.926, 0.970, smooth(0.189, 0.811)),
                v(-0.021, 0.967, smooth(0.187, 0.057)),
            ],
            2,
            false,
        ),
        Arch => regular(&[round(1.0), round(1.0), round(0.2), round(0.2)], 1.0).rotated(-135.0),
        Fan => custom(
            &[
                v(1.004, 1.000, smooth(0.148, 0.417)),
                v(0.000, 1.000, round(0.151)),
                v(0.000, -0.003, round(0.148)),
                v(0.978, 0.020, round(0.803)),
            ],
            1,
            false,
        ),
        Arrow => custom(
            &[
                v(0.500, 0.892, round(0.313)),
                v(-0.216, 1.050, round(0.207)),
                v(0.499, -0.160, smooth(0.215, 1.000)),
                v(1.225, 1.060, round(0.211)),
            ],
            1,
            false,
        ),
        SemiCircle => rectangle(1.6, 1.0, [round(0.2), round(0.2), round(1.0), round(1.0)]),
        Oval => circle(8).scaled(1.0, 0.64).rotated(-45.0),
        Pill => custom(
            &[
                v(0.961, 0.039, round(0.426)),
                v(1.001, 0.428, SHARP),
                v(1.000, 0.609, round(1.000)),
            ],
            2,
            true,
        ),
        Triangle => regular(&[round(0.2); 3], 1.0).rotated(-90.0),
        Diamond => custom(
            &[
                v(0.500, 1.096, smooth(0.151, 0.524)),
                v(0.040, 0.500, round(0.159)),
            ],
            2,
            false,
        ),
        ClamShell => custom(
            &[
                v(0.171, 0.841, round(0.159)),
                v(-0.020, 0.500, round(0.140)),
                v(0.170, 0.159, round(0.159)),
            ],
            2,
            false,
        ),
        Pentagon => custom(
            &[
                v(0.500, -0.009, round(0.172)),
                v(1.030, 0.365, round(0.164)),
                v(0.828, 0.970, round(0.169)),
            ],
            1,
            true,
        ),
        Gem => custom(
            &[
                v(0.499, 1.023, smooth(0.241, 0.778)),
                v(-0.005, 0.792, round(0.208)),
                v(0.073, 0.258, round(0.228)),
                v(0.433, -0.000, round(0.491)),
            ],
            1,
            true,
        ),
        Sunny => star(8, 0.8, round(0.15)),
        VerySunny => custom(
            &[v(0.500, 1.080, round(0.085)), v(0.358, 0.843, round(0.085))],
            8,
            false,
        ),
        Cookie4Sided => custom(
            &[v(1.237, 1.236, round(0.258)), v(0.500, 0.918, round(0.233))],
            4,
            false,
        ),
        Cookie6Sided => custom(
            &[v(0.723, 0.884, round(0.394)), v(0.500, 1.099, round(0.398))],
            6,
            false,
        ),
        Cookie7Sided => star(7, 0.75, round(0.5)).rotated(-90.0),
        Cookie9Sided => star(9, 0.8, round(0.5)).rotated(-90.0),
        Cookie12Sided => star(12, 0.8, round(0.5)).rotated(-90.0),
        Ghostish => custom(
            &[
                v(0.500, 0.0, round(1.000)),
                v(1.0, 0.0, round(1.000)),
                v(1.0, 1.140, smooth(0.254, 0.106)),
                v(0.575, 0.906, round(0.253)),
            ],
            1,
            true,
        ),
        Clover4Leaf => custom(
            &[v(0.500, 0.074, SHARP), v(0.725, -0.099, round(0.476))],
            4,
            true,
        ),
        Clover8Leaf => custom(
            &[v(0.500, 0.036, SHARP), v(0.758, -0.101, round(0.209))],
            8,
            false,
        ),
        Burst => custom(
            &[
                v(0.500, -0.006, round(0.006)),
                v(0.592, 0.158, round(0.006)),
            ],
            12,
            false,
        ),
        SoftBurst => custom(
            &[v(0.193, 0.277, round(0.053)), v(0.176, 0.055, round(0.053))],
            10,
            false,
        ),
        Boom => custom(
            &[
                v(0.457, 0.296, round(0.007)),
                v(0.500, -0.051, round(0.007)),
            ],
            15,
            false,
        ),
        SoftBoom => custom(
            &[
                v(0.733, 0.454, SHARP),
                v(0.839, 0.437, round(0.532)),
                v(0.949, 0.449, smooth(0.439, 1.000)),
                v(0.998, 0.478, round(0.174)),
            ],
            16,
            true,
        ),
        Flower => custom(
            &[
                v(0.370, 0.187, SHARP),
                v(0.416, 0.049, round(0.381)),
                v(0.479, 0.001, round(0.095)),
            ],
            8,
            true,
        ),
        PuffyDiamond => custom(
            &[
                v(0.870, 0.130, round(0.146)),
                v(0.818, 0.357, SHARP),
                v(1.000, 0.332, round(0.853)),
            ],
            4,
            true,
        ),
        PixelCircle => custom(
            &[
                v(0.500, 0.000, SHARP),
                v(0.704, 0.000, SHARP),
                v(0.704, 0.065, SHARP),
                v(0.843, 0.065, SHARP),
                v(0.843, 0.148, SHARP),
                v(0.926, 0.148, SHARP),
                v(0.926, 0.296, SHARP),
                v(1.000, 0.296, SHARP),
            ],
            2,
            true,
        ),
        PixelTriangle => custom(
            &[
                v(0.110, 0.500, SHARP),
                v(0.113, 0.000, SHARP),
                v(0.287, 0.000, SHARP),
                v(0.287, 0.087, SHARP),
                v(0.421, 0.087, SHARP),
                v(0.421, 0.170, SHARP),
                v(0.560, 0.170, SHARP),
                v(0.560, 0.265, SHARP),
                v(0.674, 0.265, SHARP),
                v(0.675, 0.344, SHARP),
                v(0.789, 0.344, SHARP),
                v(0.789, 0.439, SHARP),
                v(0.888, 0.439, SHARP),
            ],
            1,
            true,
        ),
        Bun => custom(
            &[
                v(0.796, 0.500, SHARP),
                v(0.853, 0.518, round(1.0)),
                v(0.992, 0.631, round(1.0)),
                v(0.968, 1.000, round(1.0)),
            ],
            2,
            true,
        ),
        Heart => custom(
            &[
                v(0.500, 0.268, round(0.016)),
                v(0.792, -0.066, round(0.958)),
                v(1.064, 0.276, round(1.000)),
                v(0.501, 0.946, round(0.129)),
            ],
            1,
            true,
        ),
    }
}

/// The outline of a shape in a box of size 1 around the origin, as line segments.
pub fn outline(shape: MaterialShape) -> Vec<Point> {
    made(shape).normalized().points()
}

/// Where a ray from `from` in direction `angle` crosses an outline, each time it does.
pub fn crossings(points: &[Point], from: Point, angle: f32) -> Vec<f32> {
    let dir = [angle.cos(), angle.sin()];
    let mut hits = Vec::new();
    for i in 0..points.len() {
        let a = sub(points[i], from);
        let b = sub(points[(i + 1) % points.len()], from);
        let e = sub(b, a);
        let det = cross(dir, e);
        if det.abs() < 1e-9 {
            continue;
        }
        let t = cross(a, e) / det;
        let u = cross(a, dir) / det;
        // A ray right through the point where two segments meet must not slip between them.
        if t > 0.0 && (-1e-4..=1.0 + 1e-4).contains(&u) {
            hits.push(t);
        }
    }
    hits
}

/// How well all of an outline is seen from a point: the least sine of the angle between a ray
/// from it and the outline where they meet. Above 0, every ray meets the outline once.
fn visibility(points: &[Point], from: Point) -> f32 {
    let n = points.len();
    let area: f32 = (0..n).map(|i| cross(points[i], points[(i + 1) % n])).sum();
    let turn = area.signum();
    (0..n)
        .filter_map(|i| {
            let a = sub(points[i], from);
            let e = sub(points[(i + 1) % n], points[i]);
            let (la, le) = (length(a), length(e));
            (la > 1e-6 && le > 1e-6).then(|| turn * cross(a, e) / (la * le))
        })
        .fold(f32::MAX, f32::min)
}

/// The point the table of an outline is made around: the middle of its box if all of it is seen
/// well from there, or else the point that sees it best.
fn origin_of(points: &[Point]) -> Point {
    if visibility(points, [0.0; 2]) > 0.02 {
        return [0.0; 2];
    }
    let mut best = ([0.0; 2], f32::MIN);
    for i in 0..=40 {
        for j in 0..=40 {
            let p = [i as f32 / 40.0 - 0.5, j as f32 / 40.0 - 0.5];
            let score = visibility(points, p);
            if score > best.1 {
                best = (p, score);
            }
        }
    }
    best.0
}

/// A shape as the distance to its outline in [`SAMPLES`] directions from `origin`, in a box of
/// size 1 around (0, 0).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Table {
    pub radii: [f32; SAMPLES],
    pub origin: Point,
}

impl Table {
    /// Part of the way from this to `other`, or further if `t` is beyond 1.
    pub fn mix(&self, other: &Table, t: f32) -> Table {
        Table {
            radii: std::array::from_fn(|i| {
                (self.radii[i] + (other.radii[i] - self.radii[i]) * t).max(0.0)
            }),
            origin: std::array::from_fn(|i| {
                self.origin[i] + (other.origin[i] - self.origin[i]) * t
            }),
        }
    }

    /// The radii four at a time, as the shader takes them.
    pub fn words(&self) -> [[f32; 4]; SAMPLES / 4] {
        std::array::from_fn(|i| std::array::from_fn(|j| self.radii[i * 4 + j]))
    }
}

/// Each segment fills in the directions it is seen in, so that it takes one walk around the
/// outline.
fn table_of(points: &[Point]) -> Table {
    let origin = origin_of(points);
    let step = TAU / SAMPLES as f32;
    let mut radii = [0.0f32; SAMPLES];
    for i in 0..points.len() {
        let a = sub(points[i], origin);
        let b = sub(points[(i + 1) % points.len()], origin);
        let e = sub(b, a);
        let from = a[1].atan2(a[0]);
        let mut sweep = b[1].atan2(b[0]) - from;
        if sweep > PI {
            sweep -= TAU;
        } else if sweep < -PI {
            sweep += TAU;
        }
        let (lo, hi) = (from.min(from + sweep), from.max(from + sweep));
        for k in (lo / step).ceil() as i32..=(hi / step).floor() as i32 {
            let angle = k as f32 * step;
            let det = cross([angle.cos(), angle.sin()], e);
            if det.abs() < 1e-9 {
                continue;
            }
            let r = &mut radii[k.rem_euclid(SAMPLES as i32) as usize];
            *r = r.max(cross(a, e) / det);
        }
    }
    // A direction that fell between two segments, which only a ray right through a corner can.
    for (k, r) in radii.iter_mut().enumerate() {
        if *r <= 0.0 {
            *r = crossings(points, origin, k as f32 * step)
                .into_iter()
                .fold(0.0, f32::max);
        }
    }
    Table { radii, origin }
}

const COUNT: usize = MaterialShape::ALL.len();

thread_local! {
    static TABLES: RefCell<[Option<Rc<Table>>; COUNT]> = const { RefCell::new([const { None }; COUNT]) };
}

/// The table of a shape, worked out the first time it is asked for on this thread.
pub fn table(shape: MaterialShape) -> Rc<Table> {
    if let Some(t) = TABLES.with(|t| t.borrow()[shape.index()].clone()) {
        return t;
    }
    let t = Rc::new(table_of(&outline(shape)));
    TABLES.with(|tables| tables.borrow_mut()[shape.index()] = Some(t.clone()));
    t
}

/// The size, as a part of the box, that keeps all of these shapes inside it as they turn around
/// its middle.
pub fn fit(shapes: &[MaterialShape]) -> f32 {
    shapes
        .iter()
        .map(|s| {
            let furthest = outline(*s).iter().map(|p| length(*p)).fold(0.0, f32::max);
            (0.5 / furthest).min(1.0)
        })
        .fold(1.0, f32::min)
}

/// How far a spring of stiffness 200 and damping ratio 0.6 has got from 0 towards 1.
pub fn morph_spring(t: f32) -> f32 {
    let omega = 200.0f32.sqrt();
    let zeta = 0.6;
    let damped = omega * (1.0f32 - zeta * zeta).sqrt();
    1.0 - (-zeta * omega * t).exp()
        * ((damped * t).cos() + zeta * omega / damped * (damped * t).sin())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_direction_meets_every_outline_once() {
        let mut wrong = Vec::new();
        for shape in MaterialShape::ALL {
            let outline = outline(shape);
            let origin = table(shape).origin;
            for s in 0..SAMPLES * 4 {
                let angle = (s as f32 + 0.37) * TAU / (SAMPLES * 4) as f32;
                let mut hits = crossings(&outline, origin, angle);
                // Where two segments meet it counts for both: they are the same crossing.
                hits.sort_by(f32::total_cmp);
                hits.dedup_by(|a, b| (*a - *b).abs() < 1e-3);
                if hits.len() != 1 {
                    wrong.push(format!("{shape:?} from {origin:?} at {angle}: {hits:?}"));
                    break;
                }
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[test]
    fn no_direction_of_a_table_is_missing() {
        for shape in MaterialShape::ALL {
            for (j, r) in table(shape).radii.iter().enumerate() {
                assert!(*r > 0.02, "{shape:?} at {j}: {r}");
            }
        }
    }

    #[test]
    fn a_table_is_where_the_outline_is() {
        for shape in MaterialShape::ALL {
            let outline = outline(shape);
            let t = table(shape);
            for (j, r) in t.radii.iter().enumerate() {
                let angle = j as f32 * TAU / SAMPLES as f32;
                let far = crossings(&outline, t.origin, angle)
                    .into_iter()
                    .fold(0.0, f32::max);
                assert!((r - far).abs() < 1e-3, "{shape:?} at {j}: {r} {far}");
            }
        }
    }

    #[test]
    fn outlines_fill_a_box_of_size_one() {
        for shape in MaterialShape::ALL {
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for p in outline(shape) {
                (x0, y0) = (x0.min(p[0]), y0.min(p[1]));
                (x1, y1) = (x1.max(p[0]), y1.max(p[1]));
            }
            // Compose measures the curves by their control points, which may be a little
            // outside of them.
            let size = (x1 - x0).max(y1 - y0);
            assert!((0.85..=1.0 + 1e-3).contains(&size), "{shape:?}: {size}");
            assert!(x0 >= -0.5 - 1e-3 && x1 <= 0.5 + 1e-3, "{shape:?}");
            assert!(y0 >= -0.5 - 1e-3 && y1 <= 0.5 + 1e-3, "{shape:?}");
        }
    }

    #[test]
    fn the_shapes_look_like_their_names() {
        use MaterialShape::*;
        // Bumps of the table of a shape that is symmetric about the middle.
        let bumps = |s: MaterialShape| {
            let t = table(s);
            assert_eq!(t.origin, [0.0; 2], "{s:?}");
            let r = &t.radii;
            let least = r.iter().copied().fold(f32::MAX, f32::min);
            (0..SAMPLES)
                .filter(|&i| {
                    let (a, b, c) = (r[(i + SAMPLES - 1) % SAMPLES], r[i], r[(i + 1) % SAMPLES]);
                    b >= a && b > c && b - least > 0.01
                })
                .count()
        };
        for (shape, count) in [
            (SoftBurst, 10),
            (Cookie4Sided, 4),
            (Cookie6Sided, 6),
            (Cookie7Sided, 7),
            (Cookie9Sided, 9),
            (Cookie12Sided, 12),
            (Sunny, 8),
            (VerySunny, 8),
            (Burst, 12),
            (Boom, 15),
            (Clover4Leaf, 4),
            (Clover8Leaf, 8),
            (Oval, 2),
            (Square, 4),
        ] {
            assert_eq!(bumps(shape), count, "{shape:?}");
        }
        // A circle is round. Its control points are a little outside of it, so it is a little
        // smaller than its box, as it is in Compose.
        let circle = table(Circle).radii;
        for r in circle {
            assert!((r - circle[0]).abs() < 1e-3 && r > 0.48, "{r}");
        }
        // The triangle points up, and it and the arch are flat at the bottom.
        let near = |s: MaterialShape, y: f32| -> Vec<f32> {
            outline(s)
                .iter()
                .filter(|p| (p[1] - y).abs() < 0.03)
                .map(|p| p[0].abs())
                .collect()
        };
        let triangle = outline(Triangle);
        let top = triangle.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
        assert!(near(Triangle, top).iter().all(|x| *x < 0.1));
        let bottom = triangle.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
        assert!(near(Triangle, bottom).iter().any(|x| *x > 0.25));
        assert!(near(Arch, 0.49).iter().any(|x| *x > 0.25));
        // The heart has its notch at the top.
        let notch = outline(Heart)
            .iter()
            .filter(|p| p[0].abs() < 0.01)
            .map(|p| p[1])
            .fold(f32::MAX, f32::min);
        assert!(notch > -0.3, "{notch}");
    }

    #[test]
    fn rounding_without_smoothing_is_a_plain_arc() {
        // A square of side 2 with corners of radius 0.5: they are quarters of a circle.
        let points = rectangle(2.0, 2.0, [round(0.5); 4]).points();
        let on_arc: Vec<f32> = points
            .iter()
            .filter(|q| q[0] > 0.5 && q[1] > 0.5)
            .map(|q| length(sub(*q, [0.5, 0.5])))
            .collect();
        assert!(!on_arc.is_empty());
        for d in on_arc {
            assert!((d - 0.5).abs() < 2e-3, "{d}");
        }
    }

    #[test]
    fn mixing_goes_from_one_to_the_other() {
        let (a, b) = (table(MaterialShape::Circle), table(MaterialShape::Heart));
        let close = |x: &Table, y: &Table| {
            x.radii
                .iter()
                .zip(y.radii)
                .all(|(p, q)| (p - q).abs() < 1e-5)
        };
        assert!(close(&a.mix(&b, 0.0), &a));
        assert!(close(&a.mix(&b, 1.0), &b));
        let half = a.mix(&b, 0.5);
        assert!((half.radii[3] - (a.radii[3] + b.radii[3]) / 2.0).abs() < 1e-6);
    }

    #[test]
    fn they_stay_in_the_box_as_they_turn() {
        let shapes = [MaterialShape::Square, MaterialShape::Pill];
        let fit = fit(&shapes);
        assert!(fit > 0.5 && fit < 1.0, "{fit}");
        for s in shapes {
            for p in outline(s) {
                assert!(length(p) * fit <= 0.5 + 1e-4);
            }
        }
    }

    #[test]
    fn the_spring_overshoots_a_little_and_settles() {
        assert_eq!(morph_spring(0.0), 0.0);
        let most = (0..650)
            .map(|ms| morph_spring(ms as f32 / 1000.0))
            .fold(0.0, f32::max);
        assert!((1.05..1.15).contains(&most), "{most}");
        assert!((morph_spring(0.65) - 1.0).abs() < 0.01);
    }
}
