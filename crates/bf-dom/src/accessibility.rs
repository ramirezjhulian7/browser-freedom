//! E1: el rol y el nombre accesibles de un elemento, la base del árbol de accesibilidad.
//!
//! - **Rol calculado:** WAI-ARIA 1.2 §5.4 (el primer token de `role` que es un rol concreto
//!   conocido, con su lista de respaldo; los roles abstractos se ignoran; `presentation`/`none`
//!   se ignoran si el elemento es enfocable o tiene atributos ARIA globales, §9.3 "presentational
//!   roles conflict resolution") y, sin rol explícito, el rol implícito de HTML-AAM §4.
//! - **Nombre y descripción:** Accessible Name and Description Computation 1.2 §4.3, con los
//!   nombres nativos de HTML-AAM §4.1 (`label`, `alt`, `caption`, `legend`, `figcaption`,
//!   `title`, `placeholder`, `value` de los botones).
//!
//! Los nombres de rol son los de WebDriver "Get Computed Role" (los de ARIA; `img` sale como
//! `image` y `presentation` como `none`, como en los navegadores). Lo que depende del estilo
//! (`display: none`, `visibility`, el `content` de `::before`/`::after`, si una caja es en
//! línea) y el valor actual de los controles lo da el embebedor con [`A11yEnv`]; [`NoStyle`]
//! sirve sin estilo (solo los atributos `hidden` y `aria-hidden`).

use std::collections::HashSet;

use crate::{Document, NodeData, NodeId};

/// Lo que el cálculo necesita de fuera del DOM.
pub trait A11yEnv {
    /// ¿El elemento tiene `display: none` (él, no sus ancestros)?
    fn display_none(&self, _n: NodeId) -> bool {
        false
    }
    /// ¿El nodo tiene `visibility: hidden` o `collapse` (computado, hereda)?
    fn visibility_hidden(&self, _n: NodeId) -> bool {
        false
    }
    /// ¿La caja del elemento es en línea (para no separar con espacios en el nombre)? Sin
    /// estilo, los elementos de frase de HTML.
    fn is_inline(&self, doc: &Document, n: NodeId) -> bool {
        doc.element(n)
            .is_some_and(|e| PHRASING.contains(&e.name.as_lossy_str()))
    }
    /// El texto de `content` de `::before` (`after = false`) o `::after` del elemento.
    fn pseudo_content(&self, _n: NodeId, _after: bool) -> Option<String> {
        None
    }
    /// El valor actual de un control (`input`, `textarea`); sin él, el atributo `value` o el
    /// contenido.
    fn control_value(&self, _n: NodeId) -> Option<String> {
        None
    }
    /// El `text-transform` computado del elemento (`uppercase`, `lowercase`, `capitalize`), que
    /// se aplica a sus nodos de texto.
    fn text_transform(&self, _n: NodeId) -> Option<String> {
        None
    }
}

/// `text-transform` sobre un texto (sin reglas de idioma).
fn transform_text(t: &str, how: &str) -> String {
    match how {
        "uppercase" => t.to_uppercase(),
        "lowercase" => t.to_lowercase(),
        "capitalize" => {
            let mut out = String::with_capacity(t.len());
            let mut start = true;
            for c in t.chars() {
                if start && c.is_alphanumeric() {
                    out.extend(c.to_uppercase());
                    start = false;
                } else {
                    if c.is_whitespace() {
                        start = true;
                    }
                    out.push(c);
                }
            }
            out
        }
        _ => t.to_owned(),
    }
}

/// Sin estilo.
pub struct NoStyle;

impl A11yEnv for NoStyle {}

/// Los elementos de frase de HTML (en línea por defecto).
const PHRASING: &[&str] = &[
    "a", "abbr", "b", "bdi", "bdo", "br", "cite", "code", "data", "dfn", "em", "i", "img", "input",
    "kbd", "label", "mark", "q", "s", "samp", "select", "small", "span", "strong", "sub", "sup",
    "textarea", "time", "u", "var", "wbr", "button", "output", "meter", "progress", "del", "ins",
    "font", "big", "tt", "nobr", "strike", "acronym",
];

/// Los roles concretos de WAI-ARIA 1.2 (y los de 1.3 que ya exponen los navegadores).
const CONCRETE_ROLES: &[&str] = &[
    "alert",
    "alertdialog",
    "application",
    "article",
    "banner",
    "blockquote",
    "button",
    "caption",
    "cell",
    "checkbox",
    "code",
    "columnheader",
    "combobox",
    "comment",
    "complementary",
    "contentinfo",
    "definition",
    "deletion",
    "dialog",
    "directory",
    "document",
    "emphasis",
    "feed",
    "figure",
    "form",
    "generic",
    "grid",
    "gridcell",
    "group",
    "heading",
    "image",
    "img",
    "insertion",
    "link",
    "list",
    "listbox",
    "listitem",
    "log",
    "main",
    "mark",
    "marquee",
    "math",
    "menu",
    "menubar",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "meter",
    "navigation",
    "none",
    "note",
    "option",
    "paragraph",
    "presentation",
    "progressbar",
    "radio",
    "radiogroup",
    "region",
    "row",
    "rowgroup",
    "rowheader",
    "scrollbar",
    "search",
    "searchbox",
    "separator",
    "slider",
    "spinbutton",
    "status",
    "strong",
    "subscript",
    "suggestion",
    "superscript",
    "switch",
    "tab",
    "table",
    "tablist",
    "tabpanel",
    "term",
    "textbox",
    "time",
    "timer",
    "toolbar",
    "tooltip",
    "tree",
    "treegrid",
    "treeitem",
];

