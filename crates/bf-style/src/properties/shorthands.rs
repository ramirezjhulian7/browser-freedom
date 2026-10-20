//! Shorthands y su expansión a longhands. Las propiedades lógicas (`margin-inline-start`,
//! `padding-block`, `inset-inline`, ...) se registran aquí como alias de las físicas, mapeadas
//! con `writing-mode: horizontal-tb` y `direction: ltr` (aún no hay writing modes).

use std::collections::HashMap;
use std::sync::OnceLock;

use bf_css::syntax::{ComponentValue, TokenKind, split_comma_separated};

use super::types::*;
use super::{LonghandId as L, PropValue, Value};
use crate::values::{Color, components, ident, is_ident, trim};

pub struct ShorthandDef {
    pub name: &'static str,
    pub longhands: &'static [L],
    /// Expande el valor (ya sin whitespace en los extremos) a valores de longhands.
    pub expand: fn(&[ComponentValue]) -> Option<Vec<Value>>,
}

impl std::fmt::Debug for ShorthandDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name)
    }
}

const MARGINS: &[L] = &[L::MarginTop, L::MarginRight, L::MarginBottom, L::MarginLeft];
const PADDINGS: &[L] = &[
    L::PaddingTop,
    L::PaddingRight,
    L::PaddingBottom,
    L::PaddingLeft,
];
const INSETS: &[L] = &[L::Top, L::Right, L::Bottom, L::Left];
const BORDER_WIDTHS: &[L] = &[
    L::BorderTopWidth,
    L::BorderRightWidth,
    L::BorderBottomWidth,
    L::BorderLeftWidth,
];
const BORDER_STYLES: &[L] = &[
    L::BorderTopStyle,
    L::BorderRightStyle,
    L::BorderBottomStyle,
    L::BorderLeftStyle,
];
const BORDER_COLORS: &[L] = &[
    L::BorderTopColor,
    L::BorderRightColor,
    L::BorderBottomColor,
    L::BorderLeftColor,
];
const RADII: &[L] = &[
    L::BorderTopLeftRadius,
    L::BorderTopRightRadius,
    L::BorderBottomRightRadius,
    L::BorderBottomLeftRadius,
];
const BORDER: &[L] = &[
    L::BorderTopWidth,
    L::BorderRightWidth,
    L::BorderBottomWidth,
    L::BorderLeftWidth,
    L::BorderTopStyle,
    L::BorderRightStyle,
    L::BorderBottomStyle,
    L::BorderLeftStyle,
    L::BorderTopColor,
    L::BorderRightColor,
    L::BorderBottomColor,
    L::BorderLeftColor,
];

/// Parsea un único component value con el parser del longhand `id`.
fn one(id: L, v: &ComponentValue) -> Option<Value> {
    (id.def().parse)(std::slice::from_ref(v))
}

/// `[a]{1,4}` con la regla de la caja (top, right, bottom, left).
fn box4(ids: &[L], input: &[ComponentValue]) -> Option<Vec<Value>> {
    let items = components(input);
    let parsed = items
        .iter()
        .enumerate()
        .map(|(i, v)| one(ids[i.min(3)], v))
        .collect::<Option<Vec<_>>>()?;
    let pick = |i: usize| -> Option<usize> {
        Some(match (parsed.len(), i) {
            (1, _) => 0,
            (2, 0 | 2) => 0,
            (2, _) => 1,
            (3, 0) => 0,
            (3, 1 | 3) => 1,
            (3, _) => 2,
            (4, i) => i,
            _ => return None,
        })
    };
    (0..4)
        .map(|i| {
            let src = pick(i)?;
            // Re-parsear con el longhand de esta posición para que el `Value` tenga su id.
            one(ids[i], items[src])
        })
        .collect()
}

/// `[a]{1,2}` para pares lógicos (start, end).
fn pair(ids: [L; 2], input: &[ComponentValue]) -> Option<Vec<Value>> {
    match components(input).as_slice() {
        [a] => Some(vec![one(ids[0], a)?, one(ids[1], a)?]),
        [a, b] => Some(vec![one(ids[0], a)?, one(ids[1], b)?]),
        _ => None,
    }
}

