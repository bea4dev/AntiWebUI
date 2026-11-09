//! CAM16 and HCT: hue and chroma from CAM16, tone from L*. From `hct/viewing_conditions.ts`,
//! `hct/cam16.ts` and `hct/hct.ts`.

use super::solver;
use super::utils::{self, lerp, signum};
use anti_webui::prelude::Color;
use std::f64::consts::PI;
use std::sync::LazyLock;

/// What CAM16 assumes about how a color is seen. Only the default ones are used here: sRGB white,
/// a gray background of L* 50, an average surround.
pub(crate) struct ViewingConditions {
    pub n: f64,
    pub aw: f64,
    pub nbb: f64,
    pub ncb: f64,
    pub c: f64,
    pub nc: f64,
    pub rgb_d: [f64; 3],
    pub fl: f64,
    pub fl_root: f64,
    pub z: f64,
}

pub(crate) static DEFAULT_VIEWING_CONDITIONS: LazyLock<ViewingConditions> = LazyLock::new(|| {
    ViewingConditions::new(
        [95.047, 100.0, 108.883],
        (200.0 / PI) * utils::y_from_lstar(50.0) / 100.0,
        50.0,
        2.0,
        false,
    )
});

impl ViewingConditions {
    fn new(
        white_point: [f64; 3],
        adapting_luminance: f64,
        background_lstar: f64,
        surround: f64,
        discounting_illuminant: bool,
    ) -> Self {
        let xyz = white_point;
        let r_w = xyz[0] * 0.401288 + xyz[1] * 0.650173 + xyz[2] * -0.051461;
        let g_w = xyz[0] * -0.250268 + xyz[1] * 1.204414 + xyz[2] * 0.045854;
        let b_w = xyz[0] * -0.002079 + xyz[1] * 0.048952 + xyz[2] * 0.953127;
        let f = 0.8 + surround / 10.0;
        let c = if f >= 0.9 {
            lerp(0.59, 0.69, (f - 0.9) * 10.0)
        } else {
            lerp(0.525, 0.59, (f - 0.8) * 10.0)
        };
        let d = if discounting_illuminant {
            1.0
        } else {
            f * (1.0 - (1.0 / 3.6) * ((-adapting_luminance - 42.0) / 92.0).exp())
        };
        let d = d.clamp(0.0, 1.0);
        let nc = f;
        let rgb_d = [
            d * (100.0 / r_w) + 1.0 - d,
            d * (100.0 / g_w) + 1.0 - d,
            d * (100.0 / b_w) + 1.0 - d,
        ];
        let k = 1.0 / (5.0 * adapting_luminance + 1.0);
        let k4 = k * k * k * k;
        let k4f = 1.0 - k4;
        let fl = k4 * adapting_luminance + 0.1 * k4f * k4f * (5.0 * adapting_luminance).cbrt();
        let n = utils::y_from_lstar(background_lstar) / white_point[1];
        let z = 1.48 + n.sqrt();
        let nbb = 0.725 / n.powf(0.2);
        let ncb = nbb;
        let factors = [
            (fl * rgb_d[0] * r_w / 100.0).powf(0.42),
            (fl * rgb_d[1] * g_w / 100.0).powf(0.42),
            (fl * rgb_d[2] * b_w / 100.0).powf(0.42),
        ];
        let rgb_a = factors.map(|f| 400.0 * f / (f + 27.13));
        let aw = (2.0 * rgb_a[0] + rgb_a[1] + 0.05 * rgb_a[2]) * nbb;
        Self {
            n,
            aw,
            nbb,
            ncb,
            c,
            nc,
            rgb_d,
            fl,
            fl_root: fl.powf(0.25),
            z,
        }
    }
}

/// A color in CAM16, under the default viewing conditions.
pub(crate) struct Cam16 {
    pub hue: f64,
    pub chroma: f64,
    /// These and [`Cam16::to_argb`] are for the tests, to compare with upstream's.
    #[cfg_attr(not(test), allow(dead_code))]
    pub j: f64,
    #[cfg_attr(not(test), allow(dead_code))]
    pub q: f64,
    #[cfg_attr(not(test), allow(dead_code))]
    pub m: f64,
    #[cfg_attr(not(test), allow(dead_code))]
    pub s: f64,
}

