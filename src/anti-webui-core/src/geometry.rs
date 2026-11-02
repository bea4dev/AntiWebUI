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
