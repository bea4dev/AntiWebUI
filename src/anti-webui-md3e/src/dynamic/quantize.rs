//! Reducing an image to a few colors: Wu's quantizer gives the starting clusters, and weighted
//! k-means in L*a*b* refines them, as M. Emre Celebi proposed. From `quantize/quantizer_wu.ts`,
//! `quantize/quantizer_wsmeans.ts` and `quantize/quantizer_celebi.ts`.

use super::utils;
use std::collections::HashMap;

/// The colors of `pixels` (opaque ARGB) as at most `max_colors` colors, each with how many pixels
/// it stands for, in a fixed order.
pub(crate) fn celebi(pixels: &[u32], max_colors: usize) -> Vec<(u32, u32)> {
    let wu_result = wu(pixels, max_colors);
    wsmeans(pixels, &wu_result, max_colors)
}

const INDEX_BITS: u32 = 5;
const SIDE_LENGTH: usize = 33;
const TOTAL_SIZE: usize = 35937;

#[derive(Clone, Copy, Default)]
struct Cube {
    r0: usize,
    r1: usize,
    g0: usize,
    g1: usize,
    b0: usize,
    b1: usize,
    vol: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum Direction {
    Red,
    Green,
    Blue,
}

fn index(r: usize, g: usize, b: usize) -> usize {
    (r << (INDEX_BITS * 2)) + (r << (INDEX_BITS + 1)) + r + (g << INDEX_BITS) + g + b
}

/// Wu's quantizer: cuts the RGB cube into boxes of least variance.
struct Wu {
    weights: Vec<f64>,
    moments_r: Vec<f64>,
    moments_g: Vec<f64>,
    moments_b: Vec<f64>,
    moments: Vec<f64>,
    cubes: Vec<Cube>,
}

pub(crate) fn wu(pixels: &[u32], max_colors: usize) -> Vec<u32> {
    if max_colors == 0 {
        return Vec::new();
    }
    let mut wu = Wu {
        weights: vec![0.0; TOTAL_SIZE],
        moments_r: vec![0.0; TOTAL_SIZE],
        moments_g: vec![0.0; TOTAL_SIZE],
        moments_b: vec![0.0; TOTAL_SIZE],
        moments: vec![0.0; TOTAL_SIZE],
        cubes: Vec::new(),
    };
    wu.construct_histogram(pixels);
    wu.compute_moments();
    let count = wu.create_boxes(max_colors);
    wu.create_result(count)
}

impl Wu {
    fn construct_histogram(&mut self, pixels: &[u32]) {
        let mut count_by_color = HashMap::new();
        for &pixel in pixels {
            if utils::alpha_from_argb(pixel) < 255 {
                continue;
            }
            *count_by_color.entry(pixel).or_insert(0u32) += 1;
        }
        let bits_to_remove = 8 - INDEX_BITS;
        for (pixel, count) in count_by_color {
            let red = utils::red_from_argb(pixel) as usize;
            let green = utils::green_from_argb(pixel) as usize;
            let blue = utils::blue_from_argb(pixel) as usize;
            let i = index(
                (red >> bits_to_remove) + 1,
                (green >> bits_to_remove) + 1,
                (blue >> bits_to_remove) + 1,
            );
            let count = count as f64;
            self.weights[i] += count;
            self.moments_r[i] += count * red as f64;
            self.moments_g[i] += count * green as f64;
            self.moments_b[i] += count * blue as f64;
            self.moments[i] += count * (red * red + green * green + blue * blue) as f64;
        }
    }

    /// Turns the histogram into cumulative moments, so that any box sums in constant time.
    fn compute_moments(&mut self) {
        for r in 1..SIDE_LENGTH {
            let mut area = [0.0; SIDE_LENGTH];
            let mut area_r = [0.0; SIDE_LENGTH];
            let mut area_g = [0.0; SIDE_LENGTH];
            let mut area_b = [0.0; SIDE_LENGTH];
            let mut area2 = [0.0; SIDE_LENGTH];
            for g in 1..SIDE_LENGTH {
                let (mut line, mut line_r, mut line_g, mut line_b, mut line2) =
                    (0.0, 0.0, 0.0, 0.0, 0.0);
                for b in 1..SIDE_LENGTH {
                    let i = index(r, g, b);
                    line += self.weights[i];
                    line_r += self.moments_r[i];
                    line_g += self.moments_g[i];
                    line_b += self.moments_b[i];
                    line2 += self.moments[i];

                    area[b] += line;
                    area_r[b] += line_r;
                    area_g[b] += line_g;
                    area_b[b] += line_b;
                    area2[b] += line2;

                    let previous = index(r - 1, g, b);
                    self.weights[i] = self.weights[previous] + area[b];
                    self.moments_r[i] = self.moments_r[previous] + area_r[b];
                    self.moments_g[i] = self.moments_g[previous] + area_g[b];
                    self.moments_b[i] = self.moments_b[previous] + area_b[b];
                    self.moments[i] = self.moments[previous] + area2[b];
                }
            }
        }
    }

