//! Controles de formulario (HTML §15.5) sin interactividad: cada control es un block container
//! atómico (`inline-block` según la hoja UA) cuyo contenido se genera aquí, como haría su
//! shadow tree: el valor o el placeholder de un campo de texto, la etiqueta de un botón, la
//! opción elegida de un `<select>`. El tamaño intrínseco sale de los atributos (`size`,
//! `cols`, `rows`) y de la fuente del control; lo nativo (casillas, flechas, barras) se pinta
//! con [`Widget`]. Las propiedades de autor (`width`, `height`, `border`, `background`,
//! `color`, `font`, ...) se aplican como en cualquier caja.
//!
//! TODO(forms): interactividad (foco, edición, selección de opciones, envío de formularios),
//! `::placeholder` y el scroll interno de `<textarea>` y de los list box.

use std::sync::Arc;

use bf_dom::{Document, Element, NodeId};
use bf_infra::Namespace;
use bf_style::ComputedStyle;
use bf_style::properties::types::{Appearance, ColorProperty, Display, WhiteSpace};
use bf_style::values::color::{Color, Rgba, parse_hex};

use crate::block::Layout;
use crate::box_tree::{BoxKind, LayoutBox, anonymous_style};
use crate::fragment::{MeterLevel, Widget};

/// Color del placeholder (el de `::placeholder` en los navegadores: `darkgray` oscurecido).
const PLACEHOLDER: Rgba = Rgba {
    r: 117.0,
    g: 117.0,
    b: 117.0,
    a: 1.0,
};

/// Fondo de las opciones elegidas de un list box sin foco.
const SELECTED_OPTION: Rgba = Rgba {
    r: 206.0,
    g: 206.0,
    b: 206.0,
    a: 1.0,
};

/// Cómo se dimensiona un control y qué se pinta además de su contenido.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    /// Campo de texto de una línea de `size` caracteres de ancho.
    TextField {
        size: f32,
    },
    TextArea {
        cols: f32,
        rows: f32,
    },
    /// Botón (o control que se muestra como uno): tamaño según su contenido.
    Button,
    Checkbox {
        checked: bool,
    },
    Radio {
        checked: bool,
    },
    /// `<select>` de una línea; las etiquetas dan el ancho (el de la más ancha).
    Select {
        options: Vec<String>,
    },
    /// `<select multiple>` o con `size > 1`.
    ListBox {
        rows: f32,
        options: Vec<String>,
    },
    Progress(Option<f32>),
    Meter {
        fraction: f32,
        level: MeterLevel,
    },
    Range(f32),
    Color(Rgba),
}

impl Control {
    /// Lo que se pinta sobre el content box.
    pub fn widget(&self) -> Option<Widget> {
        Some(match self {
            Control::Checkbox { checked } => Widget::Checkbox { checked: *checked },
            Control::Radio { checked } => Widget::Radio { checked: *checked },
            Control::Select { .. } => Widget::SelectArrow,
            Control::Progress(v) => Widget::Progress(*v),
            Control::Meter { fraction, level } => Widget::Meter {
                fraction: *fraction,
                level: *level,
            },
            Control::Range(v) => Widget::Range(*v),
            Control::Color(c) => Widget::ColorSwatch(*c),
            _ => return None,
        })
    }

    /// Controles de una sola línea, cuyo contenido se centra verticalmente
    /// (`align-content: center` en la hoja UA) y cuya baseline es la de su texto.
    pub fn single_line(&self) -> bool {
        matches!(
            self,
            Control::TextField { .. }
                | Control::Button
                | Control::Select { .. }
                | Control::Color(_)
        )
    }

    /// ¿El contenido se recorta al padding box?
    pub fn clips(&self) -> bool {
        matches!(
            self,
            Control::TextField { .. }
                | Control::TextArea { .. }
                | Control::Select { .. }
                | Control::ListBox { .. }
        )
    }
}

/// Número según las *rules for parsing floating-point number values* (aproximación: el prefijo
/// numérico después de quitar el white space inicial).
fn number(el: &Element, name: &str) -> Option<f32> {
    let v = el
        .attr(name)?
        .trim_start_matches([' ', '\t', '\n', '\x0C', '\r']);
    let end = v
        .char_indices()
        .find(|&(i, c)| {
            !(c.is_ascii_digit() || c == '.' || ((c == '-' || c == '+') && i == 0) || c == 'e')
        })
        .map_or(v.len(), |(i, _)| i);
    v[..end].parse::<f32>().ok().filter(|n| n.is_finite())
}

/// Entero positivo de un atributo (`size`, `cols`, `rows`), o `default`.
fn positive(el: &Element, name: &str, default: f32) -> f32 {
    el.attr(name)
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|&n| n > 0)
        .map_or(default, |n| n as f32)
}

/// Fracción de un `<progress>` (§4.10.13): `None` si es indeterminado.
fn progress_fraction(el: &Element) -> Option<f32> {
    let value = number(el, "value")?;
    let max = number(el, "max").filter(|m| *m > 0.0).unwrap_or(1.0);
    Some((value.max(0.0) / max).clamp(0.0, 1.0))
}