/// Los atributos ARIA globales (WAI-ARIA 1.2 §6.4): con alguno, `presentation`/`none` no se
/// aplica.
const GLOBAL_ARIA: &[&str] = &[
    "aria-atomic",
    "aria-busy",
    "aria-controls",
    "aria-current",
    "aria-describedby",
    "aria-description",
    "aria-details",
    "aria-dropeffect",
    "aria-flowto",
    "aria-grabbed",
    "aria-hidden",
    "aria-keyshortcuts",
    "aria-label",
    "aria-labelledby",
    "aria-live",
    "aria-owns",
    "aria-relevant",
    "aria-roledescription",
];

/// Los roles con "name from: contents" (WAI-ARIA 1.2 §5.2.8.4).
const NAME_FROM_CONTENT: &[&str] = &[
    "button",
    "cell",
    "checkbox",
    "columnheader",
    "gridcell",
    "heading",
    "link",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "option",
    "radio",
    "row",
    "rowheader",
    "sectionhead",
    "switch",
    "tab",
    "tooltip",
    "treeitem",
];

/// El espacio ASCII de HTML.
fn is_ascii_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{c}' | '\r')
}

fn attr<'a>(doc: &'a Document, n: NodeId, name: &str) -> Option<&'a str> {
    doc.element(n)?.attr(name)
}

fn local_name(doc: &Document, n: NodeId) -> Option<String> {
    let e = doc.element(n)?;
    (e.ns == crate::Namespace::Html).then(|| e.name.as_lossy_str().to_owned())
}

/// El primer ancestro elemento (sin cruzar a la raíz del documento).
fn parent_element(doc: &Document, n: NodeId) -> Option<NodeId> {
    doc.parent(n).filter(|&p| doc.element(p).is_some())
}

/// ¿Está oculto (`hidden`, `aria-hidden=true`, `display: none` en él o un ancestro, o
/// `visibility: hidden`)? AccName §4.3.1 "hidden".
pub fn is_hidden(doc: &Document, n: NodeId, env: &dyn A11yEnv) -> bool {
    if env.visibility_hidden(n) {
        return true;
    }
    let mut cur = Some(n);
    while let Some(c) = cur {
        if doc.element(c).is_some() {
            if local_name(doc, c).is_some() && attr(doc, c, "hidden").is_some() {
                return true;
            }
            if attr(doc, c, "aria-hidden").is_some_and(|v| v.trim().eq_ignore_ascii_case("true")) {
                return true;
            }
            if env.display_none(c) {
                return true;
            }
        }
        cur = doc.parent(c);
    }
    false
}

/// ¿Es enfocable (para la resolución de conflictos de `presentation`)?
fn is_focusable(doc: &Document, n: NodeId) -> bool {
    if attr(doc, n, "tabindex").is_some_and(|t| t.trim().parse::<i64>().is_ok()) {
        return true;
    }
    match local_name(doc, n).as_deref() {
        Some("a" | "area") => attr(doc, n, "href").is_some(),
        Some("button" | "select" | "textarea") => !doc.is_disabled(n),
        Some("input") => {
            !doc.is_disabled(n)
                && attr(doc, n, "type").is_none_or(|t| !t.eq_ignore_ascii_case("hidden"))
        }
        Some("summary") => true,
        _ => attr(doc, n, "contenteditable")
            .is_some_and(|v| v.is_empty() || v.eq_ignore_ascii_case("true")),
    }
}

fn has_global_aria(doc: &Document, n: NodeId) -> bool {
    GLOBAL_ARIA.iter().any(|a| attr(doc, n, a).is_some())
}

/// El rol calculado de `n` (`""` para un nodo sin rol: texto, o elementos sin mapeo).
pub fn computed_role(doc: &Document, n: NodeId, env: &dyn A11yEnv) -> String {
    if doc.element(n).is_none() {
        return String::new();
    }
    if let Some(r) = explicit_role(doc, n, env) {
        return r;
    }
    implicit_role(doc, n, env).to_owned()
}

