//! Selección de caras (CSS Fonts 4 §5.2) y familias genéricas por plataforma.

use crate::face::FaceStyle;

/// Descriptores de una cara para el matching. Las caras del sistema tienen un solo peso
/// (`weight.0 == weight.1`); las de `@font-face` pueden declarar un rango.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Descriptors {
    pub weight: (f32, f32),
    pub style: FaceStyle,
    pub stretch: u16,
}

/// Rango de `font-stretch` (solo se pide `normal`, 5): primero 5, después los más estrechos
/// de mayor a menor y luego los más anchos (§5.2 paso 4a).
fn stretch_rank(stretch: u16) -> (u8, u16) {
    match stretch {
        5 => (0, 0),
        s if s < 5 => (1, 5 - s),
        s => (2, s - 5),
    }
}

/// §5.2 paso 4b. `oblique` se pide como `italic` (el estilo computado no distingue ángulos).
fn style_rank(style: FaceStyle, italic: bool) -> u8 {
    match (italic, style) {
        (true, FaceStyle::Italic) | (false, FaceStyle::Normal) => 0,
        (_, FaceStyle::Oblique) => 1,
        _ => 2,
    }
}

/// §5.2 paso 4c: orden de preferencia de pesos para el peso pedido `w`.
fn weight_rank((min, max): (f32, f32), w: f32) -> (u8, f32) {
    if (min..=max).contains(&w) {
        return (0, 0.0);
    }
    let above = min > w;
    let distance = if above { min - w } else { w - max };
    let class = if (400.0..=500.0).contains(&w) {
        match (above, min <= 500.0) {
            (true, true) => 1,
            (false, _) => 2,
            (true, false) => 3,
        }
    } else if w < 400.0 {
        if above { 2 } else { 1 }
    } else if above {
        1
    } else {
        2
    };
    (class, distance)
}

