//! Color science: sRGB, linear RGB, XYZ, L*a*b* and L*, and a little math on angles.
//! From `utils/color_utils.ts` and `utils/math_utils.ts`.

pub(crate) fn signum(x: f64) -> f64 {
    if x < 0.0 {
        -1.0
    } else if x == 0.0 {
        0.0
    } else {
        1.0
    }
}

pub(crate) fn lerp(start: f64, stop: f64, amount: f64) -> f64 {
    (1.0 - amount) * start + amount * stop
}

/// Like `f64::clamp`, but doesn't panic when `min > max`, as upstream relies on in places.
pub(crate) fn clamp(min: f64, max: f64, input: f64) -> f64 {
    if input < min {
        min
    } else if input > max {
        max
    } else {
        input
    }
}

pub(crate) fn sanitize_degrees_int(degrees: i32) -> i32 {
    degrees.rem_euclid(360)
}

pub(crate) fn sanitize_degrees(degrees: f64) -> f64 {
    let degrees = degrees % 360.0;
    if degrees < 0.0 {
        degrees + 360.0
    } else {
        degrees
    }
}

/// The distance between two hues, in degrees, the short way round.
pub(crate) fn difference_degrees(a: f64, b: f64) -> f64 {
    180.0 - ((a - b).abs() - 180.0).abs()
}

pub(crate) fn matrix_multiply(row: [f64; 3], matrix: &[[f64; 3]; 3]) -> [f64; 3] {
    std::array::from_fn(|i| row[0] * matrix[i][0] + row[1] * matrix[i][1] + row[2] * matrix[i][2])
}

const SRGB_TO_XYZ: [[f64; 3]; 3] = [
    [0.41233895, 0.35762064, 0.18051042],
    [0.2126, 0.7152, 0.0722],
    [0.01932141, 0.11916382, 0.95034478],
];

const XYZ_TO_SRGB: [[f64; 3]; 3] = [
    [
        3.2413774792388685,
        -1.5376652402851851,
        -0.49885366846268053,
    ],
    [-0.9691452513005321, 1.8758853451067872, 0.04156585616912061],
    [
        0.05562093689691305,
        -0.20395524564742123,
        1.0571799111220335,
    ],
];

const WHITE_POINT_D65: [f64; 3] = [95.047, 100.0, 108.883];

pub(crate) fn argb_from_rgb(red: u8, green: u8, blue: u8) -> u32 {
    0xff00_0000 | (red as u32) << 16 | (green as u32) << 8 | blue as u32
}

pub(crate) fn alpha_from_argb(argb: u32) -> u8 {
    (argb >> 24) as u8
}

pub(crate) fn red_from_argb(argb: u32) -> u8 {
    (argb >> 16) as u8
}

pub(crate) fn green_from_argb(argb: u32) -> u8 {
    (argb >> 8) as u8
}

pub(crate) fn blue_from_argb(argb: u32) -> u8 {
    argb as u8
}

/// Components from 0 to 100.
pub(crate) fn argb_from_linrgb(linrgb: [f64; 3]) -> u32 {
    argb_from_rgb(
        delinearized(linrgb[0]),
        delinearized(linrgb[1]),
        delinearized(linrgb[2]),
    )
}

pub(crate) fn argb_from_xyz(x: f64, y: f64, z: f64) -> u32 {
    argb_from_linrgb(matrix_multiply([x, y, z], &XYZ_TO_SRGB))
}

pub(crate) fn xyz_from_argb(argb: u32) -> [f64; 3] {
    let r = linearized(red_from_argb(argb));
    let g = linearized(green_from_argb(argb));
    let b = linearized(blue_from_argb(argb));
    matrix_multiply([r, g, b], &SRGB_TO_XYZ)
}

pub(crate) fn argb_from_lab(l: f64, a: f64, b: f64) -> u32 {
    let fy = (l + 16.0) / 116.0;
    let fx = a / 500.0 + fy;
    let fz = fy - b / 200.0;
    let x = lab_invf(fx) * WHITE_POINT_D65[0];
    let y = lab_invf(fy) * WHITE_POINT_D65[1];
    let z = lab_invf(fz) * WHITE_POINT_D65[2];
    argb_from_xyz(x, y, z)
}

pub(crate) fn lab_from_argb(argb: u32) -> [f64; 3] {
    let [x, y, z] = xyz_from_argb(argb);
    let fx = lab_f(x / WHITE_POINT_D65[0]);
    let fy = lab_f(y / WHITE_POINT_D65[1]);
    let fz = lab_f(z / WHITE_POINT_D65[2]);
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// The gray of this L*.
pub(crate) fn argb_from_lstar(lstar: f64) -> u32 {
    let component = delinearized(y_from_lstar(lstar));
    argb_from_rgb(component, component, component)
}

pub(crate) fn lstar_from_argb(argb: u32) -> f64 {
    let y = xyz_from_argb(argb)[1];
    116.0 * lab_f(y / 100.0) - 16.0
}

/// Relative luminance from 0 to 100, from L* from 0 to 100.
pub(crate) fn y_from_lstar(lstar: f64) -> f64 {
    100.0 * lab_invf((lstar + 16.0) / 116.0)
}

pub(crate) fn lstar_from_y(y: f64) -> f64 {
    lab_f(y / 100.0) * 116.0 - 16.0
}

/// An sRGB component from 0 to 255 to a linear one from 0 to 100.
pub(crate) fn linearized(component: u8) -> f64 {
    let normalized = component as f64 / 255.0;
    if normalized <= 0.040449936 {
        normalized / 12.92 * 100.0
    } else {
        ((normalized + 0.055) / 1.055).powf(2.4) * 100.0
    }
}

/// A linear component from 0 to 100 to an sRGB one from 0 to 255.
pub(crate) fn delinearized(component: f64) -> u8 {
    let normalized = component / 100.0;
    let delinearized = if normalized <= 0.0031308 {
        normalized * 12.92
    } else {
        1.055 * normalized.powf(1.0 / 2.4) - 0.055
    };
    (delinearized * 255.0).round().clamp(0.0, 255.0) as u8
}

fn lab_f(t: f64) -> f64 {
    let e = 216.0 / 24389.0;
    let kappa = 24389.0 / 27.0;
    if t > e {
        t.powf(1.0 / 3.0)
    } else {
        (kappa * t + 16.0) / 116.0
    }
}

fn lab_invf(ft: f64) -> f64 {
    let e = 216.0 / 24389.0;
    let kappa = 24389.0 / 27.0;
    let ft3 = ft * ft * ft;
    if ft3 > e {
        ft3
    } else {
        (116.0 * ft - 16.0) / kappa
    }
}