/// El rol de `role` (§5.4): el primer token conocido y concreto. `region` y `form` necesitan
/// nombre accesible para valer. `presentation`/`none` dan `none` salvo conflicto (y entonces
/// cuenta el rol implícito).
fn explicit_role(doc: &Document, n: NodeId, env: &dyn A11yEnv) -> Option<String> {
    let value = attr(doc, n, "role")?;
    for token in value.split(is_ascii_ws).filter(|t| !t.is_empty()) {
        let t = token.to_ascii_lowercase();
        if !CONCRETE_ROLES.contains(&t.as_str()) {
            continue;
        }
        match t.as_str() {
            "region" | "form" if !has_name_from_author(doc, n, env) => continue,
            "presentation" | "none" => {
                if is_focusable(doc, n) || has_global_aria(doc, n) {
                    return None;
                }
                return Some("none".into());
            }
            "img" => return Some("image".into()),
            "directory" => return Some("list".into()),
            _ => return Some(t),
        }
    }
    None
}

/// ¿Tiene nombre por `aria-labelledby` (a elementos existentes), `aria-label` no vacío o
/// `title` no vacío?
fn has_name_from_author(doc: &Document, n: NodeId, env: &dyn A11yEnv) -> bool {
    if let Some(ids) = attr(doc, n, "aria-labelledby")
        && ids
            .split(is_ascii_ws)
            .filter(|i| !i.is_empty())
            .any(|i| find_by_id(doc, n, i).is_some())
    {
        return true;
    }
    if attr(doc, n, "aria-label").is_some_and(|l| !l.trim().is_empty()) {
        return true;
    }
    let _ = env;
    attr(doc, n, "title").is_some_and(|t| !t.trim().is_empty())
}

/// El elemento con id `id` en el árbol de `n` (su documento o su shadow root).
pub fn find_by_id(doc: &Document, n: NodeId, id: &str) -> Option<NodeId> {
    let root = doc.get_root_node(n);
    doc.descendants(root)
        .into_iter()
        .find(|&d| attr(doc, d, "id") == Some(id))
}

/// ¿Algún ancestro es de "sectioning content" o `main` (para `header`/`footer`/`aside`)?
fn in_sectioning(doc: &Document, n: NodeId, with_main: bool) -> bool {
    let mut cur = parent_element(doc, n);
    while let Some(c) = cur {
        match local_name(doc, c).as_deref() {
            Some("article" | "aside" | "nav" | "section") => return true,
            Some("main") if with_main => return true,
            _ => {}
        }
        if let Some(r) = attr(doc, c, "role") {
            let r = r.trim().to_ascii_lowercase();
            if matches!(
                r.as_str(),
                "article" | "complementary" | "navigation" | "region"
            ) || (with_main && r == "main")
            {
                return true;
            }
        }
        cur = parent_element(doc, c);
    }
    false
}

/// El elemento `table` (o con rol de tabla) que contiene a `n`.
fn ancestor_table(doc: &Document, n: NodeId) -> Option<NodeId> {
    let mut cur = parent_element(doc, n);
    while let Some(c) = cur {
        if local_name(doc, c).as_deref() == Some("table") {
            return Some(c);
        }
        cur = parent_element(doc, c);
    }
    None
}