    /// Cuts until there are `max_colors` boxes or none can be cut. Returns how many there are.
    fn create_boxes(&mut self, max_colors: usize) -> usize {
        self.cubes = vec![Cube::default(); max_colors];
        let mut volume_variance = vec![0.0; max_colors];
        self.cubes[0] = Cube {
            r1: SIDE_LENGTH - 1,
            g1: SIDE_LENGTH - 1,
            b1: SIDE_LENGTH - 1,
            ..Cube::default()
        };

        let mut generated_color_count = max_colors;
        let mut next = 0;
        let mut i = 1;
        while i < max_colors {
            let (mut one, mut two) = (self.cubes[next], self.cubes[i]);
            if self.cut(&mut one, &mut two) {
                self.cubes[next] = one;
                self.cubes[i] = two;
                volume_variance[next] = if one.vol > 1 {
                    self.variance(&one)
                } else {
                    0.0
                };
                volume_variance[i] = if two.vol > 1 {
                    self.variance(&two)
                } else {
                    0.0
                };
            } else {
                volume_variance[next] = 0.0;
                i -= 1;
            }

            next = 0;
            let mut temp = volume_variance[0];
            for (j, &v) in volume_variance.iter().enumerate().take(i + 1).skip(1) {
                if v > temp {
                    temp = v;
                    next = j;
                }
            }
            if temp <= 0.0 {
                generated_color_count = i + 1;
                break;
            }
            i += 1;
        }
        generated_color_count
    }

    fn create_result(&self, color_count: usize) -> Vec<u32> {
        let mut colors = Vec::new();
        for cube in &self.cubes[..color_count] {
            let weight = volume(cube, &self.weights);
            if weight > 0.0 {
                let r = (volume(cube, &self.moments_r) / weight).round() as u32;
                let g = (volume(cube, &self.moments_g) / weight).round() as u32;
                let b = (volume(cube, &self.moments_b) / weight).round() as u32;
                colors.push(0xff00_0000 | (r & 0xff) << 16 | (g & 0xff) << 8 | (b & 0xff));
            }
        }
        colors
    }

    fn variance(&self, cube: &Cube) -> f64 {
        let dr = volume(cube, &self.moments_r);
        let dg = volume(cube, &self.moments_g);
        let db = volume(cube, &self.moments_b);
        let xx = volume(cube, &self.moments);
        let hypotenuse = dr * dr + dg * dg + db * db;
        let volume = volume(cube, &self.weights);
        xx - hypotenuse / volume
    }

    fn cut(&self, one: &mut Cube, two: &mut Cube) -> bool {
        let whole_r = volume(one, &self.moments_r);
        let whole_g = volume(one, &self.moments_g);
        let whole_b = volume(one, &self.moments_b);
        let whole_w = volume(one, &self.weights);
        let whole = [whole_r, whole_g, whole_b, whole_w];

        let (cut_r, max_r) = self.maximize(one, Direction::Red, one.r0 + 1, one.r1, whole);
        let (cut_g, max_g) = self.maximize(one, Direction::Green, one.g0 + 1, one.g1, whole);
        let (cut_b, max_b) = self.maximize(one, Direction::Blue, one.b0 + 1, one.b1, whole);

        let direction = if max_r >= max_g && max_r >= max_b {
            if cut_r.is_none() {
                return false;
            }
            Direction::Red
        } else if max_g >= max_r && max_g >= max_b {
            Direction::Green
        } else {
            Direction::Blue
        };

        two.r1 = one.r1;
        two.g1 = one.g1;
        two.b1 = one.b1;

        // A maximum above 0 always comes with a cut.
        match direction {
            Direction::Red => {
                one.r1 = cut_r.unwrap_or(one.r1);
                two.r0 = one.r1;
                two.g0 = one.g0;
                two.b0 = one.b0;
            }
            Direction::Green => {
                one.g1 = cut_g.unwrap_or(one.g1);
                two.r0 = one.r0;
                two.g0 = one.g1;
                two.b0 = one.b0;
            }
            Direction::Blue => {
                one.b1 = cut_b.unwrap_or(one.b1);
                two.r0 = one.r0;
                two.g0 = one.g0;
                two.b0 = one.b1;
            }
        }

        one.vol = (one.r1 - one.r0) * (one.g1 - one.g0) * (one.b1 - one.b0);
        two.vol = (two.r1 - two.r0) * (two.g1 - two.g0) * (two.b1 - two.b0);
        true
    }

