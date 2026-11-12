//! Espacios de color de CSS Color 4 (§10–§11, código de ejemplo de §18) y su conversión a
//! sRGB con el gamut mapping de §13.2, más la interpolación de `color-mix()` (CSS Color 5 §2,
//! CSS Color 4 §12).
//!
//! El valor computado de un color sigue siendo un [`Rgba`] sRGB: los colores de otros
//! espacios se convierten (y se ajustan a la gama de sRGB) al parsearlos.

use super::color::Rgba;

/// Un espacio de color: los de `color()` (CSS Color 4 §10) y los de `<color-space>` de
/// `color-mix()` (CSS Color 5 §2.1, CSS Color 4 §12.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    Srgb,
    SrgbLinear,
    DisplayP3,
    A98Rgb,
    ProphotoRgb,
    Rec2020,
    XyzD50,
    XyzD65,
    Lab,
    Oklab,
    Hsl,
    Hwb,
    Lch,
    Oklch,
}

/// `<hue-interpolation-method>` (CSS Color 4 §12.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HueMethod {
    #[default]
    Shorter,
    Longer,
    Increasing,
    Decreasing,
}

type Vec3 = [f64; 3];
type Mat3 = [[f64; 3]; 3];

fn mul(m: &Mat3, v: Vec3) -> Vec3 {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

fn map3(v: Vec3, f: impl Fn(f64) -> f64) -> Vec3 {
    [f(v[0]), f(v[1]), f(v[2])]
}

// --- Matrices (CSS Color 4 §18) ---

const SRGB_TO_XYZ: Mat3 = [
    [
        0.412_390_799_265_959_34,
        0.357_584_339_383_878,
        0.180_480_788_401_834_3,
    ],
    [
        0.212_639_005_871_510_27,
        0.715_168_678_767_756,
        0.072_192_315_360_733_71,
    ],
    [
        0.019_330_818_715_591_82,
        0.119_194_779_794_625_98,
        0.950_532_152_249_660_7,
    ],
];
const XYZ_TO_SRGB: Mat3 = [
    [
        3.240_969_941_904_522_6,
        -1.537_383_177_570_094,
        -0.498_610_760_293_003_4,
    ],
    [
        -0.969_243_636_280_879_6,
        1.875_967_501_507_720_2,
        0.041_555_057_407_175_59,
    ],
    [
        0.055_630_079_696_993_66,
        -0.203_976_958_888_976_52,
        1.056_971_514_242_878_6,
    ],
];
const P3_TO_XYZ: Mat3 = [
    [
        0.486_570_948_648_216_26,
        0.265_667_693_169_092_94,
        0.198_217_285_234_362_5,
    ],
    [
        0.228_974_564_069_748_84,
        0.691_738_521_836_506_2,
        0.079_286_914_093_745,
    ],
    [0.0, 0.045_113_381_858_902_575, 1.043_944_368_900_975_7],
];
const XYZ_TO_P3: Mat3 = [
    [
        2.493_496_911_941_424_5,
        -0.931_383_617_919_123_7,
        -0.402_710_784_450_716_84,
    ],
    [
        -0.829_488_969_561_574_9,
        1.762_664_060_318_346_3,
        0.023_624_685_841_943_58,
    ],
    [
        0.035_845_830_243_784_33,
        -0.076_172_389_268_041_82,
        0.956_884_524_007_687_2,
    ],
];
const A98_TO_XYZ: Mat3 = [
    [
        0.576_669_042_910_130_5,
        0.185_558_237_906_546_3,
        0.188_228_646_234_994_7,
    ],
    [
        0.297_344_975_250_536_05,
        0.627_363_566_255_466_1,
        0.075_291_458_493_997_88,
    ],
    [
        0.027_031_361_386_412_34,
        0.070_688_852_535_827_23,
        0.991_337_536_837_638_8,
    ],
];
const XYZ_TO_A98: Mat3 = [
    [
        2.041_587_903_810_746_5,
        -0.565_006_974_278_859_6,
        -0.344_731_350_778_329_56,
    ],
    [
        -0.969_243_636_280_879_5,
        1.875_967_501_507_720_2,
        0.041_555_057_407_175_57,
    ],
    [
        0.013_444_280_632_031_142,
        -0.118_362_392_231_018_38,
        1.015_174_994_391_205_4,
    ],
];
const PROPHOTO_TO_XYZ_D50: Mat3 = [
    [
        0.797_766_644_900_642_3,
        0.135_181_297_400_533_08,
        0.031_347_734_128_392_2,
    ],
    [
        0.288_074_828_819_401_3,
        0.711_835_234_241_873,
        0.000_089_936_938_725_64,
    ],
    [0.0, 0.0, 0.825_104_602_510_460_2],
];
const XYZ_D50_TO_PROPHOTO: Mat3 = [
    [
        1.345_786_881_647_158_3,
        -0.255_572_087_379_794_64,
        -0.051_101_864_975_545_26,
    ],
    [
        -0.544_630_705_124_901_9,
        1.508_247_742_845_146_8,
        0.020_527_447_436_421_39,
    ],
    [0.0, 0.0, 1.211_967_545_638_945_2],
];
const REC2020_TO_XYZ: Mat3 = [
    [
        0.636_958_048_301_291_4,
        0.144_616_903_586_208_32,
        0.168_880_975_164_172_1,
    ],
    [
        0.262_700_212_011_267_1,
        0.677_998_071_518_870_8,
        0.059_301_716_469_861_96,
    ],
    [0.0, 0.028_072_693_049_087_428, 1.060_985_057_710_791],
];
const XYZ_TO_REC2020: Mat3 = [
    [
        1.716_651_187_971_268,
        -0.355_670_783_776_392,
        -0.253_366_281_373_66,
    ],
    [
        -0.666_684_351_832_489,
        1.616_481_236_634_939,
        0.015_768_545_813_911_1,
    ],
    [
        0.017_639_857_445_311,
        -0.042_770_613_257_809,
        0.942_103_121_235_474,
    ],
];
/// Adaptación cromática de Bradford.
const D65_TO_D50: Mat3 = [
    [
        1.047_929_792_544_996_9,
        0.022_946_870_601_609_652,
        -0.050_192_266_289_205_24,
    ],
    [
        0.029_627_808_770_055_99,
        0.990_434_426_753_879_9,
        -0.017_073_799_063_418_826,
    ],
    [
        -0.009_243_040_646_204_504,
        0.015_055_191_490_298_152,
        0.751_874_281_428_137_1,
    ],
];
const D50_TO_D65: Mat3 = [
    [
        0.955_473_421_488_075,
        -0.023_098_454_948_764_71,
        0.063_259_243_200_570_72,
    ],
    [
        -0.028_369_709_333_863_7,
        1.009_995_398_081_304_1,
        0.021_041_441_191_917_323,
    ],
    [
        0.012_314_014_864_481_998,
        -0.020_507_649_298_898_964,
        1.330_365_926_242_124,
    ],
];
const XYZ_TO_LMS: Mat3 = [
    [
        0.819_022_437_996_703,
        0.361_906_260_052_890_4,
        -0.128_873_781_520_987_9,
    ],
    [
        0.032_983_653_932_388_5,
        0.929_286_861_586_343_4,
        0.036_144_666_350_642_4,
    ],
    [
        0.048_177_189_359_624_2,
        0.264_239_531_752_730_8,
        0.633_547_828_469_430_9,
    ],
];
const LMS_TO_OKLAB: Mat3 = [
    [
        0.210_454_268_309_314,
        0.793_617_774_702_305_4,
        -0.004_072_043_011_619_3,
    ],
    [
        1.977_998_532_431_168_4,
        -2.428_592_242_048_58,
        0.450_593_709_617_411,
    ],
    [
        0.025_904_042_465_547_8,
        0.782_771_712_457_529_6,
        -0.808_675_754_923_077_4,
    ],
];
const OKLAB_TO_LMS: Mat3 = [
    [1.0, 0.396_337_777_376_174_9, 0.215_803_757_309_913_6],
    [1.0, -0.105_561_345_815_658_6, -0.063_854_172_825_813_3],
    [1.0, -0.089_484_177_529_811_9, -1.291_485_548_019_409_2],
];
const LMS_TO_XYZ: Mat3 = [
    [1.226879875845924, -0.557814994460217, 0.2813910456659646],
    [
        -0.040575745214800875,
        1.1122868032803173,
        -0.07171105806551642,
    ],
    [
        -0.07637293667466001,
        -0.4214933324022431,
        1.5869240198367816,
    ],
];

/// Blanco D50 (CSS Color 4 §10.7).
const D50: Vec3 = [0.3457 / 0.3585, 1.0, (1.0 - 0.3457 - 0.3585) / 0.3585];
const KAPPA: f64 = 24389.0 / 27.0;
const EPSILON: f64 = 216.0 / 24389.0;

// --- Funciones de transferencia ---

fn srgb_to_linear(c: f64) -> f64 {
    let a = c.abs();
    if a <= 0.04045 {
        c / 12.92
    } else {
        c.signum() * ((a + 0.055) / 1.055).powf(2.4)
    }
}

fn srgb_from_linear(c: f64) -> f64 {
    let a = c.abs();
    if a > 0.003_130_8 {
        c.signum() * (1.055 * a.powf(1.0 / 2.4) - 0.055)
    } else {
        12.92 * c
    }
}

fn a98_to_linear(c: f64) -> f64 {
    c.signum() * c.abs().powf(563.0 / 256.0)
}

fn a98_from_linear(c: f64) -> f64 {
    c.signum() * c.abs().powf(256.0 / 563.0)
}

fn prophoto_to_linear(c: f64) -> f64 {
    let a = c.abs();
    if a <= 16.0 / 512.0 {
        c / 16.0
    } else {
        c.signum() * a.powf(1.8)
    }
}

fn prophoto_from_linear(c: f64) -> f64 {
    let a = c.abs();
    if a >= 1.0 / 512.0 {
        c.signum() * a.powf(1.0 / 1.8)
    } else {
        16.0 * c
    }
}

const REC_ALPHA: f64 = 1.099_296_826_809_44;
const REC_BETA: f64 = 0.018_053_968_510_807;

fn rec2020_to_linear(c: f64) -> f64 {
    let a = c.abs();
    if a < REC_BETA * 4.5 {
        c / 4.5
    } else {
        c.signum() * ((a + REC_ALPHA - 1.0) / REC_ALPHA).powf(1.0 / 0.45)
    }
}

fn rec2020_from_linear(c: f64) -> f64 {
    let a = c.abs();
    if a > REC_BETA {
        c.signum() * (REC_ALPHA * a.powf(0.45) - (REC_ALPHA - 1.0))
    } else {
        4.5 * c
    }
}

// --- Lab, OKLab y las formas polares ---

fn lab_to_xyz_d50([l, a, b]: Vec3) -> Vec3 {
    let f1 = (l + 16.0) / 116.0;
    let f0 = a / 500.0 + f1;
    let f2 = f1 - b / 200.0;
    let x = if f0.powi(3) > EPSILON {
        f0.powi(3)
    } else {
        (116.0 * f0 - 16.0) / KAPPA
    };
    let y = if l > KAPPA * EPSILON {
        f1.powi(3)
    } else {
        l / KAPPA
    };
    let z = if f2.powi(3) > EPSILON {
        f2.powi(3)
    } else {
        (116.0 * f2 - 16.0) / KAPPA
    };
    [x * D50[0], y * D50[1], z * D50[2]]
}

fn xyz_d50_to_lab(xyz: Vec3) -> Vec3 {
    let f = |v: f64| {
        if v > EPSILON {
            v.cbrt()
        } else {
            (KAPPA * v + 16.0) / 116.0
        }
    };
    let [f0, f1, f2] = [f(xyz[0] / D50[0]), f(xyz[1] / D50[1]), f(xyz[2] / D50[2])];
    [116.0 * f1 - 16.0, 500.0 * (f0 - f1), 200.0 * (f1 - f2)]
}

fn oklab_to_xyz(c: Vec3) -> Vec3 {
    mul(&LMS_TO_XYZ, map3(mul(&OKLAB_TO_LMS, c), |v| v.powi(3)))
}

fn xyz_to_oklab(xyz: Vec3) -> Vec3 {
    mul(&LMS_TO_OKLAB, map3(mul(&XYZ_TO_LMS, xyz), f64::cbrt))
}

/// Rectangular → polar: `[L, C, H]` (`H` en grados, NaN si es powerless).
fn to_polar([l, a, b]: Vec3, epsilon: f64) -> Vec3 {
    let c = (a * a + b * b).sqrt();
    let h = if c <= epsilon {
        f64::NAN
    } else {
        b.atan2(a).to_degrees().rem_euclid(360.0)
    };
    [l, c, h]
}

fn from_polar([l, c, h]: Vec3) -> Vec3 {
    let h = if h.is_nan() { 0.0 } else { h.to_radians() };
    [l, c * h.cos(), c * h.sin()]
}

// --- HSL y HWB (CSS Color 4 §7, §8), sobre sRGB en [0, 1] ---

fn hsl_to_srgb([h, s, l]: Vec3) -> Vec3 {
    let h = if h.is_nan() { 0.0 } else { h.rem_euclid(360.0) };
    let f = |n: f64| {
        let k = (n + h / 30.0) % 12.0;
        let a = s * l.min(1.0 - l);
        l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
    };
    [f(0.0), f(8.0), f(4.0)]
}

fn srgb_to_hsl([r, g, b]: Vec3) -> Vec3 {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (min + max) / 2.0;
    let d = max - min;
    let mut h = f64::NAN;
    let mut s = 0.0;
    if d != 0.0 {
        s = if l == 0.0 || l == 1.0 {
            0.0
        } else {
            (max - l) / l.min(1.0 - l)
        };
        h = if max == r {
            (g - b) / d + if g < b { 6.0 } else { 0.0 }
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        } * 60.0;
    }
    if s.abs() < 1e-9 {
        h = f64::NAN;
    }
    [h, s, l]
}

fn hwb_to_srgb([h, w, bl]: Vec3) -> Vec3 {
    if w + bl >= 1.0 {
        let gray = w / (w + bl);
        return [gray; 3];
    }
    map3(hsl_to_srgb([h, 1.0, 0.5]), |c| c * (1.0 - w - bl) + w)
}

fn srgb_to_hwb(rgb: Vec3) -> Vec3 {
    let hsl = srgb_to_hsl(rgb);
    let w = rgb[0].min(rgb[1]).min(rgb[2]);
    let bl = 1.0 - rgb[0].max(rgb[1]).max(rgb[2]);
    let h = if w + bl >= 1.0 - 1e-9 {
        f64::NAN
    } else {
        hsl[0]
    };
    [h, w, bl]
}

impl ColorSpace {
    /// `<predefined-rgb>` y `<xyz-space>` de `color()` (CSS Color 4 §10.1).
    pub fn predefined(name: &str) -> Option<ColorSpace> {
        Some(match name.to_ascii_lowercase().as_str() {
            "srgb" => ColorSpace::Srgb,
            "srgb-linear" => ColorSpace::SrgbLinear,
            "display-p3" => ColorSpace::DisplayP3,
            "a98-rgb" => ColorSpace::A98Rgb,
            "prophoto-rgb" => ColorSpace::ProphotoRgb,
            "rec2020" => ColorSpace::Rec2020,
            "xyz" | "xyz-d65" => ColorSpace::XyzD65,
            "xyz-d50" => ColorSpace::XyzD50,
            _ => return None,
        })
    }

    /// `<color-space>` de `<color-interpolation-method>` (CSS Color 4 §12.1).
    pub fn interpolation(name: &str) -> Option<ColorSpace> {
        if let Some(s) = ColorSpace::predefined(name) {
            return Some(s);
        }
        Some(match name.to_ascii_lowercase().as_str() {
            "lab" => ColorSpace::Lab,
            "oklab" => ColorSpace::Oklab,
            "hsl" => ColorSpace::Hsl,
            "hwb" => ColorSpace::Hwb,
            "lch" => ColorSpace::Lch,
            "oklch" => ColorSpace::Oklch,
            _ => return None,
        })
    }

    /// El índice del canal de tono de los espacios polares.
    pub fn hue_index(self) -> Option<usize> {
        match self {
            ColorSpace::Hsl | ColorSpace::Hwb => Some(0),
            ColorSpace::Lch | ColorSpace::Oklch => Some(2),
            _ => None,
        }
    }

    /// Coordenadas en este espacio → XYZ D65.
    pub fn to_xyz(self, c: Vec3) -> Vec3 {
        match self {
            ColorSpace::Srgb => mul(&SRGB_TO_XYZ, map3(c, srgb_to_linear)),
            ColorSpace::SrgbLinear => mul(&SRGB_TO_XYZ, c),
            ColorSpace::DisplayP3 => mul(&P3_TO_XYZ, map3(c, srgb_to_linear)),
            ColorSpace::A98Rgb => mul(&A98_TO_XYZ, map3(c, a98_to_linear)),
            ColorSpace::ProphotoRgb => mul(
                &D50_TO_D65,
                mul(&PROPHOTO_TO_XYZ_D50, map3(c, prophoto_to_linear)),
            ),
            ColorSpace::Rec2020 => mul(&REC2020_TO_XYZ, map3(c, rec2020_to_linear)),
            ColorSpace::XyzD65 => c,
            ColorSpace::XyzD50 => mul(&D50_TO_D65, c),
            ColorSpace::Lab => mul(&D50_TO_D65, lab_to_xyz_d50(c)),
            ColorSpace::Lch => mul(&D50_TO_D65, lab_to_xyz_d50(from_polar(c))),
            ColorSpace::Oklab => oklab_to_xyz(c),
            ColorSpace::Oklch => oklab_to_xyz(from_polar(c)),
            ColorSpace::Hsl => ColorSpace::Srgb.to_xyz(hsl_to_srgb(c)),
            ColorSpace::Hwb => ColorSpace::Srgb.to_xyz(hwb_to_srgb(c)),
        }
    }

    /// XYZ D65 → coordenadas en este espacio.
    pub fn from_xyz(self, xyz: Vec3) -> Vec3 {
        match self {
            ColorSpace::Srgb => map3(mul(&XYZ_TO_SRGB, xyz), srgb_from_linear),
            ColorSpace::SrgbLinear => mul(&XYZ_TO_SRGB, xyz),
            ColorSpace::DisplayP3 => map3(mul(&XYZ_TO_P3, xyz), srgb_from_linear),
            ColorSpace::A98Rgb => map3(mul(&XYZ_TO_A98, xyz), a98_from_linear),
            ColorSpace::ProphotoRgb => map3(
                mul(&XYZ_D50_TO_PROPHOTO, mul(&D65_TO_D50, xyz)),
                prophoto_from_linear,
            ),
            ColorSpace::Rec2020 => map3(mul(&XYZ_TO_REC2020, xyz), rec2020_from_linear),
            ColorSpace::XyzD65 => xyz,
            ColorSpace::XyzD50 => mul(&D65_TO_D50, xyz),
            ColorSpace::Lab => xyz_d50_to_lab(mul(&D65_TO_D50, xyz)),
            // Umbral de "powerless" de §9.3 y §9.4 (sin él el ruido numérico de un gris
            // daría un tono arbitrario).
            ColorSpace::Lch => to_polar(xyz_d50_to_lab(mul(&D65_TO_D50, xyz)), 0.0015),
            ColorSpace::Oklab => xyz_to_oklab(xyz),
            ColorSpace::Oklch => to_polar(xyz_to_oklab(xyz), 0.000_004),
            ColorSpace::Hsl => srgb_to_hsl(ColorSpace::Srgb.from_xyz(xyz)),
            ColorSpace::Hwb => srgb_to_hwb(ColorSpace::Srgb.from_xyz(xyz)),
        }
    }
}

/// sRGB en [0, 1] → coordenadas en `space` (sin pasar por XYZ cuando no hace falta).
pub fn from_srgb(space: ColorSpace, rgb: Vec3) -> Vec3 {
    match space {
        ColorSpace::Srgb => rgb,
        ColorSpace::Hsl => srgb_to_hsl(rgb),
        ColorSpace::Hwb => srgb_to_hwb(rgb),
        _ => space.from_xyz(ColorSpace::Srgb.to_xyz(rgb)),
    }
}

/// Coordenadas en `space` → sRGB en [0, 1], ajustado a la gama de sRGB (CSS Color 4 §13.2,
/// "CSS gamut mapping to an RGB destination").
pub fn to_srgb_mapped(space: ColorSpace, c: Vec3) -> Vec3 {
    let rgb = match space {
        ColorSpace::Srgb => c,
        ColorSpace::Hsl => hsl_to_srgb(c),
        ColorSpace::Hwb => hwb_to_srgb(c),
        _ => ColorSpace::Srgb.from_xyz(space.to_xyz(c)),
    };
    if in_gamut(rgb) {
        return map3(rgb, |v| v.clamp(0.0, 1.0));
    }
    gamut_map(ColorSpace::Oklch.from_xyz(space.to_xyz(c)))
}

fn in_gamut(rgb: Vec3) -> bool {
    const TOLERANCE: f64 = 0.000_1;
    rgb.iter()
        .all(|v| v.is_finite() && *v >= -TOLERANCE && *v <= 1.0 + TOLERANCE)
}

fn clip(rgb: Vec3) -> Vec3 {
    map3(rgb, |v| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) })
}

