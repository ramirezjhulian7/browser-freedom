//! Presentational hints de WHATWG HTML §15 que dependen del valor de atributos y no se pueden
//! expresar como CSS (los que sí se pueden están en `ua/hints.css`). Se aplican como
//! declaraciones de origen author con especificidad cero (§15.2).

use bf_css::syntax::{Declaration, parse_declaration};
use bf_dom::{Document, Element, NodeId};

use crate::values::color::parse_legacy_color;

/// ASCII whitespace (Infra).
const HTML_WHITESPACE: [char; 5] = ['\t', '\n', '\x0C', '\r', ' '];

/// Rules for parsing non-negative integers (HTML §2.3.4.1).
pub fn parse_non_negative_integer(s: &str) -> Option<u32> {
    let s = s.trim_start_matches(HTML_WHITESPACE);
    let s = s.strip_prefix('+').unwrap_or(s);
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    Some(digits.parse().unwrap_or(u32::MAX))
}

/// Rules for parsing dimension values (HTML §2.3.4.4): `(valor, es_porcentaje)`.
pub fn parse_dimension(s: &str) -> Option<(f32, bool)> {
    let s = s.trim_start_matches(HTML_WHITESPACE);
    let int: String = s.chars().take_while(char::is_ascii_digit).collect();
    if int.is_empty() {
        return None;
    }
    let mut rest = &s[int.len()..];
    let mut value: f32 = int.parse().unwrap_or(f32::MAX);
    if let Some(frac) = rest.strip_prefix('.') {
        let digits: String = frac.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            value += format!("0.{digits}").parse::<f32>().unwrap_or(0.0);
        }
        rest = &frac[digits.len()..];
    }
    Some((value, rest.starts_with('%')))
}

fn dimension_css(attr: &str, ignore_zero: bool) -> Option<String> {
    let (v, percent) = parse_dimension(attr)?;
    if ignore_zero && v == 0.0 {
        return None;
    }
    Some(if percent {
        format!("{v}%")
    } else {
        format!("{v}px")
    })
}

fn color_css(attr: &str) -> Option<String> {
    let c = parse_legacy_color(attr)?;
    Some(format!("rgb({} {} {})", c.r, c.g, c.b))
}

struct Hints(Vec<Declaration>);

impl Hints {
    fn push(&mut self, name: &str, value: &str) {
        if let Ok(d) = parse_declaration(&format!("{name}: {value}")) {
            self.0.push(d);
        }
    }

    fn push_all(&mut self, names: &[&str], value: &str) {
        for n in names {
            self.push(n, value);
        }
    }
}

const BORDER_WIDTHS: [&str; 4] = [
    "border-top-width",
    "border-right-width",
    "border-bottom-width",
    "border-left-width",
];

/// El `table` más cercano (para `cellpadding`).
fn ancestor_table(doc: &Document, node: NodeId) -> Option<&Element> {
    let mut current = doc.parent(node);
    while let Some(n) = current {
        if let Some(el) = doc.element(n)
            && el.is_html("table")
        {
            return Some(el);
        }
        current = doc.parent(n);
    }
    None
}