/// `a || b || c`: cada component value a la primera ranura libre que lo acepte. Las ranuras
/// no usadas toman su valor inicial (por eso el shorthand "resetea" todo).
fn any_order(slots: &[L], input: &[ComponentValue]) -> Option<Vec<Option<Value>>> {
    let mut out: Vec<Option<Value>> = vec![None; slots.len()];
    let items = components(input);
    if items.is_empty() {
        return None;
    }
    for v in items {
        let slot = (0..slots.len()).find(|&i| out[i].is_none() && one(slots[i], v).is_some())?;
        out[slot] = one(slots[slot], v);
    }
    Some(out)
}

fn initial_of(id: L) -> Value {
    super::initial_values()[id as usize].clone()
}

/// Reemplaza el id de un valor por el de otro longhand del mismo tipo (para repetirlo en los
/// cuatro lados).
fn retarget(value: &Value, to: L) -> Value {
    let css = (value.id().def().to_css)(value);
    let tokens = bf_css::syntax::parse_component_value_list(&css);
    (to.def().parse)(trim(&tokens)).unwrap_or_else(|| value.clone())
}

fn expand_side(ids: [L; 3], input: &[ComponentValue]) -> Option<Vec<Value>> {
    let parsed = any_order(&ids, input)?;
    Some(
        parsed
            .into_iter()
            .zip(ids)
            .map(|(v, id)| v.unwrap_or_else(|| initial_of(id)))
            .collect(),
    )
}

fn border(input: &[ComponentValue]) -> Option<Vec<Value>> {
    let side = expand_side(
        [L::BorderTopWidth, L::BorderTopStyle, L::BorderTopColor],
        input,
    )?;
    let mut out = Vec::new();
    for (i, group) in [BORDER_WIDTHS, BORDER_STYLES, BORDER_COLORS]
        .iter()
        .enumerate()
    {
        for &id in *group {
            out.push(retarget(&side[i], id));
        }
    }
    Some(out)
}

fn border_radius(input: &[ComponentValue]) -> Option<Vec<Value>> {
    let items = components(input);
    let slash = items
        .iter()
        .position(|v| v.token() == Some(&TokenKind::Delim('/')));
    let (h, v) = match slash {
        Some(i) => (&items[..i], &items[i + 1..]),
        None => (&items[..], &items[..]),
    };
    let expand = |vals: &[&ComponentValue]| -> Option<Vec<crate::values::LengthPercentage>> {
        let parsed = vals
            .iter()
            .map(|v| crate::values::parse_non_negative_lp(v))
            .collect::<Option<Vec<_>>>()?;
        let idx: [usize; 4] = match parsed.len() {
            1 => [0, 0, 0, 0],
            2 => [0, 1, 0, 1],
            3 => [0, 1, 2, 1],
            4 => [0, 1, 2, 3],
            _ => return None,
        };
        Some(idx.iter().map(|&i| parsed[i].clone()).collect())
    };
    let (h, v) = (expand(h)?, expand(v)?);
    Some(
        RADII
            .iter()
            .enumerate()
            .map(|(i, &id)| {
                let r = CornerRadius(h[i].clone(), v[i].clone());
                match id {
                    L::BorderTopLeftRadius => Value::BorderTopLeftRadius(r),
                    L::BorderTopRightRadius => Value::BorderTopRightRadius(r),
                    L::BorderBottomRightRadius => Value::BorderBottomRightRadius(r),
                    _ => Value::BorderBottomLeftRadius(r),
                }
            })
            .collect(),
    )
}

fn overflow(input: &[ComponentValue]) -> Option<Vec<Value>> {
    pair([L::OverflowX, L::OverflowY], input)
}