/// El rol implícito de HTML-AAM §4 (`""` sin mapeo; `generic` para los genéricos).
fn implicit_role(doc: &Document, n: NodeId, env: &dyn A11yEnv) -> &'static str {
    let Some(name) = local_name(doc, n) else {
        // SVG y MathML: lo mínimo.
        return match doc
            .element(n)
            .map(|e| (e.ns, e.name.as_lossy_str().to_owned()))
        {
            Some((crate::Namespace::Svg, ref s)) if s == "svg" => "graphics-document",
            Some((crate::Namespace::MathMl, ref s)) if s == "math" => "math",
            _ => "",
        };
    };
    let has = |a: &str| attr(doc, n, a).is_some();
    match name.as_str() {
        "a" => {
            if has("href") {
                "link"
            } else {
                "generic"
            }
        }
        "area" => {
            if has("href") {
                "link"
            } else {
                ""
            }
        }
        "address" | "details" | "fieldset" | "hgroup" | "optgroup" => "group",
        "article" => "article",
        "aside" => {
            if in_sectioning(doc, n, false) && !has_name_from_author(doc, n, env) {
                "generic"
            } else {
                "complementary"
            }
        }
        "b" | "bdi" | "bdo" | "data" | "div" | "i" | "pre" | "q" | "samp" | "small" | "span"
        | "u" | "body" | "legend" => "generic",
        "blockquote" => "blockquote",
        "button" => "button",
        "caption" => "caption",
        "code" => "code",
        "datalist" => "listbox",
        "dd" => "definition",
        "del" | "s" => "deletion",
        "dfn" | "dt" => "term",
        "dialog" => "dialog",
        "dir" | "menu" | "ol" | "ul" => "list",
        "em" => "emphasis",
        "figure" => "figure",
        "footer" => {
            if in_sectioning(doc, n, true) {
                "generic"
            } else {
                "contentinfo"
            }
        }
        "header" => {
            if in_sectioning(doc, n, true) {
                "generic"
            } else {
                "banner"
            }
        }
        "form" => "form",
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => "heading",
        "hr" => "separator",
        "html" => "document",
        "img" => {
            let empty_alt = attr(doc, n, "alt").is_some_and(str::is_empty);
            let named = attr(doc, n, "aria-label").is_some_and(|l| !l.trim().is_empty())
                || attr(doc, n, "aria-labelledby").is_some_and(|ids| {
                    ids.split(is_ascii_ws)
                        .filter(|i| !i.is_empty())
                        .any(|i| find_by_id(doc, n, i).is_some())
                });
            if empty_alt && !named { "none" } else { "image" }
        }
        "input" => input_role(doc, n),
        "ins" => "insertion",
        "li" => match parent_element(doc, n)
            .and_then(|p| local_name(doc, p))
            .as_deref()
        {
            Some("ul" | "ol" | "menu" | "dir") => "listitem",
            _ => "generic",
        },
        "main" => "main",
        "mark" => "mark",
        "math" => "math",
        "meter" => "meter",
        "nav" => "navigation",
        "option" => {
            let in_list = doc
                .inclusive_ancestors(n)
                .iter()
                .skip(1)
                .any(|&a| matches!(local_name(doc, a).as_deref(), Some("select" | "datalist")));
            if in_list { "option" } else { "" }
        }
        "output" => "status",
        "p" => "paragraph",
        "progress" => "progressbar",
        "search" => "search",
        "section" => {
            if has_name_from_author(doc, n, env) {
                "region"
            } else {
                "generic"
            }
        }
        "select" => {
            let multiple = has("multiple");
            let size = attr(doc, n, "size")
                .and_then(|s| s.trim().parse::<u32>().ok())
                .unwrap_or(0);
            if multiple || size > 1 {
                "listbox"
            } else {
                "combobox"
            }
        }
        "strong" => "strong",
        "sub" => "subscript",
        "sup" => "superscript",
        "summary" => {
            let first = parent_element(doc, n)
                .filter(|&p| local_name(doc, p).as_deref() == Some("details"))
                .and_then(|p| {
                    doc.children(p)
                        .find(|&c| local_name(doc, c).as_deref() == Some("summary"))
                });
            if first == Some(n) {
                "button"
            } else {
                "generic"
            }
        }
        "table" => "table",
        "tbody" | "thead" | "tfoot" => "rowgroup",
        "td" => {
            if ancestor_table(doc, n).is_some() {
                "cell"
            } else {
                ""
            }
        }
        "th" => th_role(doc, n),
        "tr" => {
            if ancestor_table(doc, n).is_some() {
                "row"
            } else {
                ""
            }
        }
        "textarea" => "textbox",
        "time" => "time",
        _ => {
            if name.contains('-') || !is_known_html(&name) {
                "generic"
            } else {
                ""
            }
        }
    }
}

fn is_known_html(name: &str) -> bool {
    !matches!(
        name,
        "abbr"
            | "audio"
            | "base"
            | "br"
            | "canvas"
            | "cite"
            | "col"
            | "colgroup"
            | "embed"
            | "figcaption"
            | "head"
            | "iframe"
            | "kbd"
            | "label"
            | "link"
            | "map"
            | "meta"
            | "noscript"
            | "object"
            | "param"
            | "picture"
            | "rp"
            | "rt"
            | "ruby"
            | "script"
            | "slot"
            | "source"
            | "style"
            | "template"
            | "title"
            | "track"
            | "var"
            | "video"
            | "wbr"
    ) || false
}

fn input_role(doc: &Document, n: NodeId) -> &'static str {
    let t = attr(doc, n, "type").unwrap_or("text").to_ascii_lowercase();
    let list = attr(doc, n, "list").is_some();
    match t.as_str() {
        "button" | "image" | "reset" | "submit" => "button",
        "checkbox" => "checkbox",
        "radio" => "radio",
        "range" => "slider",
        "number" => "spinbutton",
        "search" => {
            if list {
                "combobox"
            } else {
                "searchbox"
            }
        }
        "email" | "tel" | "text" | "url" | "" => {
            if list {
                "combobox"
            } else {
                "textbox"
            }
        }
        "hidden" | "password" | "color" | "date" | "datetime-local" | "month" | "time" | "week"
        | "file" => "",
        _ => {
            if list {
                "combobox"
            } else {
                "textbox"
            }
        }
    }
}

/// `th` (HTML-AAM): `columnheader` o `rowheader` según `scope` o su posición.
fn th_role(doc: &Document, n: NodeId) -> &'static str {
    if ancestor_table(doc, n).is_none() {
        return "";
    }
    match attr(doc, n, "scope")
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("row" | "rowgroup") => return "rowheader",
        Some("col" | "colgroup") => return "columnheader",
        _ => {}
    }
    let in_thead = doc
        .inclusive_ancestors(n)
        .iter()
        .any(|&a| local_name(doc, a).as_deref() == Some("thead"));
    if in_thead {
        return "columnheader";
    }
    // Un `th` con celdas de datos en su fila es encabezado de fila.
    let row = parent_element(doc, n);
    let has_td = row.is_some_and(|r| {
        doc.children(r)
            .any(|c| local_name(doc, c).as_deref() == Some("td"))
    });
    if has_td { "rowheader" } else { "columnheader" }
}