    /// Where along `direction` cutting `cube` leaves the least variance, and how much less.
    fn maximize(
        &self,
        cube: &Cube,
        direction: Direction,
        first: usize,
        last: usize,
        whole: [f64; 4],
    ) -> (Option<usize>, f64) {
        let moments = [
            &self.moments_r,
            &self.moments_g,
            &self.moments_b,
            &self.weights,
        ];
        let bottoms = moments.map(|m| bottom(cube, direction, m));
        let mut max = 0.0;
        let mut cut = None;
        for i in first..last {
            let mut half: [f64; 4] =
                std::array::from_fn(|k| bottoms[k] + top(cube, direction, i, moments[k]));
            if half[3] == 0.0 {
                continue;
            }
            let mut temp = (half[0] * half[0] + half[1] * half[1] + half[2] * half[2]) / half[3];

            half = std::array::from_fn(|k| whole[k] - half[k]);
            if half[3] == 0.0 {
                continue;
            }
            temp += (half[0] * half[0] + half[1] * half[1] + half[2] * half[2]) / half[3];

            if temp > max {
                max = temp;
                cut = Some(i);
            }
        }
        (cut, max)
    }
}

fn volume(cube: &Cube, moment: &[f64]) -> f64 {
    moment[index(cube.r1, cube.g1, cube.b1)]
        - moment[index(cube.r1, cube.g1, cube.b0)]
        - moment[index(cube.r1, cube.g0, cube.b1)]
        + moment[index(cube.r1, cube.g0, cube.b0)]
        - moment[index(cube.r0, cube.g1, cube.b1)]
        + moment[index(cube.r0, cube.g1, cube.b0)]
        + moment[index(cube.r0, cube.g0, cube.b1)]
        - moment[index(cube.r0, cube.g0, cube.b0)]
}

fn bottom(cube: &Cube, direction: Direction, moment: &[f64]) -> f64 {
    match direction {
        Direction::Red => {
            -moment[index(cube.r0, cube.g1, cube.b1)]
                + moment[index(cube.r0, cube.g1, cube.b0)]
                + moment[index(cube.r0, cube.g0, cube.b1)]
                - moment[index(cube.r0, cube.g0, cube.b0)]
        }
        Direction::Green => {
            -moment[index(cube.r1, cube.g0, cube.b1)]
                + moment[index(cube.r1, cube.g0, cube.b0)]
                + moment[index(cube.r0, cube.g0, cube.b1)]
                - moment[index(cube.r0, cube.g0, cube.b0)]
        }
        Direction::Blue => {
            -moment[index(cube.r1, cube.g1, cube.b0)]
                + moment[index(cube.r1, cube.g0, cube.b0)]
                + moment[index(cube.r0, cube.g1, cube.b0)]
                - moment[index(cube.r0, cube.g0, cube.b0)]
        }
    }
}

fn top(cube: &Cube, direction: Direction, position: usize, moment: &[f64]) -> f64 {
    match direction {
        Direction::Red => {
            moment[index(position, cube.g1, cube.b1)]
                - moment[index(position, cube.g1, cube.b0)]
                - moment[index(position, cube.g0, cube.b1)]
                + moment[index(position, cube.g0, cube.b0)]
        }
        Direction::Green => {
            moment[index(cube.r1, position, cube.b1)]
                - moment[index(cube.r1, position, cube.b0)]
                - moment[index(cube.r0, position, cube.b1)]
                + moment[index(cube.r0, position, cube.b0)]
        }
        Direction::Blue => {
            moment[index(cube.r1, cube.g1, position)]
                - moment[index(cube.r1, cube.g0, position)]
                - moment[index(cube.r0, cube.g1, position)]
                + moment[index(cube.r0, cube.g0, position)]
        }
    }
}

const MAX_ITERATIONS: usize = 10;
const MIN_MOVEMENT_DISTANCE: f64 = 3.0;

/// Squared distance in L*a*b*.
fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (dl, da, db) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    dl * dl + da * da + db * db
}