/// Keywords de los longhands de `background` que todavía no se soportan: se aceptan y se
/// descartan para no invalidar el shorthand entero.
/// `background` (CSS Backgrounds 3 §3.10): por capa `<bg-image> || <bg-position> [ / <bg-size> ]?
/// || <repeat-style> || <attachment> || <box> || <box>`, y el color solo en la última.
fn background(input: &[ComponentValue]) -> Option<Vec<Value>> {
    let layers = split_comma_separated(input);
    let n = layers.len().max(1);
    let mut color = Color::Rgba(crate::values::Rgba::TRANSPARENT);
    let mut images = vec![Image::None; n];
    let mut positions = vec![(PositionAxis::default_start(), PositionAxis::default_start()); n];
    let mut sizes = vec![BgSize::Explicit(None, None); n];
    let mut repeats = vec![(RepeatStyle::Repeat, RepeatStyle::Repeat); n];
    let mut attachments = vec![Attachment::Scroll; n];
    let mut origins = vec![BackgroundBox::PaddingBox; n];
    let mut clips = vec![BackgroundBox::BorderBox; n];
    let is_position_token = |v: &ComponentValue| {
        ident(v).is_some_and(|i| {
            ["left", "right", "top", "bottom", "center"]
                .iter()
                .any(|k| k.eq_ignore_ascii_case(i))
        }) || crate::values::parse_length_percentage(v).is_some()
    };
    for (i, layer) in layers.iter().enumerate() {
        let last = i == layers.len() - 1;
        let items = components(layer);
        let (mut saw_color, mut saw_image, mut saw_position, mut saw_repeat, mut saw_attach) =
            (false, false, false, false, false);
        let mut boxes = 0;
        let mut k = 0;
        while k < items.len() {
            let v = items[k];
            if !saw_image && let Some(Value::BackgroundImage(img)) = one(L::BackgroundImage, v) {
                images[i] = img.0.into_iter().next().unwrap_or(Image::None);
                saw_image = true;
                k += 1;
                continue;
            }
            if !saw_position && is_position_token(v) {
                let run_end = (k..items.len())
                    .take_while(|&j| is_position_token(items[j]))
                    .last()
                    .map_or(k, |j| j + 1)
                    .min(k + 4);
                let run: Vec<ComponentValue> =
                    items[k..run_end].iter().map(|v| (*v).clone()).collect();
                positions[i] = parse_bg_position(&run)?;
                saw_position = true;
                k = run_end;
                if items.get(k).and_then(|v| v.token()) == Some(&TokenKind::Delim('/')) {
                    k += 1;
                    let size_end = (k..items.len())
                        .take_while(|&j| {
                            ident(items[j]).is_some_and(|i| {
                                ["auto", "cover", "contain"]
                                    .iter()
                                    .any(|x| x.eq_ignore_ascii_case(i))
                            }) || crate::values::parse_length_percentage(items[j]).is_some()
                        })
                        .last()
                        .map_or(k, |j| j + 1)
                        .min(k + 2);
                    let run: Vec<ComponentValue> =
                        items[k..size_end].iter().map(|v| (*v).clone()).collect();
                    let Some(Value::BackgroundSize(bs)) = (L::BackgroundSize.def().parse)(&run)
                    else {
                        return None;
                    };
                    sizes[i] = bs.0.into_iter().next()?;
                    k = size_end;
                }
                continue;
            }
            if !saw_repeat && ident(v).is_some() {
                let two: Vec<ComponentValue> = items[k..(k + 2).min(items.len())]
                    .iter()
                    .map(|v| (*v).clone())
                    .collect();
                if two.len() == 2
                    && let Some(Value::BackgroundRepeat(r)) =
                        (L::BackgroundRepeat.def().parse)(&two)
                {
                    repeats[i] = r.0[0];
                    saw_repeat = true;
                    k += 2;
                    continue;
                }
                if let Some(Value::BackgroundRepeat(r)) = one(L::BackgroundRepeat, v) {
                    repeats[i] = r.0[0];
                    saw_repeat = true;
                    k += 1;
                    continue;
                }
            }
            if !saw_attach && let Some(a) = ident(v).and_then(Attachment::from_ident) {
                attachments[i] = a;
                saw_attach = true;
                k += 1;
                continue;
            }
            if let Some(b) = ident(v).and_then(BackgroundBox::from_ident)
                && b != BackgroundBox::BorderArea
                && boxes < 2
            {
                if boxes == 0 {
                    origins[i] = if b == BackgroundBox::Text {
                        origins[i]
                    } else {
                        b
                    };
                }
                clips[i] = b;
                boxes += 1;
                k += 1;
                continue;
            }
            if last
                && !saw_color
                && let Some(Value::BackgroundColor(c)) = one(L::BackgroundColor, v)
            {
                color = c;
                saw_color = true;
                k += 1;
                continue;
            }
            return None;
        }
    }
    Some(vec![
        Value::BackgroundColor(color),
        Value::BackgroundImage(BackgroundImages(images)),
        Value::BackgroundClip(BackgroundClip(clips)),
        Value::BackgroundRepeat(BackgroundRepeat(repeats)),
        Value::BackgroundPosition(BackgroundPosition(positions)),
        Value::BackgroundSize(BackgroundSize(sizes)),
        Value::BackgroundOrigin(BackgroundOrigin(origins)),
        Value::BackgroundAttachment(BackgroundAttachment(attachments)),
    ])
}

