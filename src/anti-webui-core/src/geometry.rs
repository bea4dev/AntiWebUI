//! Physical-pixel geometry and the logical → physical conversion.

/// Display scale expressed as `n / 120`, the same representation as `wp_fractional_scale_v1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Scale(pub u32);

impl Scale {
    pub const ONE: Scale = Scale(120);

    pub fn from_f64(scale: f64) -> Scale {
        Scale((scale * 120.0).round().max(1.0) as u32)
    }

    pub fn as_f32(self) -> f32 {
        self.0 as f32 / 120.0
    }

    pub fn as_f64(self) -> f64 {
        self.0 as f64 / 120.0
    }

    /// The single rounding rule of the pixel model (R1): round half away from zero.
    pub fn px(self, logical: f32) -> i32 {
        (logical as f64 * self.0 as f64 / 120.0).round() as i32
    }

    /// Like [`Scale::px`], but a non-zero length never collapses to zero (R2).
    pub fn px_min1(self, logical: f32) -> i32 {
        if logical > 0.0 {
            self.px(logical).max(1)
        } else {
            0
        }
    }

    /// Unrounded physical value, for paint-only quantities (radii, blur, font size).
    pub fn pxf(self, logical: f32) -> f32 {
        (logical as f64 * self.0 as f64 / 120.0) as f32
    }

    /// The whole logical pixels needed to cover `physical` pixels.
    pub fn logical_ceil(self, physical: i32) -> i32 {
        (physical as i64 * 120).div_euclid(self.0 as i64) as i32
            + ((physical as i64 * 120).rem_euclid(self.0 as i64) != 0) as i32
    }

    /// The whole logical pixels up to `physical` pixels.
    pub fn logical_floor(self, physical: i32) -> i32 {
        (physical as i64 * 120).div_euclid(self.0 as i64) as i32
    }

    /// Physical → logical, for coordinates handed back to the OS (R7).
    pub fn to_logical(self, physical: f32) -> f32 {
        (physical as f64 * 120.0 / self.0 as f64) as f32
    }
}

impl Default for Scale {
    fn default() -> Self {
        Scale::ONE
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PointI {
    pub x: i32,
    pub y: i32,
}

impl PointI {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PointF {
    pub x: f32,
    pub y: f32,
}

impl PointF {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn distance(self, other: PointF) -> f32 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}

/// A rectangle in logical pixels, as what is measured is told to programs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl RectF {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SizeI {
    pub width: i32,
    pub height: i32,
}

impl SizeI {
    pub const fn new(width: i32, height: i32) -> Self {
        Self { width, height }
    }
}

/// A 2D affine transform in physical pixels: `x' = a x + c y + e`, `y' = b x + d y + f`.
///
/// `f64` so that a transform far down a long list stays exact after moving back to the target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    pub const IDENTITY: Affine = Affine::translation(0.0, 0.0);

    pub const fn translation(x: f64, y: f64) -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: x,
            f: y,
        }
    }

    pub const fn scaling(x: f64, y: f64) -> Self {
        Self {
            a: x,
            b: 0.0,
            c: 0.0,
            d: y,
            e: 0.0,
            f: 0.0,
        }
    }

    /// Clockwise on the screen, where y grows downwards.
    pub fn rotation(degrees: f64) -> Self {
        let (s, c) = degrees.to_radians().sin_cos();
        Self {
            a: c,
            b: s,
            c: -s,
            d: c,
            e: 0.0,
            f: 0.0,
        }
    }

    /// Applies `other` first, then `self`.
    pub fn then_apply(&self, other: &Affine) -> Affine {
        Affine {
            a: self.a * other.a + self.c * other.b,
            b: self.b * other.a + self.d * other.b,
            c: self.a * other.c + self.c * other.d,
            d: self.b * other.c + self.d * other.d,
            e: self.a * other.e + self.c * other.f + self.e,
            f: self.b * other.e + self.d * other.f + self.f,
        }
    }

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    pub fn apply_point(&self, p: PointF) -> PointF {
        let (x, y) = self.apply(p.x as f64, p.y as f64);
        PointF::new(x as f32, y as f32)
    }

    /// `None` when it flattens everything to a line or a point.
    pub fn inverse(&self) -> Option<Affine> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-12 {
            return None;
        }
        Some(Affine {
            a: self.d / det,
            b: -self.b / det,
            c: -self.c / det,
            d: self.a / det,
            e: (self.c * self.f - self.d * self.e) / det,
            f: (self.b * self.e - self.a * self.f) / det,
        })
    }

    pub fn is_translation(&self) -> bool {
        self.a == 1.0 && self.b == 0.0 && self.c == 0.0 && self.d == 1.0
    }

    /// Whether pixels stay exactly on pixels.
    pub fn is_whole_translation(&self) -> bool {
        // Going through a transform origin and back leaves rounding errors.
        let whole = |v: f64| (v - v.round()).abs() < 1e-6;
        self.is_translation() && whole(self.e) && whole(self.f)
    }

    /// Whether the axes stay along the axes.
    pub fn is_axis_aligned(&self) -> bool {
        self.b == 0.0 && self.c == 0.0
    }

    /// The most it stretches anything.
    pub fn max_scale(&self) -> f64 {
        (self.a * self.a + self.b * self.b)
            .sqrt()
            .max((self.c * self.c + self.d * self.d).sqrt())
    }

    /// The smallest whole rectangle around the transformed `r`.
    pub fn bounds_of(&self, r: RectI) -> RectI {
        if r.is_empty() {
            return RectI::default();
        }
        let (x0, y0) = (r.x as f64, r.y as f64);
        let (x1, y1) = (r.right() as f64, r.bottom() as f64);
        let corners = [
            self.apply(x0, y0),
            self.apply(x1, y0),
            self.apply(x0, y1),
            self.apply(x1, y1),
        ];
        let (mut left, mut top) = corners[0];
        let (mut right, mut bottom) = corners[0];
        for (x, y) in &corners[1..] {
            left = left.min(*x);
            right = right.max(*x);
            top = top.min(*y);
            bottom = bottom.max(*y);
        }
        let limit = |v: f64| v.clamp(-(1 << 28) as f64, (1 << 28) as f64);
        let (left, top) = (limit(left).floor() as i32, limit(top).floor() as i32);
        let (right, bottom) = (limit(right).ceil() as i32, limit(bottom).ceil() as i32);
        RectI::new(left, top, right - left, bottom - top)
    }
}