// --- Nombre y descripción (AccName 1.2 §4.3) ---

/// El nombre accesible de `n`.
pub fn accessible_name(doc: &Document, n: NodeId, env: &dyn A11yEnv) -> String {
    if doc.element(n).is_none() {
        return String::new();
    }
    let mut cx = Cx {
        doc,
        env,
        visited: HashSet::new(),
    };
    let s = cx.name(
        n,
        Mode {
            root: true,
            ..Mode::default()
        },
    );
    flatten(&s)
}

/// La descripción accesible de `n`: `aria-describedby`, `aria-description` o `title` (si no
/// se usó para el nombre).
pub fn accessible_description(doc: &Document, n: NodeId, env: &dyn A11yEnv) -> String {
    let mut cx = Cx {
        doc,
        env,
        visited: HashSet::new(),
    };
    if let Some(ids) = attr(doc, n, "aria-describedby") {
        let parts: Vec<String> = ids
            .split(is_ascii_ws)
            .filter(|i| !i.is_empty())
            .filter_map(|i| find_by_id(doc, n, i))
            .map(|r| {
                cx.visited.clear();
                let hidden = is_hidden(doc, r, env);
                cx.name(
                    r,
                    Mode {
                        referenced: true,
                        allow_hidden: hidden,
                        ..Mode::default()
                    },
                )
            })
            .collect();
        let s = flatten(&parts.join(" "));
        if !s.is_empty() {
            return s;
        }
    }
    if let Some(d) = attr(doc, n, "aria-description") {
        let s = flatten(d);
        if !s.is_empty() {
            return s;
        }
    }
    let name = accessible_name(doc, n, env);
    match attr(doc, n, "title") {
        Some(t) if flatten(t) != name => flatten(t),
        _ => String::new(),
    }
}

/// Espacios colapsados y recortados (lo que exponen los navegadores).
fn flatten(s: &str) -> String {
    s.split(|c: char| c.is_whitespace() && c != '\u{a0}')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Clone, Copy, Default)]
struct Mode {
    /// El nodo raíz del cálculo.
    root: bool,
    /// Dentro de un recorrido de `aria-labelledby`/`aria-describedby` (no se siguen otros).
    referenced: bool,
    /// Dentro de un recorrido recursivo (contenido de otro nodo, una etiqueta, una referencia).
    recursion: bool,
    /// El nodo referenciado estaba oculto: su subárbol cuenta igual (§4.3.2 paso 2A).
    allow_hidden: bool,
    /// Dentro de una etiqueta nativa (`label`) que nombra a otro control.
    in_label: bool,
}

struct Cx<'a> {
    doc: &'a Document,
    env: &'a dyn A11yEnv,
    visited: HashSet<NodeId>,
}