fn delta_eok(a: Vec3, b: Vec3) -> f64 {
    let (a, b) = (from_polar(a), from_polar(b));
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// El algoritmo de §13.2 a partir del color en OKLCh.
fn gamut_map(origin: Vec3) -> Vec3 {
    const JND: f64 = 0.02;
    const EPS: f64 = 0.000_1;
    let [l, c, h] = origin;
    if l >= 1.0 {
        return [1.0; 3];
    }
    if l <= 0.0 || !l.is_finite() {
        return [0.0; 3];
    }
    let srgb = |oklch: Vec3| ColorSpace::Srgb.from_xyz(ColorSpace::Oklch.to_xyz(oklch));
    let oklch = |rgb: Vec3| ColorSpace::Oklch.from_xyz(ColorSpace::Srgb.to_xyz(rgb));
    let mut current = origin;
    let mut clipped = clip(srgb(current));
    if delta_eok(oklch(clipped), current) < JND {
        return clipped;
    }
    let (mut min, mut max) = (0.0, c);
    let mut min_in_gamut = true;
    while max - min > EPS {
        let chroma = (min + max) / 2.0;
        current = [l, chroma, h];
        let rgb = srgb(current);
        if min_in_gamut && in_gamut(rgb) {
            min = chroma;
            continue;
        }
        clipped = clip(rgb);
        let e = delta_eok(oklch(clipped), current);
        if e < JND {
            if JND - e < EPS {
                return clipped;
            }
            min_in_gamut = false;
            min = chroma;
        } else {
            max = chroma;
        }
    }
    clipped
}

/// Un color en `space` con alfa → [`Rgba`].
pub fn to_rgba(space: ColorSpace, c: Vec3, alpha: f64) -> Rgba {
    let [r, g, b] = to_srgb_mapped(space, c);
    Rgba {
        r: (r * 255.0) as f32,
        g: (g * 255.0) as f32,
        b: (b * 255.0) as f32,
        a: alpha.clamp(0.0, 1.0) as f32,
    }
}

/// [`Rgba`] → coordenadas en `space`.
pub fn from_rgba(space: ColorSpace, c: Rgba) -> Vec3 {
    let n = |v: f32| f64::from(v) / 255.0;
    from_srgb(space, [n(c.r), n(c.g), n(c.b)])
}

/// Interpola dos colores en `space` (CSS Color 4 §12: alfa premultiplicado, tonos según
/// `hue`, componentes que faltan tomados del otro color). `t` es el peso del segundo.
pub fn interpolate(space: ColorSpace, hue: HueMethod, a: Rgba, b: Rgba, t: f64) -> Rgba {
    let (mut ca, mut cb) = (from_rgba(space, a), from_rgba(space, b));
    let (aa, ab) = (f64::from(a.a), f64::from(b.a));
    let hi = space.hue_index();
    // §12.2: un tono que falta toma el del otro color.
    if let Some(i) = hi {
        match (ca[i].is_nan(), cb[i].is_nan()) {
            (true, false) => ca[i] = cb[i],
            (false, true) => cb[i] = ca[i],
            (true, true) => {
                ca[i] = 0.0;
                cb[i] = 0.0;
            }
            _ => {}
        }
        let (h1, h2) = fix_hues(ca[i], cb[i], hue);
        ca[i] = h1;
        cb[i] = h2;
    }
    let alpha = aa + (ab - aa) * t;
    let mut out = [0.0; 3];
    for k in 0..3 {
        out[k] = if Some(k) == hi {
            ca[k] + (cb[k] - ca[k]) * t
        } else {
            // §12.3: premultiplicado.
            let v = ca[k] * aa + (cb[k] * ab - ca[k] * aa) * t;
            if alpha == 0.0 { v } else { v / alpha }
        };
    }
    if let Some(i) = hi {
        out[i] = out[i].rem_euclid(360.0);
    }
    to_rgba(space, out, alpha)
}

/// §12.4: ajusta los tonos según el método de interpolación.
fn fix_hues(h1: f64, h2: f64, method: HueMethod) -> (f64, f64) {
    let (mut h1, mut h2) = (h1.rem_euclid(360.0), h2.rem_euclid(360.0));
    let d = h2 - h1;
    match method {
        HueMethod::Shorter => {
            if d > 180.0 {
                h1 += 360.0;
            } else if d < -180.0 {
                h2 += 360.0;
            }
        }
        HueMethod::Longer => {
            if 0.0 < d && d < 180.0 {
                h1 += 360.0;
            } else if -180.0 < d && d <= 0.0 {
                h2 += 360.0;
            }
        }
        HueMethod::Increasing => {
            if d < 0.0 {
                h2 += 360.0;
            }
        }
        HueMethod::Decreasing => {
            if d > 0.0 {
                h1 += 360.0;
            }
        }
    }
    (h1, h2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3, eps: f64) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < eps)
    }

    #[test]
    fn round_trips() {
        let rgb = [0.2, 0.6, 0.9];
        for space in [
            ColorSpace::SrgbLinear,
            ColorSpace::DisplayP3,
            ColorSpace::A98Rgb,
            ColorSpace::ProphotoRgb,
            ColorSpace::Rec2020,
            ColorSpace::XyzD50,
            ColorSpace::XyzD65,
            ColorSpace::Lab,
            ColorSpace::Lch,
            ColorSpace::Oklab,
            ColorSpace::Oklch,
            ColorSpace::Hsl,
            ColorSpace::Hwb,
        ] {
            let c = from_srgb(space, rgb);
            assert!(
                close(to_srgb_mapped(space, c), rgb, 1e-6),
                "{space:?}: {c:?}"
            );
        }
    }

    #[test]
    fn known_values() {
        // Ejemplos de CSS Color 4: oklab de blanco y rojo, lab de rojo sRGB.
        let white = from_srgb(ColorSpace::Oklab, [1.0, 1.0, 1.0]);
        assert!(close(white, [1.0, 0.0, 0.0], 1e-4), "{white:?}");
        let red = from_srgb(ColorSpace::Oklab, [1.0, 0.0, 0.0]);
        assert!(close(red, [0.627_96, 0.224_86, 0.125_85], 1e-4), "{red:?}");
        let lab = from_srgb(ColorSpace::Lab, [1.0, 0.0, 0.0]);
        assert!(close(lab, [54.29, 80.80, 69.89], 0.05), "{lab:?}");
    }

    #[test]
    fn out_of_gamut_is_mapped() {
        // display-p3 rojo puro queda fuera de sRGB: se ajusta a un rojo dentro de la gama.
        let rgb = to_srgb_mapped(ColorSpace::DisplayP3, [1.0, 0.0, 0.0]);
        assert!(rgb.iter().all(|v| (0.0..=1.0).contains(v)));
        assert!(rgb[0] > 0.95 && rgb[1] < 0.2 && rgb[2] < 0.2, "{rgb:?}");
        // L ≥ 1 es blanco.
        assert_eq!(
            to_srgb_mapped(ColorSpace::Oklch, [1.2, 0.3, 40.0]),
            [1.0; 3]
        );
    }
}