/// Declaraciones de presentational hints para el elemento `node`.
pub fn presentational_hints(doc: &Document, node: NodeId) -> Vec<Declaration> {
    let mut h = Hints(Vec::new());
    let Some(el) = doc.element(node) else {
        return h.0;
    };
    if el.ns != bf_infra::Namespace::Html {
        return h.0;
    }
    let attr = |name: &str| el.attr(name);
    let name = el.name.as_str();

    // §15.3.2: márgenes del body (8px por defecto) y colores.
    if name == "body" {
        let px = |names: &[&str]| {
            names
                .iter()
                .find_map(|n| attr(n))
                .and_then(parse_non_negative_integer)
                .map_or_else(|| "8px".to_owned(), |v| format!("{v}px"))
        };
        let vertical = px(&["marginheight", "topmargin"]);
        let horizontal = px(&["marginwidth", "leftmargin"]);
        h.push("margin-top", &vertical);
        h.push("margin-bottom", &vertical);
        h.push("margin-left", &horizontal);
        h.push("margin-right", &horizontal);
        if let Some(c) = attr("text").and_then(color_css) {
            h.push("color", &c);
        }
    }

    // bgcolor y background (§15.3.2, §15.3.8).
    if matches!(
        name,
        "body" | "table" | "thead" | "tbody" | "tfoot" | "tr" | "td" | "th"
    ) {
        if let Some(c) = attr("bgcolor").and_then(color_css) {
            h.push("background-color", &c);
        }
        if let Some(url) = attr("background").filter(|v| !v.is_empty()) {
            h.push("background-image", &format!("url({url:?})"));
        }
    }

    // Alineación de texto (§15.3.3, §15.3.8).
    if matches!(
        name,
        "div" | "thead" | "tbody" | "tfoot" | "tr" | "td" | "th"
    ) && let Some(align) = attr("align")
    {
        let value = match align.to_ascii_lowercase().as_str() {
            "center" | "middle" => Some("center"),
            "left" => Some("left"),
            "right" => Some("right"),
            "justify" => Some("justify"),
            _ => None,
        };
        if let Some(v) = value {
            h.push("text-align", v);
        }
    }
    if name == "center" {
        h.push("text-align", "center");
    }

    // Tablas (§15.3.8).
    match name {
        "table" => {
            if let Some(w) = attr("width").and_then(|v| dimension_css(v, true)) {
                h.push("width", &w);
            }
            if let Some(v) = attr("height").and_then(|v| dimension_css(v, false)) {
                h.push("height", &v);
            }
            if let Some(b) = attr("border") {
                let px = parse_non_negative_integer(b).unwrap_or(1);
                h.push_all(&BORDER_WIDTHS, &format!("{px}px"));
            }
            // E4: `cellspacing` se mapea a la longitud en px de `border-spacing`.
            if let Some(v) = attr("cellspacing").and_then(parse_non_negative_integer) {
                h.push("border-spacing", &format!("{v}px"));
            }
            if let Some(c) = attr("bordercolor").and_then(color_css) {
                h.push_all(
                    &[
                        "border-top-color",
                        "border-right-color",
                        "border-bottom-color",
                        "border-left-color",
                    ],
                    &c,
                );
            }
        }
        "td" | "th" => {
            if let Some(w) = attr("width").and_then(|v| dimension_css(v, true)) {
                h.push("width", &w);
            }
            if let Some(v) = attr("height").and_then(|v| dimension_css(v, true)) {
                h.push("height", &v);
            }
            if let Some(p) = ancestor_table(doc, node)
                .and_then(|t| t.attr("cellpadding"))
                .and_then(parse_non_negative_integer)
            {
                h.push_all(
                    &[
                        "padding-top",
                        "padding-right",
                        "padding-bottom",
                        "padding-left",
                    ],
                    &format!("{p}px"),
                );
            }
        }
        "thead" | "tbody" | "tfoot" | "tr" => {
            if let Some(v) = attr("height").and_then(|v| dimension_css(v, false)) {
                h.push("height", &v);
            }
        }
        "col" => {
            if let Some(w) = attr("width").and_then(|v| dimension_css(v, false)) {
                h.push("width", &w);
            }
        }
        _ => {}
    }

    // Contenido embebido e imágenes (§15.4.3).
    let image_button =
        name == "input" && attr("type").is_some_and(|t| t.eq_ignore_ascii_case("image"));
    if matches!(name, "img" | "embed" | "iframe" | "object" | "video") || image_button {
        if let Some(w) = attr("width").and_then(|v| dimension_css(v, false)) {
            h.push("width", &w);
        }
        if let Some(v) = attr("height").and_then(|v| dimension_css(v, false)) {
            h.push("height", &v);
        }
    }
    if matches!(name, "img" | "embed" | "object") || image_button {
        if let Some(v) = attr("hspace").and_then(|v| dimension_css(v, false)) {
            h.push_all(&["margin-left", "margin-right"], &v);
        }
        if let Some(v) = attr("vspace").and_then(|v| dimension_css(v, false)) {
            h.push_all(&["margin-top", "margin-bottom"], &v);
        }
    }
    if (matches!(name, "img" | "object") || image_button)
        && let Some(b) = attr("border").and_then(parse_non_negative_integer)
        && b > 0
    {
        h.push_all(&BORDER_WIDTHS, &format!("{b}px"));
        h.push_all(
            &[
                "border-top-style",
                "border-right-style",
                "border-bottom-style",
                "border-left-style",
            ],
            "solid",
        );
    }
    if name == "iframe"
        && let Some(fb) = attr("frameborder")
        && fb.trim().parse::<i64>().map_or(true, |v| v == 0)
    {
        h.push_all(&BORDER_WIDTHS, "0");
    }

    // hr (§15.3.11).
    if name == "hr" {
        let has_color_or_noshade = attr("color").is_some() || attr("noshade").is_some();
        if let Some(size) = attr("size").and_then(parse_non_negative_integer) {
            if has_color_or_noshade {
                h.push_all(&BORDER_WIDTHS, &format!("{}px", size as f32 / 2.0));
            } else if size == 1 {
                h.push("border-bottom-width", "0");
            } else if size > 1 {
                h.push("height", &format!("{}px", size - 2));
            }
        }
        if let Some(w) = attr("width").and_then(|v| dimension_css(v, false)) {
            h.push("width", &w);
        }
        if let Some(c) = attr("color").and_then(color_css) {
            h.push("color", &c);
        }
    }

    // font (§15.3.4).
    if name == "font" {
        if let Some(c) = attr("color").and_then(color_css) {
            h.push("color", &c);
        }
        if let Some(face) = attr("face") {
            h.push("font-family", face);
        }
        if let Some(size) = attr("size").and_then(legacy_font_size) {
            h.push("font-size", size);
        }
    }
    h.0
}

/// Rules for parsing a legacy font size (HTML §15.3.4).
fn legacy_font_size(input: &str) -> Option<&'static str> {
    let s = input.trim_start_matches(HTML_WHITESPACE);
    let (mode, rest) = match s.chars().next()? {
        '+' => (1, &s[1..]),
        '-' => (-1, &s[1..]),
        _ => (0, s),
    };
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let mut value: i64 = digits.parse().unwrap_or(i64::MAX / 2);
    match mode {
        1 => value += 3,
        -1 => value = 3 - value,
        _ => {}
    }
    Some(match value.clamp(1, 7) {
        1 => "x-small",
        2 => "small",
        3 => "medium",
        4 => "large",
        5 => "x-large",
        6 => "xx-large",
        _ => "xxx-large",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microsyntaxes() {
        assert_eq!(parse_dimension("100"), Some((100.0, false)));
        assert_eq!(parse_dimension(" 50.5%"), Some((50.5, true)));
        assert_eq!(parse_dimension("abc"), None);
        assert_eq!(parse_non_negative_integer("+12px"), Some(12));
        assert_eq!(legacy_font_size("+2"), Some("x-large"));
        assert_eq!(legacy_font_size("-1"), Some("small"));
        assert_eq!(legacy_font_size("9"), Some("xxx-large"));
    }
}
