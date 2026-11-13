//! Hojas de estilo del documento: `<style>`, `<link rel="stylesheet">` y `@import`, con sus
//! URLs resueltas contra la hoja que las contiene.
//!
//! Las hojas importadas se insertan en la regla `@import`, envueltas en `@media`, `@supports` y
//! `@layer` según sus condiciones, así la recolección de declaraciones de `bf-css` las trata
//! como cualquier otra regla anidada.

use bf_css::media::{MediaContext, parse_media_query_list};
use bf_css::stylesheet::{
    ImportRule, LayerName, LayerRule, MediaRule, Origin, Rule, Stylesheet, SupportsRule,
};
use bf_css::syntax::Declaration;
use bf_dom::{Document, NodeData, NodeId};
use bf_infra::encoding::{self, Encoding};
use bf_infra::{ResourceLoader, Url};

use crate::cascade::is_supported;

/// Dónde buscar los recursos del documento.
#[derive(Clone, Copy)]
pub struct Resources<'a> {
    pub loader: &'a dyn ResourceLoader,
    /// URL base del documento.
    pub base: &'a Url,
    /// Encoding del documento (el de las hojas sin BOM ni `@charset`, CSS Syntax §3.2).
    pub encoding: Encoding,
}

/// Límite de anidamiento de `@import` (protege de cadenas patológicas; los ciclos se cortan
/// aparte).
const MAX_IMPORT_DEPTH: usize = 16;

fn is_css_type(t: Option<&str>) -> bool {
    t.is_none_or(|t| t.is_empty() || t.eq_ignore_ascii_case("text/css"))
}

fn text_content(doc: &Document, node: NodeId) -> String {
    doc.children(node)
        .filter_map(|c| match &doc.get(c).data {
            NodeData::Text(t) | NodeData::CdataSection(t) => Some(t.as_lossy_str()),
            _ => None,
        })
        .collect()
}

/// E1: las hojas de un shadow tree (CSS Scoping §3.1): sus `<style>` y, después, sus
/// `adoptedStyleSheets`. Los `<link rel=stylesheet>` dentro de un shadow tree todavía no se
/// cargan.
pub fn shadow_tree_stylesheets(
    doc: &Document,
    root: NodeId,
    media: &MediaContext,
) -> Vec<Stylesheet> {
    let mut sheets = Vec::new();
    for node in doc.descendants(root) {
        let Some(el) = doc.element(node) else {
            continue;
        };
        if !el.is_html("style") || !is_css_type(el.attr("type")) {
            continue;
        }
        if let Some(m) = el.attr("media")
            && !parse_media_query_list(m).evaluate(media)
        {
            continue;
        }
        sheets.push(Stylesheet::parse(&text_content(doc, node), Origin::Author));
    }
    for text in doc.adopted_style_texts(root) {
        sheets.push(Stylesheet::parse(text, Origin::Author));
    }
    sheets
}

/// E5: la marca del texto con las caras que los scripts añadieron a `document.fonts` (CSS Font
/// Loading 3 §4), que `bf-web` guarda entre los textos adoptados de la raíz del documento
/// (`Document::set_adopted_style_texts`). [`document_stylesheets`] solo lee de la raíz los
/// textos con esta marca: las `adoptedStyleSheets` del documento llegan por el CSSOM.
pub const SCRIPT_FONT_FACES: &str = "/*bf-font-loading*/";

