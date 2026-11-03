//! The shapes of the loading indicator as tables of how far the outline is from the center, which
//! are easy to go between. The points are those of `MaterialShapes` in Jetpack Compose. See
//! `docs/design/15-md3e.md`.

use std::f32::consts::{PI, TAU};

/// How many directions a table has.
pub const SAMPLES: usize = 128;

type Point = [f32; 2];

/// A corner: where it is and the radius of its rounding.
#[derive(Clone, Copy)]
struct Corner {
    at: Point,
    radius: f32,
}

fn corner(x: f32, y: f32, radius: f32) -> Corner {
    Corner { at: [x, y], radius }
}

/// Repeats corners around the center of the unit square.
fn repeated(corners: &[Corner], times: usize) -> Vec<Corner> {
    (0..corners.len() * times)
        .map(|i| {
            let c = corners[i % corners.len()];
            let angle = (i / corners.len()) as f32 * TAU / times as f32;
            let (x, y) = (c.at[0] - 0.5, c.at[1] - 0.5);
            Corner {
                at: [
                    x * angle.cos() - y * angle.sin() + 0.5,
                    x * angle.sin() + y * angle.cos() + 0.5,
                ],
                radius: c.radius,
            }
        })
        .collect()
}

/// Repeats corners around the center, every other time mirrored.
fn mirrored(corners: &[Corner], times: usize) -> Vec<Corner> {
    let polar: Vec<(f32, f32)> = corners
        .iter()
        .map(|c| {
            let (x, y) = (c.at[0] - 0.5, c.at[1] - 0.5);
            (y.atan2(x), x.hypot(y))
        })
        .collect();
    let section = TAU / (times * 2) as f32;
    let mut out = Vec::new();
    for turn in 0..times * 2 {
        for index in 0..corners.len() {
            let i = if turn % 2 == 0 {
                index
            } else {
                corners.len() - 1 - index
            };
            if i > 0 || turn % 2 == 0 {
                let angle = section * turn as f32
                    + if turn % 2 == 0 {
                        polar[i].0
                    } else {
                        section - polar[i].0 + 2.0 * polar[0].0
                    };
                out.push(Corner {
                    at: [
                        angle.cos() * polar[i].1 + 0.5,
                        angle.sin() * polar[i].1 + 0.5,
                    ],
                    radius: corners[i].radius,
                });
            }
        }
    }
    out
}

/// A star around the origin with `points` tips at a distance of 1.
fn star(points: usize, inner: f32, radius: f32, turn: f32) -> Vec<Corner> {
    (0..points * 2)
        .map(|i| {
            let angle = i as f32 * PI / points as f32 + turn;
            let r = if i % 2 == 0 { 1.0 } else { inner };
            Corner {
                at: [angle.cos() * r, angle.sin() * r],
                radius,
            }
        })
        .collect()
}