const SYSTEM_FONTS: &[&str] = &[
    "caption",
    "icon",
    "menu",
    "message-box",
    "small-caption",
    "status-bar",
];

/// `font` (CSS Fonts 4 §2.10).
fn font(input: &[ComponentValue]) -> Option<Vec<Value>> {
    let items = components(input);
    let mut style = FontStyle::Normal;
    let mut weight = FontWeight::Absolute(400.0);
    if let [v] = items.as_slice()
        && ident(v).is_some_and(|i| SYSTEM_FONTS.iter().any(|s| s.eq_ignore_ascii_case(i)))
    {
        // Fuentes del sistema: se usa la familia `system-ui` con un tamaño fijo.
        return Some(vec![
            Value::FontStyle(style),
            Value::FontWeight(weight),
            Value::FontSize(FontSize::Px(13.0)),
            Value::LineHeight(LineHeight::Normal),
            Value::FontFamily(FontFamily(vec![FamilyName::Generic("system-ui".into())])),
        ]);
    }
    let mut i = 0;
    let (mut saw_style, mut saw_weight, mut saw_variant, mut saw_stretch) =
        (false, false, false, false);
    let mut normals = 0;
    while i < items.len() {
        let v = items[i];
        if is_ident(v, "normal") {
            normals += 1;
        } else if !saw_style && let Some(s) = ident(v).and_then(FontStyle::from_ident) {
            style = s;
            saw_style = true;
        } else if !saw_variant && is_ident(v, "small-caps") {
            saw_variant = true;
        } else if !saw_weight
            && let Some(w) = FontWeight::parse(std::slice::from_ref(v))
            && (ident(v).is_some() || matches!(v.token(), Some(TokenKind::Number(_))))
        {
            // Un número solo es peso si no es el tamaño (el tamaño necesita unidad o %).
            weight = w;
            saw_weight = true;
        } else if !saw_stretch
            && ident(v).is_some_and(|i| {
                [
                    "ultra-condensed",
                    "extra-condensed",
                    "condensed",
                    "semi-condensed",
                    "semi-expanded",
                    "expanded",
                    "extra-expanded",
                    "ultra-expanded",
                ]
                .iter()
                .any(|k| k.eq_ignore_ascii_case(i))
            })
        {
            saw_stretch = true;
        } else {
            break;
        }
        i += 1;
    }
    let used = [saw_style, saw_weight, saw_variant, saw_stretch]
        .iter()
        .filter(|b| **b)
        .count();
    if used + normals > 4 {
        return None;
    }
    let size = FontSize::parse(std::slice::from_ref(items.get(i)?))?;
    i += 1;
    let mut line_height = LineHeight::Normal;
    if items.get(i).and_then(|v| v.token()) == Some(&TokenKind::Delim('/')) {
        line_height = LineHeight::parse(std::slice::from_ref(items.get(i + 1)?))?;
        i += 2;
    }
    // La familia es el resto del valor (con sus comas y whitespace originales).
    let family_start = items.get(i).map(|v| v.start())?;
    let family_values: Vec<ComponentValue> = input
        .iter()
        .filter(|v| v.start() >= family_start)
        .cloned()
        .collect();
    let family = FontFamily::parse(trim(&family_values))?;
    Some(vec![
        Value::FontStyle(style),
        Value::FontWeight(weight),
        Value::FontSize(size),
        Value::LineHeight(line_height),
        Value::FontFamily(family),
    ])
}