/// Hojas de autor del documento en orden de árbol: `<style>` y `<link rel="stylesheet">`
/// (este último solo con `resources`), con su atributo `media` evaluado.
pub fn document_stylesheets(
    doc: &Document,
    media: &MediaContext,
    resources: Option<Resources<'_>>,
) -> Vec<Stylesheet> {
    let quirks = doc.mode == bf_dom::QuirksMode::Quirks;
    let mut sheets = Vec::new();
    for node in doc.inclusive_descendants(doc.root()) {
        let Some(el) = doc.element(node) else {
            continue;
        };
        if let Some(m) = el.attr("media")
            && (el.is_html("style") || el.is_html("link"))
            && !parse_media_query_list(m).evaluate(media)
        {
            continue;
        }
        if el.is_html("style") {
            if !is_css_type(el.attr("type")) {
                continue;
            }
            let mut sheet = Stylesheet::parse(&text_content(doc, node), Origin::Author);
            if let Some(res) = resources {
                process(
                    &mut sheet,
                    res.base,
                    res,
                    quirks,
                    &mut vec![res.base.serialize(true)],
                );
            }
            sheets.push(sheet);
        } else if el.is_html("link") {
            let rel = el.attr("rel").unwrap_or("");
            let has = |token: &str| {
                rel.split_ascii_whitespace()
                    .any(|t| t.eq_ignore_ascii_case(token))
            };
            // Las hojas alternativas no se aplican hasta que el usuario las elige.
            if !has("stylesheet") || has("alternate") || el.attr("disabled").is_some() {
                continue;
            }
            if !is_css_type(el.attr("type")) {
                continue;
            }
            let (Some(res), Some(href)) = (resources, el.attr("href")) else {
                continue;
            };
            if let Some(sheet) = load_sheet(href, res.base, res, quirks, &mut Vec::new()) {
                sheets.push(sheet);
            }
        }
    }
    // E5: las caras de `document.fonts` añadidas desde scripts, después de las hojas (con URLs
    // ya absolutas).
    for text in doc
        .adopted_style_texts(doc.root())
        .iter()
        .filter(|t| t.starts_with(SCRIPT_FONT_FACES))
    {
        sheets.push(Stylesheet::parse(text, Origin::Author));
    }
    sheets
}

/// Carga una hoja externa: parseada (E4: de [`crate::sheet_cache`] si ya lo estaba) y con sus
/// imports resueltos.
fn load_sheet(
    href: &str,
    base: &Url,
    res: Resources<'_>,
    quirks: bool,
    chain: &mut Vec<String>,
) -> Option<Stylesheet> {
    let url = Url::parse(href, Some(base)).ok()?;
    let resource = res.loader.load(&url)?;
    let key = resource.url.serialize(true);
    if chain.contains(&key) || chain.len() > MAX_IMPORT_DEPTH {
        return None;
    }
    let ctx = crate::sheet_cache::SheetContext {
        quirks,
        encoding: res.encoding,
    };
    // Parseada y con las `url()` ya resueltas contra `resource.url`.
    let mut sheet = (*crate::sheet_cache::warm(&resource.url, &resource.bytes, &ctx)).clone();
    chain.push(key);
    resolve_imports(&mut sheet, &resource.url, res, quirks, chain);
    chain.pop();
    Some(sheet)
}

/// Decodificación de una hoja (CSS Syntax §3.2): BOM; si no, `@charset "…";` al principio
/// (UTF-16 se toma como UTF-8); si no, el encoding del entorno.
pub fn decode_css(bytes: &[u8], environment: Encoding) -> String {
    if let Some((e, n)) = encoding::sniff_bom(bytes) {
        return e.decode_without_bom(&bytes[n..]);
    }
    let fallback = charset_rule(bytes).unwrap_or(environment);
    fallback.decode_without_bom(bytes)
}

/// Encoding de un `@charset "…";` en los primeros 1024 bytes.
fn charset_rule(bytes: &[u8]) -> Option<Encoding> {
    let head = &bytes[..bytes.len().min(1024)];
    let rest = head.strip_prefix(b"@charset \"")?;
    let end = rest.iter().position(|&b| b == b'"')?;
    if rest.get(end + 1) != Some(&b';') {
        return None;
    }
    let label = std::str::from_utf8(&rest[..end]).ok()?;
    match Encoding::for_label(label)? {
        Encoding::Utf16Le | Encoding::Utf16Be => Some(Encoding::Utf8),
        e => Some(e),
    }
}