impl Cam16 {
    pub fn from_argb(argb: u32) -> Self {
        let vc = &*DEFAULT_VIEWING_CONDITIONS;
        let red_l = utils::linearized(utils::red_from_argb(argb));
        let green_l = utils::linearized(utils::green_from_argb(argb));
        let blue_l = utils::linearized(utils::blue_from_argb(argb));
        let x = 0.41233895 * red_l + 0.35762064 * green_l + 0.18051042 * blue_l;
        let y = 0.2126 * red_l + 0.7152 * green_l + 0.0722 * blue_l;
        let z = 0.01932141 * red_l + 0.11916382 * green_l + 0.95034478 * blue_l;

        let r_c = 0.401288 * x + 0.650173 * y - 0.051461 * z;
        let g_c = -0.250268 * x + 1.204414 * y + 0.045854 * z;
        let b_c = -0.002079 * x + 0.048952 * y + 0.953127 * z;

        let adapted = |c: f64, d: f64| {
            let d = d * c;
            let af = (vc.fl * d.abs() / 100.0).powf(0.42);
            signum(d) * 400.0 * af / (af + 27.13)
        };
        let r_a = adapted(r_c, vc.rgb_d[0]);
        let g_a = adapted(g_c, vc.rgb_d[1]);
        let b_a = adapted(b_c, vc.rgb_d[2]);

        let a = (11.0 * r_a + -12.0 * g_a + b_a) / 11.0;
        let b = (r_a + g_a - 2.0 * b_a) / 9.0;
        let u = (20.0 * r_a + 20.0 * g_a + 21.0 * b_a) / 20.0;
        let p2 = (40.0 * r_a + 20.0 * g_a + b_a) / 20.0;

        let atan_degrees = b.atan2(a) * 180.0 / PI;
        let hue = utils::sanitize_degrees(atan_degrees);
        let ac = p2 * vc.nbb;
        let j = 100.0 * (ac / vc.aw).powf(vc.c * vc.z);
        let q = (4.0 / vc.c) * (j / 100.0).sqrt() * (vc.aw + 4.0) * vc.fl_root;
        let hue_prime = if hue < 20.14 { hue + 360.0 } else { hue };
        let e_hue = 0.25 * ((hue_prime * PI / 180.0 + 2.0).cos() + 3.8);
        let p1 = (50000.0 / 13.0) * e_hue * vc.nc * vc.ncb;
        let t = p1 * (a * a + b * b).sqrt() / (u + 0.305);
        let alpha = t.powf(0.9) * (1.64 - 0.29f64.powf(vc.n)).powf(0.73);
        let c = alpha * (j / 100.0).sqrt();
        let m = c * vc.fl_root;
        let s = 50.0 * (alpha * vc.c / (vc.aw + 4.0)).sqrt();
        Self {
            hue,
            chroma: c,
            j,
            q,
            m,
            s,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn to_argb(&self) -> u32 {
        let vc = &*DEFAULT_VIEWING_CONDITIONS;
        let alpha = if self.chroma == 0.0 || self.j == 0.0 {
            0.0
        } else {
            self.chroma / (self.j / 100.0).sqrt()
        };
        let t = (alpha / (1.64 - 0.29f64.powf(vc.n)).powf(0.73)).powf(1.0 / 0.9);
        let h_rad = self.hue * PI / 180.0;
        let e_hue = 0.25 * ((h_rad + 2.0).cos() + 3.8);
        let ac = vc.aw * (self.j / 100.0).powf(1.0 / vc.c / vc.z);
        let p1 = e_hue * (50000.0 / 13.0) * vc.nc * vc.ncb;
        let p2 = ac / vc.nbb;
        let h_sin = h_rad.sin();
        let h_cos = h_rad.cos();
        let gamma = 23.0 * (p2 + 0.305) * t / (23.0 * p1 + 11.0 * t * h_cos + 108.0 * t * h_sin);
        let a = gamma * h_cos;
        let b = gamma * h_sin;
        let r_a = (460.0 * p2 + 451.0 * a + 288.0 * b) / 1403.0;
        let g_a = (460.0 * p2 - 891.0 * a - 261.0 * b) / 1403.0;
        let b_a = (460.0 * p2 - 220.0 * a - 6300.0 * b) / 1403.0;

        let unadapted = |a: f64| {
            let base = (27.13 * a.abs() / (400.0 - a.abs())).max(0.0);
            signum(a) * (100.0 / vc.fl) * base.powf(1.0 / 0.42)
        };
        let r_f = unadapted(r_a) / vc.rgb_d[0];
        let g_f = unadapted(g_a) / vc.rgb_d[1];
        let b_f = unadapted(b_a) / vc.rgb_d[2];

        let x = 1.86206786 * r_f - 1.01125463 * g_f + 0.14918677 * b_f;
        let y = 0.38752654 * r_f + 0.62144744 * g_f - 0.00897398 * b_f;
        let z = -0.01584150 * r_f - 0.03412294 * g_f + 1.04996444 * b_f;
        utils::argb_from_xyz(x, y, z)
    }
}

/// A color as hue, chroma and tone. Hue (0 to 360) and chroma come from CAM16; tone is L*, from 0
/// for black to 100 for white, so that a difference in tone says how far apart in contrast two
/// colors are. Not every combination exists: [`Hct::new`] gives the closest one that does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hct {
    hue: f64,
    chroma: f64,
    tone: f64,
    argb: u32,
}

impl Hct {
    /// The sRGB color closest to this hue, chroma and tone. The tone is kept and the hue as close as
    /// can be; the chroma is lowered if it is too high for them.
    pub fn new(hue: f64, chroma: f64, tone: f64) -> Self {
        Self::from_argb(solver::solve_to_argb(hue, chroma, tone))
    }

    /// `0xAARRGGBB`; the alpha is ignored.
    pub fn from_argb(argb: u32) -> Self {
        let argb = argb | 0xff00_0000;
        let cam = Cam16::from_argb(argb);
        Self {
            hue: cam.hue,
            chroma: cam.chroma,
            tone: utils::lstar_from_argb(argb),
            argb,
        }
    }

    /// Takes the closest color with 8 bits per channel, ignoring the alpha.
    pub fn from_color(color: Color) -> Self {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Self::from_argb(utils::argb_from_rgb(c(color.r), c(color.g), c(color.b)))
    }

    pub fn hue(&self) -> f64 {
        self.hue
    }

    pub fn chroma(&self) -> f64 {
        self.chroma
    }

    pub fn tone(&self) -> f64 {
        self.tone
    }

    /// `0xFFRRGGBB`
    pub fn to_argb(&self) -> u32 {
        self.argb
    }

    pub fn to_color(&self) -> Color {
        color_from_argb(self.argb)
    }

    pub(crate) fn is_yellow(hue: f64) -> bool {
        (105.0..125.0).contains(&hue)
    }
}

pub(crate) fn color_from_argb(argb: u32) -> Color {
    Color::from_rgb(argb & 0x00ff_ffff)
}