/// An integer rectangle in physical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RectI {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl RectI {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// A rectangle that contains everything; used as "no clip".
    pub const INFINITE: RectI = RectI::new(-(1 << 28), -(1 << 28), 1 << 29, 1 << 29);

    pub fn right(&self) -> i32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.height
    }

    pub fn origin(&self) -> PointI {
        PointI::new(self.x, self.y)
    }

    pub fn size(&self) -> SizeI {
        SizeI::new(self.width, self.height)
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    pub fn contains(&self, p: PointF) -> bool {
        p.x >= self.x as f32
            && p.y >= self.y as f32
            && p.x < self.right() as f32
            && p.y < self.bottom() as f32
    }

    pub fn intersect(&self, other: &RectI) -> RectI {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let r = self.right().min(other.right());
        let b = self.bottom().min(other.bottom());
        RectI::new(x, y, (r - x).max(0), (b - y).max(0))
    }

    /// The smallest rectangle containing both; an empty side is ignored.
    pub fn union(&self, other: &RectI) -> RectI {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let r = self.right().max(other.right());
        let b = self.bottom().max(other.bottom());
        RectI::new(x, y, r - x, b - y)
    }

    pub fn area(&self) -> i64 {
        if self.is_empty() {
            0
        } else {
            self.width as i64 * self.height as i64
        }
    }

    pub fn intersects(&self, other: &RectI) -> bool {
        !self.intersect(other).is_empty()
    }

    pub fn translate(&self, dx: i32, dy: i32) -> RectI {
        RectI::new(self.x + dx, self.y + dy, self.width, self.height)
    }

    /// Shrinks the rectangle by the given edge amounts.
    pub fn inset(&self, top: i32, right: i32, bottom: i32, left: i32) -> RectI {
        RectI::new(
            self.x + left,
            self.y + top,
            (self.width - left - right).max(0),
            (self.height - top - bottom).max(0),
        )
    }

    /// The rectangle `k` times bigger, around the origin.
    pub fn scaled(&self, k: i32) -> RectI {
        RectI::new(self.x * k, self.y * k, self.width * k, self.height * k)
    }

    pub fn outset(&self, amount: i32) -> RectI {
        RectI::new(
            self.x - amount,
            self.y - amount,
            self.width + amount * 2,
            self.height + amount * 2,
        )
    }

    /// Distance from `p` to the rectangle along each axis (0 when inside on that axis).
    pub fn axis_distance(&self, p: PointF) -> (f32, f32) {
        let dx = if p.x < self.x as f32 {
            self.x as f32 - p.x
        } else if p.x >= self.right() as f32 {
            p.x - self.right() as f32 + 1.0
        } else {
            0.0
        };
        let dy = if p.y < self.y as f32 {
            self.y as f32 - p.y
        } else if p.y >= self.bottom() as f32 {
            p.y - self.bottom() as f32 + 1.0
        } else {
            0.0
        };
        (dx, dy)
    }

    pub fn clamp_point(&self, p: PointF) -> PointF {
        PointF::new(
            p.x.clamp(self.x as f32, (self.right() - 1).max(self.x) as f32),
            p.y.clamp(self.y as f32, (self.bottom() - 1).max(self.y) as f32),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_is_symmetric() {
        let s = Scale(180); // 1.5
        assert_eq!(s.px(33.0), 50);
        assert_eq!(s.px(-33.0), -50);
        assert_eq!(s.px(1.0), 2);
        assert_eq!(Scale(108).px_min1(0.4), 1); // 0.9x
    }
}