impl Cx<'_> {
    fn attr(&self, n: NodeId, a: &str) -> Option<&str> {
        attr(self.doc, n, a)
    }

    fn role(&self, n: NodeId) -> String {
        computed_role(self.doc, n, self.env)
    }

    /// "Compute the text alternative of the current node" (§4.3.2).
    fn name(&mut self, n: NodeId, mode: Mode) -> String {
        let doc = self.doc;
        // Texto (paso 2G).
        if let NodeData::Text(t) | NodeData::CdataSection(t) = &doc.get(n).data {
            if !mode.allow_hidden && self.env.visibility_hidden(n) {
                return String::new();
            }
            let text = t.as_lossy_str().to_owned();
            return match doc.parent(n).and_then(|p| self.env.text_transform(p)) {
                Some(how) => transform_text(&text, &how),
                None => text,
            };
        }
        if doc.element(n).is_none() {
            return String::new();
        }
        if !self.visited.insert(n) {
            return String::new();
        }
        // 2A: oculto y no referenciado (el nodo raíz se nombra aunque esté oculto, como en los
        // navegadores: un `dialog` cerrado tiene nombre).
        if !mode.root && !mode.allow_hidden && is_hidden(doc, n, self.env) {
            return String::new();
        }
        // 2B: `aria-labelledby` (no dentro de otro recorrido de referencias).
        if !mode.referenced
            && let Some(ids) = self.attr(n, "aria-labelledby")
        {
            let refs: Vec<NodeId> = ids
                .split(is_ascii_ws)
                .filter(|i| !i.is_empty())
                .filter_map(|i| find_by_id(doc, n, i))
                .collect();
            if !refs.is_empty() {
                let parts: Vec<String> = refs
                    .into_iter()
                    .map(|r| {
                        let hidden = is_hidden(doc, r, self.env);
                        self.visited.remove(&r);
                        let s = self.name(
                            r,
                            Mode {
                                root: false,
                                referenced: true,
                                recursion: true,
                                allow_hidden: hidden || mode.allow_hidden,
                                in_label: false,
                            },
                        );
                        s.trim().to_owned()
                    })
                    .filter(|s| !s.is_empty())
                    .collect();
                let s = parts.join(" ");
                if !s.trim().is_empty() {
                    return s;
                }
            }
        }
        let role = self.role(n);
        // 2C: un control dentro de la etiqueta de otro (o de un recorrido recursivo).
        if !mode.root
            && (mode.recursion || mode.in_label)
            && let Some(v) = self.embedded_control_value(n, &role)
        {
            return v;
        }
        // 2D: `aria-label`.
        if let Some(l) = self.attr(n, "aria-label")
            && !l.trim().is_empty()
        {
            return l.to_owned();
        }
        // Un `area` con `alt=""` no tiene nombre (ni de `title`).
        if local_name(doc, n).as_deref() == Some("area") && self.attr(n, "alt") == Some("") {
            return String::new();
        }
        // 2E: el nombre nativo de HTML (salvo `presentation`/`none`).
        if role != "none"
            && let Some(s) = self.native_name(n, mode)
            && !s.trim().is_empty()
        {
            return s;
        }
        // 2F: nombre desde el contenido.
        let from_content = NAME_FROM_CONTENT.contains(&role.as_str())
            || (!mode.root && (mode.recursion || mode.in_label))
            || matches!(
                local_name(doc, n).as_deref(),
                Some("label" | "legend" | "caption" | "figcaption")
            ) && !mode.root;
        let mut content = String::new();
        if from_content {
            content = self.content(n, mode);
            if !content.trim().is_empty() {
                return content;
            }
        }
        // 2I: `title` (y para los controles de texto, `placeholder`, que va en 2E).
        if let Some(t) = self.attr(n, "title")
            && !t.trim().is_empty()
        {
            return t.to_owned();
        }
        // En la recursión, un contenido de solo espacios cuenta (separa palabras).
        if mode.root { String::new() } else { content }
    }

    /// El valor de un control incrustado (§4.3.2 paso 2C).
    fn embedded_control_value(&mut self, n: NodeId, role: &str) -> Option<String> {
        let doc = self.doc;
        match role {
            "textbox" | "searchbox" => {
                if local_name(doc, n).as_deref() == Some("textarea") {
                    return Some(
                        self.env
                            .control_value(n)
                            .unwrap_or_else(|| doc.child_text_content(n).as_lossy_str().to_owned()),
                    );
                }
                Some(self.input_value(n))
            }
            "combobox" | "listbox" => {
                if local_name(doc, n).as_deref() == Some("select") {
                    let selected: Vec<String> = doc
                        .list_of_options(n)
                        .into_iter()
                        .filter(|&o| doc.option_selectedness(o))
                        .map(|o| flatten(&doc.text_content(o)))
                        .collect();
                    return Some(selected.join(" "));
                }
                if local_name(doc, n).as_deref() == Some("input") {
                    return Some(self.input_value(n));
                }
                // ARIA: las opciones seleccionadas.
                let selected: Vec<String> = doc
                    .descendants(n)
                    .into_iter()
                    .filter(|&d| {
                        self.attr(d, "aria-selected")
                            .is_some_and(|v| v.eq_ignore_ascii_case("true"))
                    })
                    .map(|d| flatten(&doc.text_content(d)))
                    .collect();
                Some(selected.join(" "))
            }
            "slider" | "spinbutton" | "progressbar" | "meter" | "scrollbar" => {
                if let Some(t) = self.attr(n, "aria-valuetext") {
                    return Some(t.to_owned());
                }
                if let Some(v) = self.attr(n, "aria-valuenow") {
                    return Some(v.trim().to_owned());
                }
                if local_name(doc, n).as_deref() == Some("input") {
                    return Some(self.input_value(n));
                }
                None
            }
            _ => None,
        }
    }

    fn input_value(&self, n: NodeId) -> String {
        self.env
            .control_value(n)
            .or_else(|| self.attr(n, "value").map(str::to_owned))
            .unwrap_or_default()
    }

    /// Los `label` de un control labelable (los que lo tienen por `for` o como descendiente).
    fn labels(&self, n: NodeId) -> Vec<NodeId> {
        let doc = self.doc;
        let root = doc.get_root_node(n);
        doc.descendants(root)
            .into_iter()
            .filter(|&l| local_name(doc, l).as_deref() == Some("label"))
            .filter(|&l| label_control(doc, l) == Some(n))
            .collect()
    }

    /// El texto de las etiquetas nativas de `n` unidas por espacios.
    fn labels_text(&mut self, n: NodeId, mode: Mode) -> Option<String> {
        let labels = self.labels(n);
        if labels.is_empty() {
            return None;
        }
        let parts: Vec<String> = labels
            .into_iter()
            .map(|l| {
                self.visited.remove(&l);
                let s = self.name(
                    l,
                    Mode {
                        root: false,
                        recursion: true,
                        in_label: true,
                        ..mode
                    },
                );
                s.trim().to_owned()
            })
            .filter(|s| !s.is_empty())
            .collect();
        Some(parts.join(" "))
    }

    /// `placeholder`, y si no, `aria-placeholder` (HTML-AAM §4.1.1).
    fn placeholder(&self, n: NodeId) -> Option<String> {
        ["placeholder", "aria-placeholder"].iter().find_map(|a| {
            self.attr(n, a)
                .filter(|v| !v.trim().is_empty())
                .map(str::to_owned)
        })
    }

    /// El primer hijo elemento con nombre local `child`.
    fn first_child(&self, n: NodeId, child: &str) -> Option<NodeId> {
        self.doc
            .children(n)
            .find(|&c| local_name(self.doc, c).as_deref() == Some(child))
    }

    /// El nombre del contenido de un hijo nativo (`legend`, `caption`, `figcaption`…).
    fn child_text(&mut self, c: NodeId, mode: Mode) -> String {
        self.visited.remove(&c);
        self.name(
            c,
            Mode {
                root: false,
                recursion: true,
                ..mode
            },
        )
    }

    /// HTML-AAM §4.1: el nombre nativo de cada elemento.
    fn native_name(&mut self, n: NodeId, mode: Mode) -> Option<String> {
        let doc = self.doc;
        let name = local_name(doc, n)?;
        match name.as_str() {
            "input" => {
                let t = self.attr(n, "type").unwrap_or("text").to_ascii_lowercase();
                match t.as_str() {
                    "button" | "submit" | "reset" => {
                        if let Some(l) = self.labels_text(n, mode).filter(|s| !s.trim().is_empty())
                        {
                            return Some(l);
                        }
                        if let Some(v) = self.attr(n, "value") {
                            return Some(v.to_owned());
                        }
                        match t.as_str() {
                            "submit" => Some("Submit".into()),
                            "reset" => Some("Reset".into()),
                            _ => self.attr(n, "title").map(str::to_owned),
                        }
                    }
                    "image" => {
                        if let Some(l) = self.labels_text(n, mode).filter(|s| !s.trim().is_empty())
                        {
                            return Some(l);
                        }
                        if let Some(a) = self.attr(n, "alt").filter(|a| !a.trim().is_empty()) {
                            return Some(a.to_owned());
                        }
                        if let Some(v) = self.attr(n, "value").filter(|a| !a.trim().is_empty()) {
                            return Some(v.to_owned());
                        }
                        if let Some(v) = self.attr(n, "title").filter(|a| !a.trim().is_empty()) {
                            return Some(v.to_owned());
                        }
                        Some("Submit".into())
                    }
                    "hidden" => None,
                    _ => {
                        if let Some(l) = self.labels_text(n, mode).filter(|s| !s.trim().is_empty())
                        {
                            return Some(l);
                        }
                        if let Some(t) = self.attr(n, "title").filter(|a| !a.trim().is_empty()) {
                            return Some(t.to_owned());
                        }
                        self.placeholder(n)
                    }
                }
            }
            "textarea" => {
                if let Some(l) = self.labels_text(n, mode).filter(|s| !s.trim().is_empty()) {
                    return Some(l);
                }
                if let Some(t) = self.attr(n, "title").filter(|a| !a.trim().is_empty()) {
                    return Some(t.to_owned());
                }
                self.placeholder(n)
            }
            "select" | "meter" | "output" | "progress" => {
                self.labels_text(n, mode).filter(|s| !s.trim().is_empty())
            }
            "button" => {
                if let Some(l) = self.labels_text(n, mode).filter(|s| !s.trim().is_empty()) {
                    return Some(l);
                }
                let s = self.content(n, mode);
                (!s.trim().is_empty()).then_some(s)
            }
            "img" => {
                if let Some(a) = self.attr(n, "alt")
                    && !a.trim().is_empty()
                {
                    return Some(a.to_owned());
                }
                None
            }
            // `area`: lo que esperan `html-aam/accname-computation-by-element/area.html` (y
            // hacen los navegadores): sin nombre nativo propio; `alt=""` lo deja sin nombre (ver
            // `name`).
            "area" => None,
            "fieldset" => {
                let l = self.first_child(n, "legend")?;
                Some(self.child_text(l, mode))
            }
            "figure" => {
                let c = doc
                    .children(n)
                    .find(|&c| local_name(doc, c).as_deref() == Some("figcaption"))?;
                Some(self.child_text(c, mode))
            }
            "table" => {
                let c = self.first_child(n, "caption")?;
                Some(self.child_text(c, mode))
            }
            "details" => None,
            "optgroup" => self.attr(n, "label").map(str::to_owned),
            "option" => {
                if let Some(l) = self.attr(n, "label").filter(|l| !l.is_empty()) {
                    return Some(l.to_owned());
                }
                None
            }
            _ => None,
        }
    }

    /// El nombre desde el contenido (§4.3.2 paso 2F): `::before`, los hijos y `::after`, con
    /// espacios alrededor de las cajas que no son en línea.
    fn content(&mut self, n: NodeId, mode: Mode) -> String {
        let doc = self.doc;
        let mut out = String::new();
        if let Some(b) = self.env.pseudo_content(n, false) {
            out.push_str(&b);
        }
        let children: Vec<NodeId> = doc.children(n).collect();
        for c in children {
            if doc.element(c).is_some() && is_hidden(doc, c, self.env) && !mode.allow_hidden {
                continue;
            }
            let is_el = doc.element(c).is_some();
            let block = is_el && !self.env.is_inline(doc, c);
            let s = if is_el {
                self.name(
                    c,
                    Mode {
                        root: false,
                        recursion: true,
                        ..mode
                    },
                )
            } else {
                self.name(c, mode)
            };
            if local_name(doc, c).as_deref() == Some("br") {
                out.push(' ');
                continue;
            }
            if block {
                out.push(' ');
                out.push_str(&s);
                out.push(' ');
            } else {
                out.push_str(&s);
            }
        }
        if let Some(a) = self.env.pseudo_content(n, true) {
            out.push_str(&a);
        }
        out
    }
}

