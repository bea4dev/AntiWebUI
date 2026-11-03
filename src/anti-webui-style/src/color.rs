/// A non-premultiplied sRGB color with components in `0.0..=1.0`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Color = Color::new(0.0, 0.0, 0.0, 0.0);
    pub const BLACK: Color = Color::new(0.0, 0.0, 0.0, 1.0);
    pub const WHITE: Color = Color::new(1.0, 1.0, 1.0, 1.0);

    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// `0xRRGGBB`
    pub const fn from_rgb(hex: u32) -> Self {
        Self::from_rgba((hex << 8) | 0xff)
    }

    /// `0xRRGGBBAA`
    pub const fn from_rgba(hex: u32) -> Self {
        Self::new(
            ((hex >> 24) & 0xff) as f32 / 255.0,
            ((hex >> 16) & 0xff) as f32 / 255.0,
            ((hex >> 8) & 0xff) as f32 / 255.0,
            (hex & 0xff) as f32 / 255.0,
        )
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// The closest color with 8 bits per channel. Colors differing by less than that look the
    /// same on screen, so they shouldn't cause a repaint.
    pub fn snapped(self) -> Self {
        let snap = |v: f32| (v * 255.0).round() / 255.0;
        Self::new(snap(self.r), snap(self.g), snap(self.b), snap(self.a))
    }

    pub fn is_transparent(&self) -> bool {
        self.a <= 0.0
    }

    /// Premultiplied components, ready for the GPU.
    pub fn premultiplied(&self) -> [f32; 4] {
        [self.r * self.a, self.g * self.a, self.b * self.a, self.a]
    }

    /// Linear interpolation in sRGB space.
    pub fn mix(self, other: Color, t: f32) -> Color {
        let l = |a: f32, b: f32| a + (b - a) * t;
        Color::new(
            l(self.r, other.r),
            l(self.g, other.g),
            l(self.b, other.b),
            l(self.a, other.a),
        )
    }
}

/// `0xRRGGBB`
pub const fn rgb(hex: u32) -> Color {
    Color::from_rgb(hex)
}

/// `0xRRGGBBAA`
pub const fn rgba(hex: u32) -> Color {
    Color::from_rgba(hex)
}