fn text_decoration(input: &[ComponentValue]) -> Option<Vec<Value>> {
    // `text-decoration-line` acepta varias keywords: se juntan antes de repartir el resto.
    let items = components(input);
    let mut line_tokens = Vec::new();
    let mut rest = Vec::new();
    for v in items {
        if ident(v).is_some_and(|i| {
            ["underline", "overline", "line-through", "blink", "none"]
                .iter()
                .any(|k| k.eq_ignore_ascii_case(i))
        }) {
            line_tokens.push(v.clone());
        } else {
            rest.push(v.clone());
        }
    }
    let line = if line_tokens.is_empty() {
        initial_of(L::TextDecorationLine)
    } else {
        Value::TextDecorationLine(TextDecorationLine::parse(&line_tokens)?)
    };
    let mut style = initial_of(L::TextDecorationStyle);
    let mut color = initial_of(L::TextDecorationColor);
    let parsed = if rest.is_empty() {
        vec![None, None]
    } else {
        any_order(&[L::TextDecorationStyle, L::TextDecorationColor], &rest)?
    };
    if let Some(s) = parsed[0].clone() {
        style = s;
    }
    if let Some(c) = parsed[1].clone() {
        color = c;
    }
    Some(vec![line, style, color])
}

fn list_style(input: &[ComponentValue]) -> Option<Vec<Value>> {
    let items = components(input);
    let nones = items.iter().filter(|v| is_ident(v, "none")).count();
    let rest: Vec<ComponentValue> = items
        .iter()
        .filter(|v| !is_ident(v, "none"))
        .map(|v| (*v).clone())
        .collect();
    let slots = [L::ListStylePosition, L::ListStyleImage, L::ListStyleType];
    let mut parsed = if rest.is_empty() {
        vec![None, None, None]
    } else {
        any_order(&slots, &rest)?
    };
    // `none` va a la imagen y/o al tipo, los que no se hayan dado (CSS Lists 3 §4).
    let mut nones_left = nones;
    for i in [2, 1] {
        if nones_left > 0 && parsed[i].is_none() {
            parsed[i] = Some(if i == 2 {
                Value::ListStyleType(ListStyleType::None)
            } else {
                Value::ListStyleImage(Image::None)
            });
            nones_left -= 1;
        }
    }
    if nones_left > 0 {
        return None;
    }
    Some(
        parsed
            .into_iter()
            .zip(slots)
            .map(|(v, id)| v.unwrap_or_else(|| initial_of(id)))
            .collect(),
    )
}

fn flex(input: &[ComponentValue]) -> Option<Vec<Value>> {
    let items = components(input);
    let num = |n: f32| NonNegativeNumber(n);
    let out = |g: f32, s: f32, b: FlexBasis| {
        vec![
            Value::FlexGrow(num(g)),
            Value::FlexShrink(num(s)),
            Value::FlexBasis(b),
        ]
    };
    if let [v] = items.as_slice()
        && is_ident(v, "none")
    {
        return Some(out(0.0, 0.0, FlexBasis::Size(Size::Auto)));
    }
    if let [v] = items.as_slice()
        && is_ident(v, "auto")
    {
        return Some(out(1.0, 1.0, FlexBasis::Size(Size::Auto)));
    }
    let mut grow = None;
    let mut shrink = None;
    let mut basis = None;
    let mut i = 0;
    while i < items.len() {
        let v = items[i];
        if grow.is_none()
            && let Some(Value::FlexGrow(g)) = one(L::FlexGrow, v)
        {
            grow = Some(g.0);
            if let Some(Value::FlexShrink(s)) = items.get(i + 1).and_then(|n| one(L::FlexShrink, n))
            {
                shrink = Some(s.0);
                i += 1;
            }
        } else if basis.is_none()
            && let Some(Value::FlexBasis(b)) = one(L::FlexBasis, v)
        {
            basis = Some(b);
        } else {
            return None;
        }
        i += 1;
    }
    // Omitir el basis con un número lo deja en 0 (CSS Flexbox 1 §7.2).
    let basis = basis.unwrap_or(FlexBasis::Size(Size::LengthPercentage(
        crate::values::LengthPercentage::Percentage(0.0),
    )));
    Some(out(grow.unwrap_or(1.0), shrink.unwrap_or(1.0), basis))
}

fn flex_flow(input: &[ComponentValue]) -> Option<Vec<Value>> {
    let slots = [L::FlexDirection, L::FlexWrap];
    let parsed = any_order(&slots, input)?;
    Some(
        parsed
            .into_iter()
            .zip(slots)
            .map(|(v, id)| v.unwrap_or_else(|| initial_of(id)))
            .collect(),
    )
}

fn gap(input: &[ComponentValue]) -> Option<Vec<Value>> {
    pair([L::RowGap, L::ColumnGap], input)
}