/// El control etiquetado de un `label` (HTML §4.10.4): el del atributo `for` si es
/// labelable, o el primer descendiente labelable.
pub fn label_control(doc: &Document, label: NodeId) -> Option<NodeId> {
    if let Some(id) = attr(doc, label, "for") {
        let e = find_by_id(doc, label, id)?;
        return is_labelable(doc, e).then_some(e);
    }
    doc.descendants(label)
        .into_iter()
        .find(|&d| is_labelable(doc, d))
}

fn is_labelable(doc: &Document, n: NodeId) -> bool {
    match local_name(doc, n).as_deref() {
        Some("button" | "meter" | "output" | "progress" | "select" | "textarea") => true,
        Some("input") => attr(doc, n, "type").is_none_or(|t| !t.eq_ignore_ascii_case("hidden")),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (nombre local, atributos, texto) de cada hijo del `body`.
    type Spec<'a> = (&'a str, &'a [(&'a str, &'a str)], Option<&'a str>);

    fn doc_with(html: &[Spec<'_>]) -> (Document, Vec<NodeId>) {
        let mut d = Document::new();
        let root = d.root();
        let body = d.create_element(&"body".into()).expect("body");
        d.append(root, body);
        let mut ids = Vec::new();
        for (name, attrs, text) in html {
            let e = d.create_element(&(*name).into()).expect("elemento");
            for (k, v) in *attrs {
                let _ = d.set_attribute(e, &(*k).into(), (*v).into());
            }
            if let Some(t) = text {
                let tn = d.create_text_node((*t).into());
                d.append(e, tn);
            }
            d.append(body, e);
            ids.push(e);
        }
        (d, ids)
    }

    #[test]
    fn roles() {
        let (d, ids) = doc_with(&[
            ("a", &[("href", "#")], Some("x")),
            ("a", &[], Some("x")),
            ("section", &[], Some("x")),
            ("section", &[("aria-label", "s")], Some("x")),
            ("img", &[("alt", "")], None),
            ("div", &[("role", "foo button")], Some("x")),
            ("div", &[("role", "region group")], Some("x")),
            (
                "nav",
                &[("role", "presentation"), ("aria-label", "x")],
                Some("x"),
            ),
            ("span", &[("role", "none")], Some("x")),
            ("div", &[("role", "IMG")], None),
        ]);
        let got: Vec<String> = ids
            .iter()
            .map(|&n| computed_role(&d, n, &NoStyle))
            .collect();
        assert_eq!(
            got,
            [
                "link",
                "generic",
                "generic",
                "region",
                "none",
                "button",
                "group",
                "navigation",
                "none",
                "image"
            ]
        );
    }

    #[test]
    fn names() {
        let (d, ids) = doc_with(&[
            ("span", &[("id", "l1")], Some("uno")),
            ("span", &[("id", "l2")], Some("dos")),
            ("button", &[("aria-labelledby", "l1 l2")], Some("x")),
            ("button", &[("aria-label", "etiqueta")], Some("x")),
            ("button", &[], Some("  contenido   del botón ")),
            ("img", &[("alt", "foto")], None),
            ("input", &[("type", "submit")], None),
            ("input", &[("placeholder", "buscar")], None),
            ("strong", &[], Some("no")),
            ("div", &[("title", "tip")], None),
        ]);
        let got: Vec<String> = ids[2..]
            .iter()
            .map(|&n| accessible_name(&d, n, &NoStyle))
            .collect();
        assert_eq!(
            got,
            [
                "uno dos",
                "etiqueta",
                "contenido del botón",
                "foto",
                "Submit",
                "buscar",
                "",
                "tip"
            ]
        );
    }
}