/// The outline as line segments, with its corners rounded by arcs. Corners too close to each other
/// to take all of their rounding share what there is, as in Compose.
fn outline(corners: &[Corner]) -> Vec<Point> {
    let n = corners.len();
    let sub = |a: Point, b: Point| [a[0] - b[0], a[1] - b[1]];
    let length = |v: Point| v[0].hypot(v[1]);
    let geometry: Vec<(Point, Point, f32)> = (0..n)
        .map(|i| {
            let at = corners[i].at;
            let to_prev = sub(corners[(i + n - 1) % n].at, at);
            let to_next = sub(corners[(i + 1) % n].at, at);
            let (lp, ln) = (length(to_prev), length(to_next));
            let (up, un) = (
                [to_prev[0] / lp, to_prev[1] / lp],
                [to_next[0] / ln, to_next[1] / ln],
            );
            let cos = (up[0] * un[0] + up[1] * un[1]).clamp(-0.9999, 0.9999);
            // tan of half of the angle between the edges.
            let tan = ((1.0 - cos) / (1.0 + cos)).sqrt();
            (up, un, tan)
        })
        .collect();
    let wanted: Vec<f32> = (0..n).map(|i| corners[i].radius / geometry[i].2).collect();
    let mut out = Vec::new();
    for i in 0..n {
        let at = corners[i].at;
        let (up, un, tan) = geometry[i];
        let (prev, next) = ((i + n - 1) % n, (i + 1) % n);
        let room = |other: usize, j: usize| {
            let edge = length(sub(corners[other].at, corners[j].at));
            let sum = wanted[other] + wanted[j];
            if sum > edge { edge / sum } else { 1.0 }
        };
        let cut = wanted[i] * room(prev, i).min(room(next, i));
        if cut <= 1e-5 {
            out.push(at);
            continue;
        }
        let radius = cut * tan;
        let (start, end) = (
            [at[0] + up[0] * cut, at[1] + up[1] * cut],
            [at[0] + un[0] * cut, at[1] + un[1] * cut],
        );
        let bisector = [up[0] + un[0], up[1] + un[1]];
        let bl = length(bisector).max(1e-6);
        let reach = cut.hypot(radius);
        let center = [
            at[0] + bisector[0] / bl * reach,
            at[1] + bisector[1] / bl * reach,
        ];
        let a0 = (start[1] - center[1]).atan2(start[0] - center[0]);
        let a1 = (end[1] - center[1]).atan2(end[0] - center[0]);
        let mut sweep = a1 - a0;
        if sweep > PI {
            sweep -= TAU;
        } else if sweep < -PI {
            sweep += TAU;
        }
        for step in 0..=16 {
            let a = a0 + sweep * step as f32 / 16.0;
            out.push([center[0] + a.cos() * radius, center[1] + a.sin() * radius]);
        }
    }
    out
}

/// Fits an outline into a square of size 1 around the origin, as `normalized()` does in Compose.
fn normalized(mut points: Vec<Point>) -> Vec<Point> {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in &points {
        (x0, y0) = (x0.min(p[0]), y0.min(p[1]));
        (x1, y1) = (x1.max(p[0]), y1.max(p[1]));
    }
    let size = (x1 - x0).max(y1 - y0);
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    for p in &mut points {
        *p = [(p[0] - cx) / size, (p[1] - cy) / size];
    }
    points
}

/// The outlines of the indicator, in the order it goes through them.
fn outlines() -> [Vec<Point>; 7] {
    let soft_burst = repeated(
        &[corner(0.193, 0.277, 0.053), corner(0.176, 0.055, 0.053)],
        10,
    );
    let cookie_9 = star(9, 0.8, 0.5, -PI / 2.0);
    let pentagon = mirrored(
        &[
            corner(0.500, -0.009, 0.172),
            corner(1.030, 0.365, 0.164),
            corner(0.828, 0.970, 0.169),
        ],
        1,
    );
    let pill = mirrored(
        &[
            corner(0.961, 0.039, 0.426),
            corner(1.001, 0.428, 0.0),
            corner(1.000, 0.609, 1.0),
        ],
        2,
    );
    let sunny = star(8, 0.8, 0.15, 0.0);
    let cookie_4 = repeated(
        &[corner(1.237, 1.236, 0.258), corner(0.500, 0.918, 0.233)],
        4,
    );
    // An ellipse, which is what a circle squeezed to 0.64 and turned by -45 degrees is.
    let turn = -PI / 4.0;
    let oval = (0..256)
        .map(|i| {
            let a = i as f32 * TAU / 256.0;
            let (x, y) = (a.cos(), a.sin() * 0.64);
            [
                x * turn.cos() - y * turn.sin(),
                x * turn.sin() + y * turn.cos(),
            ]
        })
        .collect();
    [
        outline(&soft_burst),
        outline(&cookie_9),
        outline(&pentagon),
        outline(&pill),
        outline(&sunny),
        outline(&cookie_4),
        oval,
    ]
    .map(normalized)
}

/// Where a ray from the origin in direction `angle` crosses an outline, each time it does.
fn crossings(points: &[Point], angle: f32) -> Vec<f32> {
    let dir = [angle.cos(), angle.sin()];
    let mut hits = Vec::new();
    for i in 0..points.len() {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        let e = [b[0] - a[0], b[1] - a[1]];
        let det = dir[0] * e[1] - dir[1] * e[0];
        if det.abs() < 1e-9 {
            continue;
        }
        let t = (a[0] * e[1] - a[1] * e[0]) / det;
        let u = (a[0] * dir[1] - a[1] * dir[0]) / det;
        // A ray right through the point where two segments meet must not slip between them.
        if t > 0.0 && (-1e-4..=1.0 + 1e-4).contains(&u) {
            hits.push(t);
        }
    }
    hits
}