/// Fracción y región de un `<meter>` (§4.10.14).
fn meter(el: &Element) -> (f32, MeterLevel) {
    let min = number(el, "min").unwrap_or(0.0);
    let max = number(el, "max").unwrap_or(1.0).max(min);
    let value = number(el, "value").unwrap_or(0.0).clamp(min, max);
    let low = number(el, "low").unwrap_or(min).clamp(min, max);
    let high = number(el, "high").unwrap_or(max).clamp(low, max);
    let optimum = number(el, "optimum")
        .unwrap_or((min + max) / 2.0)
        .clamp(min, max);
    let level = if optimum < low {
        if value < low {
            MeterLevel::Optimum
        } else if value <= high {
            MeterLevel::Suboptimum
        } else {
            MeterLevel::EvenLessGood
        }
    } else if optimum > high {
        if value > high {
            MeterLevel::Optimum
        } else if value >= low {
            MeterLevel::Suboptimum
        } else {
            MeterLevel::EvenLessGood
        }
    } else if (low..=high).contains(&value) {
        MeterLevel::Optimum
    } else {
        MeterLevel::Suboptimum
    };
    let fraction = if max > min {
        (value - min) / (max - min)
    } else {
        0.0
    };
    (fraction, level)
}

/// Posición de un `<input type=range>` (§4.10.5.1.15).
fn range_fraction(el: &Element) -> f32 {
    let min = number(el, "min").unwrap_or(0.0);
    let max = number(el, "max").unwrap_or(100.0).max(min);
    let value = number(el, "value")
        .unwrap_or(min + (max - min) / 2.0)
        .clamp(min, max);
    if max > min {
        (value - min) / (max - min)
    } else {
        0.0
    }
}

/// Valor de un campo con los saltos de línea quitados (*value sanitization* de los campos de
/// texto).
fn single_line(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, '\n' | '\r')).collect()
}

/// Estilo del texto generado dentro de un control: hereda del control, con `white-space`
/// y color propios.
fn text_style(
    style: &Arc<ComputedStyle>,
    white_space: Option<WhiteSpace>,
    color: Option<Rgba>,
) -> Arc<ComputedStyle> {
    let mut s = (*anonymous_style(style, Display::Inline)).clone();
    if let Some(ws) = white_space {
        s.white_space = ws;
    }
    if let Some(c) = color {
        s.color = ColorProperty(Color::Rgba(c));
    }
    Arc::new(s)
}

fn text_box(text: String, style: Arc<ComputedStyle>, node: NodeId) -> LayoutBox {
    LayoutBox::new(BoxKind::Text(text), style, Some(node))
}

/// Etiqueta de un botón `<input>`: su valor o la etiqueta por defecto (dependiente del UA y
/// del idioma; §4.10.5.1.18 y §4.10.5.1.20).
fn button_label(el: &Element, ty: &str) -> String {
    if let Some(v) = el.attr("value") {
        return single_line(v);
    }
    match ty {
        "submit" => "Enviar".into(),
        "reset" => "Restablecer".into(),
        _ => String::new(),
    }
}

/// Texto que muestra un control de fecha u hora vacío (aproximación: el formato esperado).
fn date_format(ty: &str) -> &'static str {
    match ty {
        "date" => "dd/mm/aaaa",
        "month" => "-------- de ----",
        "week" => "Semana --, ----",
        "time" => "--:--",
        _ => "dd/mm/aaaa, --:--",
    }
}