/// Resuelve las URLs de `sheet` contra `base` e inserta las hojas importadas.
fn process(
    sheet: &mut Stylesheet,
    base: &Url,
    res: Resources<'_>,
    quirks: bool,
    chain: &mut Vec<String>,
) {
    sheet.rewrite_urls(&mut |u| Url::parse(u, Some(base)).ok().map(|u| u.href()));
    resolve_imports(sheet, base, res, quirks, chain);
}

/// Inserta las hojas que `sheet` importa (con sus URLs ya resueltas contra `base`).
fn resolve_imports(
    sheet: &mut Stylesheet,
    base: &Url,
    res: Resources<'_>,
    quirks: bool,
    chain: &mut Vec<String>,
) {
    let rules = std::mem::take(&mut sheet.rules);
    let mut out = Vec::with_capacity(rules.len());
    for rule in rules {
        let Rule::Import(import) = rule else {
            out.push(rule);
            continue;
        };
        let imported = load_sheet(&import.url, base, res, quirks, chain);
        let wrapped = imported.map(|s| wrap_import(&import, s, chain.len()));
        out.push(Rule::Import(import));
        out.extend(wrapped);
    }
    sheet.rules = out;
    sheet.rebuild_layer_order();
}

/// Contenido de una hoja importada como reglas anidadas con las condiciones del `@import`.
fn wrap_import(import: &ImportRule, sheet: Stylesheet, depth: usize) -> Rule {
    let mut rules = sheet.rules;
    // Las capas de la hoja importada quedan dentro de la capa del import; las anónimas
    // reciben un sufijo para no chocar con las de la hoja que importa.
    let prefix = import.layer.clone().unwrap_or(LayerName(Vec::new()));
    let tag = format!("@{depth}:{}", import.url);
    rename_layers(&mut rules, &|name: &LayerName| {
        LayerName(
            prefix
                .0
                .iter()
                .cloned()
                .chain(name.0.iter().map(|s| {
                    if s.starts_with('\0') {
                        format!("{s}{tag}")
                    } else {
                        s.clone()
                    }
                }))
                .collect(),
        )
    });
    if let Some(name) = &import.layer {
        rules = vec![Rule::Layer(LayerRule::Block {
            name: name.clone(),
            rules,
        })];
    }
    if let Some(condition) = &import.supports {
        rules = vec![Rule::Supports(SupportsRule {
            condition: condition.clone(),
            rules,
        })];
    }
    Rule::Media(MediaRule {
        queries: import.media.clone(),
        rules,
    })
}

fn rename_layers(rules: &mut [Rule], f: &dyn Fn(&LayerName) -> LayerName) {
    for rule in rules {
        match rule {
            Rule::Layer(LayerRule::Statement(names)) => {
                for n in names {
                    *n = f(n);
                }
            }
            Rule::Layer(LayerRule::Block { name, rules }) => {
                *name = f(name);
                rename_layers(rules, f);
            }
            Rule::Import(i) => {
                if let Some(l) = &mut i.layer {
                    *l = f(l);
                }
            }
            Rule::Media(m) => rename_layers(&mut m.rules, f),
            Rule::Supports(s) => rename_layers(&mut s.rules, f),
            Rule::Style(s) => rename_layers(&mut s.nested, f),
            _ => {}
        }
    }
}

/// Las reglas `@font-face` que aplican (dentro de `@media`/`@supports` que se cumplen y de
/// cualquier capa), en orden.
pub fn font_face_rules<'a>(
    sheets: &[&'a Stylesheet],
    media: &MediaContext,
) -> Vec<&'a [Declaration]> {
    fn walk<'a>(rules: &'a [Rule], media: &MediaContext, out: &mut Vec<&'a [Declaration]>) {
        for rule in rules {
            match rule {
                Rule::FontFace(decls) => out.push(decls),
                Rule::Media(m) if m.queries.evaluate(media) => walk(&m.rules, media, out),
                Rule::Supports(s) if s.condition.evaluate(&is_supported) => {
                    walk(&s.rules, media, out);
                }
                Rule::Layer(LayerRule::Block { rules, .. }) => walk(rules, media, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    for sheet in sheets {
        walk(&sheet.rules, media, &mut out);
    }
    out
}