/// Índice de la mejor cara de `faces` para el peso y estilo pedidos.
pub fn best_match(faces: &[Descriptors], weight: f32, italic: bool) -> Option<usize> {
    let key = |d: &Descriptors| {
        let (wc, wd) = weight_rank(d.weight, weight);
        (stretch_rank(d.stretch), style_rank(d.style, italic), wc, wd)
    };
    (0..faces.len()).min_by(|&a, &b| {
        key(&faces[a])
            .partial_cmp(&key(&faces[b]))
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// Familias candidatas para cada genérica, en orden. Se incluyen las de todas las plataformas
/// (primero las de la actual); se usa la primera que exista.
pub fn generic_candidates(generic: &str) -> &'static [&'static str] {
    #[cfg(windows)]
    const TABLE: &[(&str, &[&str])] = &[
        (
            "serif",
            &[
                "Times New Roman",
                "Liberation Serif",
                "DejaVu Serif",
                "Times",
            ],
        ),
        (
            "sans-serif",
            &["Arial", "Liberation Sans", "DejaVu Sans", "Helvetica"],
        ),
        (
            "monospace",
            &[
                "Consolas",
                "Courier New",
                "Liberation Mono",
                "DejaVu Sans Mono",
            ],
        ),
        ("cursive", &["Comic Sans MS", "Segoe Script", "Gabriola"]),
        ("fantasy", &["Impact", "Gabriola", "Comic Sans MS"]),
        ("system-ui", &["Segoe UI", "Tahoma", "Arial"]),
        ("ui-serif", &["Times New Roman", "Cambria"]),
        ("ui-sans-serif", &["Segoe UI", "Arial"]),
        (
            "ui-monospace",
            &["Cascadia Mono", "Consolas", "Courier New"],
        ),
        ("ui-rounded", &["Segoe UI", "Arial"]),
        ("math", &["Cambria Math", "Times New Roman"]),
        ("emoji", &["Segoe UI Emoji", "Segoe UI Symbol"]),
        ("fangsong", &["FangSong", "SimSun"]),
    ];
    #[cfg(target_os = "macos")]
    const TABLE: &[(&str, &[&str])] = &[
        ("serif", &["Times", "Times New Roman"]),
        ("sans-serif", &["Helvetica", "Arial"]),
        ("monospace", &["Menlo", "Courier", "Courier New"]),
        ("cursive", &["Apple Chancery", "Snell Roundhand"]),
        ("fantasy", &["Papyrus", "Herculanum"]),
        (
            "system-ui",
            &["SF Pro", "SF Pro Text", "Helvetica Neue", "Helvetica"],
        ),
        ("ui-serif", &["New York", "Times"]),
        ("ui-sans-serif", &["SF Pro", "Helvetica Neue"]),
        ("ui-monospace", &["SF Mono", "Menlo"]),
        ("ui-rounded", &["SF Pro Rounded", "Helvetica Neue"]),
        ("math", &["STIX Two Math", "Times"]),
        ("emoji", &["Apple Color Emoji"]),
        ("fangsong", &["STFangsong"]),
    ];
    #[cfg(not(any(windows, target_os = "macos")))]
    const TABLE: &[(&str, &[&str])] = &[
        (
            "serif",
            &[
                "DejaVu Serif",
                "Liberation Serif",
                "Noto Serif",
                "FreeSerif",
            ],
        ),
        (
            "sans-serif",
            &["DejaVu Sans", "Liberation Sans", "Noto Sans", "FreeSans"],
        ),
        (
            "monospace",
            &[
                "DejaVu Sans Mono",
                "Liberation Mono",
                "Noto Sans Mono",
                "FreeMono",
            ],
        ),
        ("cursive", &["URW Chancery L", "Z003", "DejaVu Sans"]),
        ("fantasy", &["DejaVu Sans", "Liberation Sans"]),
        (
            "system-ui",
            &["Cantarell", "Ubuntu", "Noto Sans", "DejaVu Sans"],
        ),
        ("ui-serif", &["DejaVu Serif", "Noto Serif"]),
        ("ui-sans-serif", &["DejaVu Sans", "Noto Sans"]),
        ("ui-monospace", &["DejaVu Sans Mono", "Noto Sans Mono"]),
        ("ui-rounded", &["DejaVu Sans", "Noto Sans"]),
        ("math", &["DejaVu Math TeX Gyre", "Noto Sans Math"]),
        ("emoji", &["Noto Color Emoji"]),
        ("fangsong", &["AR PL UKai CN"]),
    ];
    TABLE
        .iter()
        .find(|(g, _)| g.eq_ignore_ascii_case(generic))
        .map_or(&[], |(_, list)| list)
}

/// Familias que se prueban primero en el fallback por carácter.
#[cfg(windows)]
pub const FALLBACK_FAMILIES: &[&str] = &[
    "Segoe UI",
    "Arial",
    "Segoe UI Symbol",
    "Segoe UI Emoji",
    "Segoe UI Historic",
    "Microsoft YaHei",
    "Yu Gothic",
    "Malgun Gothic",
    "Nirmala UI",
    "Ebrima",
    "Gadugi",
    "Leelawadee UI",
    "Microsoft Himalaya",
    "Mongolian Baiti",
    "Myanmar Text",
    "Javanese Text",
    "Cambria Math",
];

#[cfg(target_os = "macos")]
pub const FALLBACK_FAMILIES: &[&str] = &[
    "Helvetica",
    "Lucida Grande",
    "Apple Symbols",
    "Apple Color Emoji",
    "PingFang SC",
    "Hiragino Sans",
    "Apple SD Gothic Neo",
    "Kohinoor Devanagari",
];

#[cfg(not(any(windows, target_os = "macos")))]
pub const FALLBACK_FAMILIES: &[&str] = &[
    "DejaVu Sans",
    "Noto Sans",
    "Liberation Sans",
    "Noto Sans Symbols",
    "Noto Sans Symbols2",
    "Noto Color Emoji",
    "Noto Sans CJK SC",
    "Noto Sans Devanagari",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn d(w: f32, style: FaceStyle) -> Descriptors {
        Descriptors {
            weight: (w, w),
            style,
            stretch: 5,
        }
    }

    #[test]
    fn weights() {
        let faces = [
            d(300.0, FaceStyle::Normal),
            d(400.0, FaceStyle::Normal),
            d(600.0, FaceStyle::Normal),
            d(900.0, FaceStyle::Normal),
        ];
        assert_eq!(best_match(&faces, 400.0, false), Some(1));
        // 500 sin cara exacta: primero los menores (hasta 500 no hay más), luego 400.
        assert_eq!(best_match(&faces, 500.0, false), Some(1));
        assert_eq!(best_match(&faces, 700.0, false), Some(3));
        assert_eq!(best_match(&faces, 350.0, false), Some(0));
        assert_eq!(best_match(&faces, 200.0, false), Some(0));
        assert_eq!(best_match(&faces[2..], 450.0, false), Some(0));
        let ranged = [Descriptors {
            weight: (100.0, 900.0),
            style: FaceStyle::Normal,
            stretch: 5,
        }];
        assert_eq!(best_match(&ranged, 650.0, false), Some(0));
    }

    #[test]
    fn styles() {
        let faces = [d(400.0, FaceStyle::Normal), d(400.0, FaceStyle::Oblique)];
        assert_eq!(best_match(&faces, 400.0, true), Some(1));
        let faces = [d(700.0, FaceStyle::Italic), d(400.0, FaceStyle::Normal)];
        // El estilo pesa más que el peso.
        assert_eq!(best_match(&faces, 400.0, true), Some(0));
    }
}