/// Caja de un control de formulario, o `None` si `el` no es uno (o tiene `appearance: none`
/// y no tiene contenido propio que mostrar).
pub(crate) fn control_box(
    doc: &Document,
    node: NodeId,
    el: &Element,
    style: &Arc<ComputedStyle>,
) -> Option<LayoutBox> {
    if el.ns != Namespace::Html {
        return None;
    }
    let native = style.appearance != Appearance::None;
    let pre = Some(WhiteSpace::Pre);
    let mut children = Vec::new();
    let control = match el.name.as_lossy_str() {
        "input" => {
            let ty = el.attr("type").unwrap_or("").to_ascii_lowercase();
            match ty.as_str() {
                "hidden" | "image" => return None,
                "checkbox" | "radio" if !native => return None,
                "checkbox" => Control::Checkbox {
                    checked: el.attr("checked").is_some(),
                },
                "radio" => Control::Radio {
                    checked: el.attr("checked").is_some(),
                },
                "range" if !native => return None,
                "range" => Control::Range(range_fraction(el)),
                "color" => Control::Color(
                    el.attr("value")
                        .and_then(|v| v.strip_prefix('#'))
                        .filter(|v| v.len() == 6)
                        .and_then(parse_hex)
                        .unwrap_or(Rgba::BLACK),
                ),
                "submit" | "reset" | "button" | "file" => {
                    let label = if ty == "file" {
                        "Seleccionar archivo".to_owned()
                    } else {
                        button_label(el, &ty)
                    };
                    if !label.is_empty() {
                        children.push(text_box(label, text_style(style, pre, None), node));
                    }
                    Control::Button
                }
                "date" | "month" | "week" | "time" | "datetime-local" => {
                    let value = el
                        .attr("value")
                        .map(single_line)
                        .filter(|v| !v.is_empty())
                        .unwrap_or_else(|| date_format(&ty).to_owned());
                    children.push(text_box(value, text_style(style, pre, None), node));
                    Control::Button
                }
                _ => {
                    let value = el.attr("value").map(single_line).unwrap_or_default();
                    if !value.is_empty() {
                        let shown = if ty == "password" {
                            "•".repeat(value.chars().count())
                        } else {
                            value
                        };
                        children.push(text_box(shown, text_style(style, pre, None), node));
                    } else if let Some(p) = el.attr("placeholder").map(single_line)
                        && !p.is_empty()
                    {
                        let s = text_style(style, pre, Some(PLACEHOLDER));
                        children.push(text_box(p, s, node));
                    }
                    Control::TextField {
                        size: positive(el, "size", 20.0),
                    }
                }
            }
        }
        "textarea" => {
            let text = doc.text_content(node);
            if !text.is_empty() {
                children.push(text_box(text, text_style(style, None, None), node));
            } else if let Some(p) = el.attr("placeholder").filter(|p| !p.is_empty()) {
                let s = text_style(style, None, Some(PLACEHOLDER));
                children.push(text_box(p.to_owned(), s, node));
            }
            Control::TextArea {
                cols: positive(el, "cols", 20.0),
                rows: positive(el, "rows", 2.0),
            }
        }
        "select" => {
            let options = doc.list_of_options(node);
            let labels: Vec<String> = options.iter().map(|&o| doc.option_label(o)).collect();
            let rows = doc.display_size(node);
            if el.attr("multiple").is_some() || rows > 1 {
                for (&o, label) in options.iter().zip(&labels) {
                    let mut block_style = (*anonymous_style(style, Display::Block)).clone();
                    if doc.option_selectedness(o) {
                        block_style.background_color = Color::Rgba(SELECTED_OPTION);
                    }
                    let mut row =
                        LayoutBox::new(BoxKind::AnonymousBlock, Arc::new(block_style), None);
                    // Una opción vacía ocupa igual una fila.
                    let text = if label.is_empty() {
                        "\u{A0}".to_owned()
                    } else {
                        label.clone()
                    };
                    row.children
                        .push(text_box(text, text_style(style, pre, None), node));
                    children.push(row);
                }
                Control::ListBox {
                    rows: rows.max(1) as f32,
                    options: labels,
                }
            } else {
                if let Some(i) = options.iter().position(|&o| doc.option_selectedness(o))
                    && !labels[i].is_empty()
                {
                    children.push(text_box(
                        labels[i].clone(),
                        text_style(style, pre, None),
                        node,
                    ));
                }
                Control::Select { options: labels }
            }
        }
        "progress" if native => Control::Progress(progress_fraction(el)),
        "meter" if native => {
            let (fraction, level) = meter(el);
            Control::Meter { fraction, level }
        }
        _ => return None,
    };
    let kind = if style.display.is_inline_level() {
        BoxKind::InlineBlock
    } else {
        BoxKind::Block
    };
    let mut b = LayoutBox::new(kind, style.clone(), Some(node));
    b.children = children;
    b.control = Some(control);
    Some(b)
}

impl Layout<'_> {
    /// Ancho del texto `s` con la fuente de `style`.
    fn text_width(&self, s: &str, style: &ComputedStyle) -> f32 {
        self.shape(s, style).iter().map(|g| g.advance).sum()
    }

    /// Tamaño intrínseco del content box de un control (`None` en un eje = según su
    /// contenido).
    pub(crate) fn control_size(&self, b: &LayoutBox) -> (Option<f32>, Option<f32>) {
        let Some(control) = &b.control else {
            return (None, None);
        };
        let style = &b.style;
        let fs = style.font_size_px();
        let lh = self.line_height(style);
        // `ch`: el avance del "0" de la fuente del control.
        let ch = self.text_width("0", style);
        let widest = |options: &[String]| {
            options
                .iter()
                .map(|o| self.text_width(o, style))
                .fold(0.0f32, f32::max)
        };
        match control {
            Control::TextField { size } => (Some(size * ch), Some(lh)),
            Control::TextArea { cols, rows } => (Some(cols * ch), Some(rows * lh)),
            Control::Button => (None, None),
            Control::Checkbox { .. } | Control::Radio { .. } => (Some(13.0), Some(13.0)),
            Control::Select { options } => (Some(widest(options) + select_arrow(fs)), Some(lh)),
            Control::ListBox { rows, options } => (Some(widest(options)), Some(rows * lh)),
            Control::Progress(_) => (Some(10.0 * fs), Some(fs)),
            Control::Meter { .. } => (Some(5.0 * fs), Some(fs)),
            Control::Range(_) => (Some(129.0), Some(16.0)),
            Control::Color(_) => (Some(3.0 * fs), Some(lh)),
        }
    }
}

/// Espacio que ocupa la flecha de un `<select>` al final del content box.
pub fn select_arrow(font_size: f32) -> f32 {
    (1.5 * font_size).round()
}