fn place(ids: [L; 2], input: &[ComponentValue]) -> Option<Vec<Value>> {
    pair(ids, input)
}

/// Alias de un solo longhand (propiedades lógicas).
macro_rules! alias {
    ($name:literal => $id:ident) => {
        ShorthandDef {
            name: $name,
            longhands: &[L::$id],
            expand: |input| Some(vec![(L::$id.def().parse)(input)?]),
        }
    };
}

pub static SHORTHANDS: &[ShorthandDef] = &[
    ShorthandDef {
        name: "margin",
        longhands: MARGINS,
        expand: |i| box4(MARGINS, i),
    },
    ShorthandDef {
        name: "padding",
        longhands: PADDINGS,
        expand: |i| box4(PADDINGS, i),
    },
    ShorthandDef {
        name: "inset",
        longhands: INSETS,
        expand: |i| box4(INSETS, i),
    },
    ShorthandDef {
        name: "border-width",
        longhands: BORDER_WIDTHS,
        expand: |i| box4(BORDER_WIDTHS, i),
    },
    ShorthandDef {
        name: "border-style",
        longhands: BORDER_STYLES,
        expand: |i| box4(BORDER_STYLES, i),
    },
    ShorthandDef {
        name: "border-color",
        longhands: BORDER_COLORS,
        expand: |i| box4(BORDER_COLORS, i),
    },
    ShorthandDef {
        name: "border-top",
        longhands: &[L::BorderTopWidth, L::BorderTopStyle, L::BorderTopColor],
        expand: |i| expand_side([L::BorderTopWidth, L::BorderTopStyle, L::BorderTopColor], i),
    },
    ShorthandDef {
        name: "border-right",
        longhands: &[
            L::BorderRightWidth,
            L::BorderRightStyle,
            L::BorderRightColor,
        ],
        expand: |i| {
            expand_side(
                [
                    L::BorderRightWidth,
                    L::BorderRightStyle,
                    L::BorderRightColor,
                ],
                i,
            )
        },
    },
    ShorthandDef {
        name: "border-bottom",
        longhands: &[
            L::BorderBottomWidth,
            L::BorderBottomStyle,
            L::BorderBottomColor,
        ],
        expand: |i| {
            expand_side(
                [
                    L::BorderBottomWidth,
                    L::BorderBottomStyle,
                    L::BorderBottomColor,
                ],
                i,
            )
        },
    },
    ShorthandDef {
        name: "border-left",
        longhands: &[L::BorderLeftWidth, L::BorderLeftStyle, L::BorderLeftColor],
        expand: |i| {
            expand_side(
                [L::BorderLeftWidth, L::BorderLeftStyle, L::BorderLeftColor],
                i,
            )
        },
    },
    ShorthandDef {
        name: "border",
        longhands: BORDER,
        expand: border,
    },
    ShorthandDef {
        name: "border-radius",
        longhands: RADII,
        expand: border_radius,
    },
    ShorthandDef {
        name: "overflow",
        longhands: &[L::OverflowX, L::OverflowY],
        expand: overflow,
    },
    ShorthandDef {
        name: "background",
        longhands: &[
            L::BackgroundColor,
            L::BackgroundImage,
            L::BackgroundClip,
            L::BackgroundRepeat,
            L::BackgroundPosition,
            L::BackgroundSize,
            L::BackgroundOrigin,
            L::BackgroundAttachment,
        ],
        expand: background,
    },
    ShorthandDef {
        name: "font",
        longhands: &[
            L::FontStyle,
            L::FontWeight,
            L::FontSize,
            L::LineHeight,
            L::FontFamily,
        ],
        expand: font,
    },
    ShorthandDef {
        name: "text-decoration",
        longhands: &[
            L::TextDecorationLine,
            L::TextDecorationStyle,
            L::TextDecorationColor,
        ],
        expand: text_decoration,
    },
    ShorthandDef {
        name: "list-style",
        longhands: &[L::ListStylePosition, L::ListStyleImage, L::ListStyleType],
        expand: list_style,
    },
    ShorthandDef {
        name: "flex",
        longhands: &[L::FlexGrow, L::FlexShrink, L::FlexBasis],
        expand: flex,
    },
    ShorthandDef {
        name: "flex-flow",
        longhands: &[L::FlexDirection, L::FlexWrap],
        expand: flex_flow,
    },
    ShorthandDef {
        name: "gap",
        longhands: &[L::RowGap, L::ColumnGap],
        expand: gap,
    },
    ShorthandDef {
        name: "grid-gap",
        longhands: &[L::RowGap, L::ColumnGap],
        expand: gap,
    },
    ShorthandDef {
        name: "place-content",
        longhands: &[L::AlignContent, L::JustifyContent],
        expand: |i| place([L::AlignContent, L::JustifyContent], i),
    },
    ShorthandDef {
        name: "place-items",
        longhands: &[L::AlignItems, L::JustifyItems],
        expand: |i| place([L::AlignItems, L::JustifyItems], i),
    },
    ShorthandDef {
        name: "place-self",
        longhands: &[L::AlignSelf, L::JustifySelf],
        expand: |i| place([L::AlignSelf, L::JustifySelf], i),
    },
    // Propiedades lógicas (horizontal-tb, ltr).
    ShorthandDef {
        name: "margin-block",
        longhands: &[L::MarginTop, L::MarginBottom],
        expand: |i| pair([L::MarginTop, L::MarginBottom], i),
    },
    ShorthandDef {
        name: "margin-inline",
        longhands: &[L::MarginLeft, L::MarginRight],
        expand: |i| pair([L::MarginLeft, L::MarginRight], i),
    },
    ShorthandDef {
        name: "padding-block",
        longhands: &[L::PaddingTop, L::PaddingBottom],
        expand: |i| pair([L::PaddingTop, L::PaddingBottom], i),
    },
    ShorthandDef {
        name: "padding-inline",
        longhands: &[L::PaddingLeft, L::PaddingRight],
        expand: |i| pair([L::PaddingLeft, L::PaddingRight], i),
    },
    ShorthandDef {
        name: "inset-block",
        longhands: &[L::Top, L::Bottom],
        expand: |i| pair([L::Top, L::Bottom], i),
    },
    ShorthandDef {
        name: "inset-inline",
        longhands: &[L::Left, L::Right],
        expand: |i| pair([L::Left, L::Right], i),
    },
    ShorthandDef {
        name: "border-block-width",
        longhands: &[L::BorderTopWidth, L::BorderBottomWidth],
        expand: |i| pair([L::BorderTopWidth, L::BorderBottomWidth], i),
    },
    ShorthandDef {
        name: "border-block-style",
        longhands: &[L::BorderTopStyle, L::BorderBottomStyle],
        expand: |i| pair([L::BorderTopStyle, L::BorderBottomStyle], i),
    },
    ShorthandDef {
        name: "border-block-color",
        longhands: &[L::BorderTopColor, L::BorderBottomColor],
        expand: |i| pair([L::BorderTopColor, L::BorderBottomColor], i),
    },
    ShorthandDef {
        name: "border-inline-width",
        longhands: &[L::BorderLeftWidth, L::BorderRightWidth],
        expand: |i| pair([L::BorderLeftWidth, L::BorderRightWidth], i),
    },
    ShorthandDef {
        name: "border-inline-style",
        longhands: &[L::BorderLeftStyle, L::BorderRightStyle],
        expand: |i| pair([L::BorderLeftStyle, L::BorderRightStyle], i),
    },
    ShorthandDef {
        name: "border-inline-color",
        longhands: &[L::BorderLeftColor, L::BorderRightColor],
        expand: |i| pair([L::BorderLeftColor, L::BorderRightColor], i),
    },
    // Alias legado (CSS UI 4 §7.2).
    alias!("-webkit-appearance" => Appearance),
    alias!("margin-block-start" => MarginTop),
    alias!("margin-block-end" => MarginBottom),
    alias!("margin-inline-start" => MarginLeft),
    alias!("margin-inline-end" => MarginRight),
    alias!("padding-block-start" => PaddingTop),
    alias!("padding-block-end" => PaddingBottom),
    alias!("padding-inline-start" => PaddingLeft),
    alias!("padding-inline-end" => PaddingRight),
    alias!("inset-block-start" => Top),
    alias!("inset-block-end" => Bottom),
    alias!("inset-inline-start" => Left),
    alias!("inset-inline-end" => Right),
    alias!("block-size" => Height),
    alias!("inline-size" => Width),
    alias!("min-block-size" => MinHeight),
    alias!("min-inline-size" => MinWidth),
    alias!("max-block-size" => MaxHeight),
    alias!("max-inline-size" => MaxWidth),
    alias!("border-block-start-width" => BorderTopWidth),
    alias!("border-block-end-width" => BorderBottomWidth),
    alias!("border-inline-start-width" => BorderLeftWidth),
    alias!("border-inline-end-width" => BorderRightWidth),
    alias!("border-block-start-style" => BorderTopStyle),
    alias!("border-block-end-style" => BorderBottomStyle),
    alias!("border-inline-start-style" => BorderLeftStyle),
    alias!("border-inline-end-style" => BorderRightStyle),
    alias!("border-block-start-color" => BorderTopColor),
    alias!("border-block-end-color" => BorderBottomColor),
    alias!("border-inline-start-color" => BorderLeftColor),
    alias!("border-inline-end-color" => BorderRightColor),
    alias!("word-wrap" => OverflowWrap),
];