/// A shape as the distance to its outline in [`SAMPLES`] directions.
pub type Table = [f32; SAMPLES];

fn table(points: &[Point]) -> Table {
    std::array::from_fn(|i| {
        crossings(points, i as f32 * TAU / SAMPLES as f32)
            .into_iter()
            .fold(0.0, f32::max)
    })
}

/// The shapes, and the scale that keeps all of them inside the same box as they turn.
pub struct Shapes {
    pub tables: [Table; 7],
    pub fit: f32,
}

pub fn shapes() -> Shapes {
    let outlines = outlines();
    let fit = outlines
        .iter()
        .map(|o| {
            let furthest = o.iter().map(|p| p[0].hypot(p[1])).fold(0.0, f32::max);
            let (mut w, mut h) = ((f32::MAX, f32::MIN), (f32::MAX, f32::MIN));
            for p in o {
                w = (w.0.min(p[0]), w.1.max(p[0]));
                h = (h.0.min(p[1]), h.1.max(p[1]));
            }
            ((w.1 - w.0).max(h.1 - h.0) / (furthest * 2.0)).min(1.0)
        })
        .fold(1.0, f32::min);
    Shapes {
        tables: outlines.each_ref().map(|o| table(o)),
        fit,
    }
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
        for (i, outline) in outlines().iter().enumerate() {
            for s in 0..SAMPLES * 4 {
                let angle = (s as f32 + 0.37) * TAU / (SAMPLES * 4) as f32;
                let mut hits = crossings(outline, angle);
                // Where two segments meet it counts for both: they are the same crossing.
                hits.sort_by(f32::total_cmp);
                hits.dedup_by(|a, b| (*a - *b).abs() < 1e-3);
                assert_eq!(hits.len(), 1, "shape {i} at {angle}: {hits:?}");
            }
        }
    }

    #[test]
    fn no_direction_of_a_table_is_missing() {
        let s = shapes();
        for (i, t) in s.tables.iter().enumerate() {
            for (j, r) in t.iter().enumerate() {
                let next = t[(j + 1) % SAMPLES];
                assert!(*r > 0.1, "shape {i} at {j}: {r}");
                assert!((r - next).abs() < 0.1, "shape {i} at {j}: {r} then {next}");
            }
        }
    }

    #[test]
    fn outlines_fill_a_box_of_size_one() {
        for (i, outline) in outlines().iter().enumerate() {
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for p in outline {
                (x0, y0) = (x0.min(p[0]), y0.min(p[1]));
                (x1, y1) = (x1.max(p[0]), y1.max(p[1]));
            }
            let size = (x1 - x0).max(y1 - y0);
            assert!((size - 1.0).abs() < 1e-3, "shape {i}: {size}");
            assert!(
                (x0 + x1).abs() < 1e-3 && (y0 + y1).abs() < 1e-3,
                "shape {i}"
            );
        }
    }

    #[test]
    fn the_shapes_look_like_their_names() {
        let s = shapes();
        let ripples = |t: &Table| {
            (0..SAMPLES)
                .filter(|&i| {
                    let (a, b, c) = (t[(i + SAMPLES - 1) % SAMPLES], t[i], t[(i + 1) % SAMPLES]);
                    b >= a && b > c
                })
                .count()
        };
        // Soft burst, 9-sided cookie, pentagon, pill, sunny, 4-sided cookie, oval.
        assert_eq!(ripples(&s.tables[0]), 10);
        assert_eq!(ripples(&s.tables[1]), 9);
        assert_eq!(ripples(&s.tables[4]), 8);
        assert_eq!(ripples(&s.tables[5]), 4);
        assert_eq!(ripples(&s.tables[6]), 2);
    }

    #[test]
    fn they_stay_in_the_box_as_they_turn() {
        let s = shapes();
        assert!(s.fit > 0.5 && s.fit <= 1.0, "{}", s.fit);
        for t in &s.tables {
            let furthest = t.iter().copied().fold(0.0, f32::max);
            assert!(furthest * s.fit <= 0.5 + 1e-3, "{furthest} {}", s.fit);
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