/// Weighted k-means in L*a*b*, starting from `starting_clusters`, or from random ones if there are
/// none.
pub(crate) fn wsmeans(
    input_pixels: &[u32],
    starting_clusters: &[u32],
    max_colors: usize,
) -> Vec<(u32, u32)> {
    let mut random = JavaRandom::new(0x42688);

    // The distinct colors, in the order they first appear, and how many of each.
    let mut index_of_pixel = HashMap::new();
    let mut points = Vec::new();
    let mut counts: Vec<u32> = Vec::new();
    for &pixel in input_pixels {
        let i = *index_of_pixel.entry(pixel).or_insert_with(|| {
            points.push(utils::lab_from_argb(pixel));
            counts.push(0);
            points.len() - 1
        });
        counts[i] += 1;
    }
    let point_count = points.len();

    let mut cluster_count = max_colors.min(point_count);
    if !starting_clusters.is_empty() {
        cluster_count = cluster_count.min(starting_clusters.len());
    }
    if cluster_count == 0 {
        return Vec::new();
    }

    let mut clusters: Vec<[f64; 3]> = starting_clusters
        .iter()
        .map(|&c| utils::lab_from_argb(c))
        .collect();
    if starting_clusters.is_empty() {
        for _ in 0..cluster_count {
            let l = random.next_double() * 100.0;
            let a = random.next_double() * (100.0 - (-100.0) + 1.0) + -100.0;
            let b = random.next_double() * (100.0 - (-100.0) + 1.0) + -100.0;
            clusters.push([l, a, b]);
        }
    }

    let mut cluster_indices: Vec<usize> = (0..point_count)
        .map(|_| random.next_int(cluster_count as u32) as usize)
        .collect();

    // Upstream's TypeScript sorts each row of this matrix without a comparator, which leaves it as
    // it is, and then reads it by cluster index; so it is just the distances between clusters.
    let mut distance_matrix = vec![vec![0.0; cluster_count]; cluster_count];
    let mut pixel_count_sums = vec![0u32; cluster_count];

    for iteration in 0..MAX_ITERATIONS {
        for i in 0..cluster_count {
            for j in i + 1..cluster_count {
                let d = distance(clusters[i], clusters[j]);
                distance_matrix[j][i] = d;
                distance_matrix[i][j] = d;
            }
        }

        let mut points_moved = 0;
        for i in 0..point_count {
            let point = points[i];
            let previous_cluster_index = cluster_indices[i];
            let previous_distance = distance(point, clusters[previous_cluster_index]);
            let mut minimum_distance = previous_distance;
            let mut new_cluster_index = None;
            for j in 0..cluster_count {
                // Too far from the current cluster to be nearer the point.
                if distance_matrix[previous_cluster_index][j] >= 4.0 * previous_distance {
                    continue;
                }
                let d = distance(point, clusters[j]);
                if d < minimum_distance {
                    minimum_distance = d;
                    new_cluster_index = Some(j);
                }
            }
            if let Some(new_cluster_index) = new_cluster_index {
                let distance_change = (minimum_distance.sqrt() - previous_distance.sqrt()).abs();
                if distance_change > MIN_MOVEMENT_DISTANCE {
                    points_moved += 1;
                    cluster_indices[i] = new_cluster_index;
                }
            }
        }

        if points_moved == 0 && iteration != 0 {
            break;
        }

        let mut sums = vec![[0.0; 3]; cluster_count];
        pixel_count_sums.fill(0);
        for i in 0..point_count {
            let cluster_index = cluster_indices[i];
            let count = counts[i];
            pixel_count_sums[cluster_index] += count;
            for k in 0..3 {
                sums[cluster_index][k] += points[i][k] * count as f64;
            }
        }
        for i in 0..cluster_count {
            let count = pixel_count_sums[i];
            clusters[i] = if count == 0 {
                [0.0; 3]
            } else {
                sums[i].map(|s| s / count as f64)
            };
        }
    }

    let mut argb_to_population: Vec<(u32, u32)> = Vec::new();
    for i in 0..cluster_count {
        let count = pixel_count_sums[i];
        if count == 0 {
            continue;
        }
        let [l, a, b] = clusters[i];
        let possible_new_cluster = utils::argb_from_lab(l, a, b);
        if argb_to_population
            .iter()
            .any(|&(argb, _)| argb == possible_new_cluster)
        {
            continue;
        }
        argb_to_population.push((possible_new_cluster, count));
    }
    argb_to_population
}

/// `java.util.Random`, which upstream's `SeededRandom` reproduces.
struct JavaRandom {
    seed: u64,
}

impl JavaRandom {
    const MULTIPLIER: u64 = 0x5_DEEC_E66D;
    const MASK: u64 = (1 << 48) - 1;

    fn new(seed: u64) -> Self {
        Self {
            seed: (seed ^ Self::MULTIPLIER) & Self::MASK,
        }
    }

    fn next(&mut self, bits: u32) -> u32 {
        self.seed = self.seed.wrapping_mul(Self::MULTIPLIER).wrapping_add(0xB) & Self::MASK;
        (self.seed >> (48 - bits)) as u32
    }

    fn next_int(&mut self, bound: u32) -> u32 {
        if bound == 0 {
            return 0;
        }
        if bound.is_power_of_two() {
            return ((bound as u64 * self.next(31) as u64) >> 31) as u32;
        }
        loop {
            let bits = self.next(31) as u64;
            let val = bits % bound as u64;
            if bits - val + (bound as u64 - 1) < 1 << 31 {
                return val as u32;
            }
        }
    }

    fn next_double(&mut self) -> f64 {
        ((self.next(26) as u64 * (1 << 27) + self.next(27) as u64) as f64) / (1u64 << 53) as f64
    }
}