pub fn shorthand_by_name(name: &str) -> Option<&'static ShorthandDef> {
    static INDEX: OnceLock<HashMap<&'static str, usize>> = OnceLock::new();
    let index = INDEX.get_or_init(|| {
        SHORTHANDS
            .iter()
            .enumerate()
            .map(|(i, s)| (s.name, i))
            .collect()
    });
    index
        .get(name.to_ascii_lowercase().as_str())
        .map(|&i| &SHORTHANDS[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand(name: &str, value: &str) -> Option<Vec<String>> {
        let def = shorthand_by_name(name)?;
        let tokens = bf_css::syntax::parse_component_value_list(value);
        (def.expand)(trim(&tokens)).map(|values| {
            values
                .iter()
                .map(|v| format!("{}: {}", v.id().name(), (v.id().def().to_css)(v)))
                .collect()
        })
    }

    #[test]
    fn shorthands() {
        assert_eq!(
            expand("margin", "1px 2px 3px").unwrap(),
            [
                "margin-top: 1px",
                "margin-right: 2px",
                "margin-bottom: 3px",
                "margin-left: 2px"
            ]
        );
        assert_eq!(
            expand("border", "2px solid red").unwrap()[0..1],
            ["border-top-width: 2px".to_owned()]
        );
        assert_eq!(
            expand("border", "solid").unwrap()[0],
            "border-top-width: 3px"
        );
        assert!(expand("border", "solid solid").is_none());
        assert_eq!(
            expand("font", "italic bold 12px/30px Georgia, serif").unwrap(),
            [
                "font-style: italic",
                "font-weight: 700",
                "font-size: 12px",
                "line-height: 30px",
                "font-family: \"Georgia\", serif"
            ]
        );
        assert!(expand("font", "12px").is_none());
        assert!(expand("font", "bold").is_none());
        assert_eq!(
            expand("flex", "1").unwrap(),
            ["flex-grow: 1", "flex-shrink: 1", "flex-basis: 0%"]
        );
        assert_eq!(
            expand("list-style", "none").unwrap(),
            [
                "list-style-position: outside",
                "list-style-image: none",
                "list-style-type: none"
            ]
        );
        assert_eq!(
            expand("background", "url(a.png) no-repeat center / cover red").unwrap(),
            [
                "background-color: rgb(255, 0, 0)",
                "background-image: url(\"a.png\")",
                "background-clip: border-box",
                "background-repeat: no-repeat",
                "background-position: 50% 50%",
                "background-size: cover",
                "background-origin: padding-box",
                "background-attachment: scroll"
            ]
        );
        assert_eq!(
            expand(
                "background",
                "url(a.png) right 10px top / 5px auto repeat-y fixed, blue"
            )
            .unwrap()[3..6],
            [
                "background-repeat: repeat-y, repeat",
                "background-position: right 10px 0%, 0% 0%",
                "background-size: 5px auto, auto auto"
            ]
        );
        assert_eq!(
            expand("background", "padding-box content-box green").unwrap()[2],
            "background-clip: content-box"
        );
        assert_eq!(
            expand("text-decoration", "underline dotted blue").unwrap(),
            [
                "text-decoration-line: underline",
                "text-decoration-style: dotted",
                "text-decoration-color: rgb(0, 0, 255)"
            ]
        );
        assert_eq!(
            expand("border-radius", "1px 2px / 3px").unwrap()[1],
            "border-top-right-radius: 2px 3px"
        );
    }
}
