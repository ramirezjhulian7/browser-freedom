//! Referencias a recursos de otros documentos (SVG 2 §5.6.2 y §16.1.5, *URL references*):
//! `use href="otro.svg#id"`, `fill="url(otro.svg#grad)"`, marcadores, `clip-path`, `mask`, la
//! cadena `href` de gradientes y patrones. `bf-svg` no carga nada: quien pinta pide la lista
//! ([`Svg::external_documents`]), carga y parsea cada documento y lo une al árbol con
//! [`Svg::attach_external`].
//!
//! Al unirlo, los nodos del documento externo se copian detrás de los propios y todas sus
//! referencias se reescriben con una clave sintética (`bf-ext:<n>`), así que se resuelven en
//! su propio documento: sus `#id` locales no chocan con los del documento que lo usa.

use std::collections::HashMap;

use bf_css::syntax::{ComponentValue, TokenKind};

use crate::Svg;

/// Prefijo de las claves sintéticas de los documentos unidos.
const EXT_SCHEME: &str = "bf-ext:";

/// Propiedades cuyo valor puede ser una referencia `url(…)` a otro elemento.
const URL_PROPERTIES: &[&str] = &[
    "fill",
    "stroke",
    "marker-start",
    "marker-mid",
    "marker-end",
    "clip-path",
    "mask",
    "filter",
];

/// Elementos cuyo `href` apunta a otro elemento SVG (no a una imagen ni a un enlace).
const HREF_ELEMENTS: &[&str] = &[
    "use",
    "linearGradient",
    "radialGradient",
    "pattern",
    "textPath",
    "filter",
    "feImage",
];

/// Un documento externo ya unido: sus `id` y su elemento raíz (índices del documento que lo
/// usa).
#[derive(Debug, Clone, Default)]
pub(crate) struct ExtDoc {
    pub ids: HashMap<String, usize>,
    pub root: usize,
}

/// Divide una referencia en (documento, fragmento): `otro.svg#a` → (`otro.svg`, `a`), `#a` →
/// (``, `a`), `otro.svg` → (`otro.svg`, ``).
pub(crate) fn split(url: &str) -> (&str, &str) {
    let url = url.trim();
    match url.rsplit_once('#') {
        Some((doc, id)) => (doc.trim(), id),
        None => (url, ""),
    }
}

fn url_in(c: &ComponentValue, out: &mut Vec<String>) {
    match c {
        ComponentValue::Token(t) => {
            if let TokenKind::Url(u) = &t.kind {
                out.push(u.clone());
            }
        }
        ComponentValue::Function(f) if f.name.eq_ignore_ascii_case("url") => {
            for v in &f.value {
                if let ComponentValue::Token(t) = v
                    && let TokenKind::String(s) = &t.kind
                {
                    out.push(s.clone());
                }
            }
        }
        _ => {}
    }
}

/// Reescribe la parte de documento de una referencia con `rename`.
fn rewrite(url: &str, rename: &dyn Fn(&str) -> String) -> String {
    let (doc, id) = split(url);
    format!("{}#{id}", rename(doc))
}

fn rewrite_values(values: &mut [ComponentValue], rename: &dyn Fn(&str) -> String) {
    for c in values {
        match c {
            ComponentValue::Token(t) => {
                if let TokenKind::Url(u) = &mut t.kind {
                    *u = rewrite(u, rename);
                }
            }
            ComponentValue::Function(f) if f.name.eq_ignore_ascii_case("url") => {
                for v in &mut f.value {
                    if let ComponentValue::Token(t) = v
                        && let TokenKind::String(s) = &mut t.kind
                    {
                        *s = rewrite(s, rename);
                    }
                }
            }
            _ => {}
        }
    }
}

impl Svg {
    /// Los documentos externos a los que se refiere este SVG (la parte de la URL antes del
    /// `#`, tal como está escrita y sin repetir): `href` de `use`, gradientes, patrones y
    /// `textPath`, y `url(…)` de `fill`, `stroke`, marcadores, `clip-path`, `mask` y `filter`.
    /// No incluye los ya unidos ni las imágenes (`image`, ver [`Svg::image_hrefs`]).
    pub fn external_documents(&self) -> Vec<String> {
        let mut refs: Vec<String> = Vec::new();
        for n in &self.nodes {
            if n.is_text() {
                continue;
            }
            if HREF_ELEMENTS.contains(&n.name.as_str())
                && let Some(h) = n.attr("href")
            {
                refs.push(h.to_owned());
            }
            for p in URL_PROPERTIES {
                if let Some(v) = n.props.get(*p) {
                    for c in v {
                        url_in(c, &mut refs);
                    }
                }
            }
        }
        let mut out: Vec<String> = Vec::new();
        for r in refs {
            let (doc, _) = split(&r);
            if doc.is_empty() || doc.starts_with(EXT_SCHEME) || self.externals.contains_key(doc) {
                continue;
            }
            if !out.iter().any(|o| o == doc) {
                out.push(doc.to_owned());
            }
        }
        out
    }

    /// Une el documento `ext` (con sus propios externos ya unidos) como el destino de las
    /// referencias a `doc` (como lo devolvió [`Svg::external_documents`]). La cascada de `ext`
    /// es la suya: las hojas de este documento no lo alcanzan.
    pub fn attach_external(&mut self, doc: &str, ext: &Svg) {
        if ext.nodes.is_empty() {
            return;
        }
        self.ext_count += 1;
        let key = format!("{EXT_SCHEME}{}", self.ext_count);
        let offset = self.nodes.len();
        let rename = |d: &str| -> String {
            if d.is_empty() {
                key.clone()
            } else {
                format!("{key}/{d}")
            }
        };
        for n in &ext.nodes {
            let mut n = n.clone();
            for c in &mut n.children {
                *c += offset;
            }
            if !n.is_text() {
                if HREF_ELEMENTS.contains(&n.name.as_str())
                    && let Some(h) = n.attrs.get_mut("href")
                {
                    *h = rewrite(h, &rename);
                }
                for p in URL_PROPERTIES {
                    if let Some(v) = n.props.get_mut(*p) {
                        rewrite_values(v, &rename);
                    }
                }
                // Un `foreignObject` de otro documento no tiene nodo en el DOM de este.
                n.dom = None;
            }
            self.nodes.push(n);
        }
        for (k, v) in &ext.shadows {
            self.shadows.insert(k + offset, v + offset);
        }
        let shift = |e: &ExtDoc| ExtDoc {
            ids: e.ids.iter().map(|(k, v)| (k.clone(), v + offset)).collect(),
            root: e.root + offset,
        };
        let own = ExtDoc {
            ids: ext
                .ids
                .iter()
                .map(|(k, v)| (k.clone(), v + offset))
                .collect(),
            root: offset,
        };
        for (k, e) in &ext.externals {
            self.externals.insert(rename(k), shift(e));
        }
        self.externals.insert(doc.trim().to_owned(), own.clone());
        self.externals.insert(key, own);
    }

    /// El elemento al que apunta `url`: `#id` en este documento, `otro.svg#id` (o su raíz sin
    /// fragmento) en un documento unido.
    pub(crate) fn lookup(&self, url: &str) -> Option<usize> {
        let (doc, id) = split(url);
        if doc.is_empty() {
            return self.ids.get(id).copied();
        }
        let ext = self.externals.get(doc)?;
        if id.is_empty() {
            Some(ext.root)
        } else {
            ext.ids.get(id).copied()
        }
    }
}
