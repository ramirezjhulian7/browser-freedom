//! E1: el árbol de accesibilidad de un documento (lo que leería un lector de pantalla).
//!
//! - **Qué nodos entran** (WAI-ARIA 1.2 §7.1 "Excluding Elements from the Accessibility Tree"
//!   y Core-AAM §5.1 "Including Elements"): fuera los ocultos (`hidden`, `aria-hidden="true"`,
//!   `display: none`, con todo su subárbol; `visibility: hidden` solo saca el nodo, porque un
//!   descendiente puede volver a ser visible) y lo que no se pinta (`head`, `script`, `style`,
//!   `template`, `input type=hidden`…). Los elementos sin semántica (`role=none`/
//!   `presentation`, `generic` sin nombre ni atributos ARIA globales, los que no tienen rol)
//!   no tienen nodo propio: sus hijos suben al padre (§9.3 y Core-AAM §5.1.2).
//! - **Hijos:** por el árbol plano (CSS Scoping §2.2: el shadow root de un host y lo asignado a
//!   cada `slot`), más los elementos de `aria-owns` (§6.6, al final y en su orden), que salen
//!   de su sitio en el DOM. Un elemento tiene un solo dueño (el primero en orden del árbol) y
//!   no se admiten ciclos. Los roles con "Children Presentational: True" (§5.2.8: `button`,
//!   `checkbox`, `image`…) son hojas.
//! - **Por nodo:** rol ([`super::computed_role`]), nombre y descripción (AccName 1.2 §4.3) y
//!   los estados de [`AxStates`].
//!
//! [`serialize`] da una forma de texto estable (una línea por nodo, sangrada) para los tests y
//! para depurar; el shell la puede usar para comparar árboles.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use super::{
    A11yEnv, GLOBAL_ARIA, accessible_description, accessible_name, attr, computed_role, find_by_id,
    flatten, is_ascii_ws, is_focusable, local_name, transform_text,
};
use crate::{Document, NodeData, NodeId};

/// Un valor de `aria-checked`/`aria-pressed` (WAI-ARIA 1.2 §6.6: "tristate").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tristate {
    False,
    True,
    Mixed,
}

impl Tristate {
    /// Como lo escribe WebDriver y la serialización: `true`, `false` o `mixed`.
    pub fn as_str(self) -> &'static str {
        match self {
            Tristate::False => "false",
            Tristate::True => "true",
            Tristate::Mixed => "mixed",
        }
    }

    fn parse(v: &str, mixed: bool) -> Option<Tristate> {
        match v.trim().to_ascii_lowercase().as_str() {
            "true" => Some(Tristate::True),
            "false" => Some(Tristate::False),
            "mixed" if mixed => Some(Tristate::Mixed),
            "mixed" => Some(Tristate::False),
            _ => None,
        }
    }
}

/// Los estados y propiedades de un nodo (WAI-ARIA 1.2 §6.4 y sus equivalentes nativos de
/// HTML-AAM §4). `None` es "no aplica" (no "falso").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AxStates {
    /// `aria-checked` o la checkedness de `input type=checkbox/radio` (`mixed`: indeterminado).
    pub checked: Option<Tristate>,
    /// `aria-pressed` de un botón conmutable.
    pub pressed: Option<Tristate>,
    /// `aria-expanded`, o el `open` del `details` de su `summary`.
    pub expanded: Option<bool>,
    /// `aria-selected`, o la selectedness de un `option`.
    pub selected: Option<bool>,
    /// Desactivado (el control nativo, o `aria-disabled="true"` en él o un ancestro).
    pub disabled: bool,
    /// Nivel de un encabezado (`h1`–`h6`, `aria-level`) o de un elemento con `aria-level`.
    pub level: Option<u32>,
    /// El valor de un control: el texto de un campo, la opción elegida de un `select`,
    /// `aria-valuetext`/`aria-valuenow` de un rango.
    pub value: Option<String>,
    /// ¿Puede recibir el foco? (para el foco del sistema; no se serializa).
    pub focusable: bool,
}

/// Un nodo del árbol de accesibilidad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxNode {
    /// El nodo del DOM que lo origina (el documento en la raíz; un nodo de texto en `text`).
    pub node: NodeId,
    /// El rol (los nombres de WebDriver "Get Computed Role"); `document` en la raíz y `text`
    /// en el texto.
    pub role: String,
    pub name: String,
    pub description: String,
    pub states: AxStates,
    pub children: Vec<AxNode>,
}

impl AxNode {
    /// El nodo de `n` (en preorden), si está en el árbol.
    pub fn find(&self, n: NodeId) -> Option<&AxNode> {
        if self.node == n {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(n))
    }

    /// Los nodos en orden de lectura (preorden).
    pub fn walk(&self) -> Vec<&AxNode> {
        let mut out = vec![self];
        for c in &self.children {
            out.extend(c.walk());
        }
        out
    }
}

/// Elementos que no se pintan y no entran nunca (con su subárbol).
const NOT_RENDERED: &[&str] = &[
    "base", "datalist", "head", "link", "meta", "noscript", "param", "script", "source", "style",
    "template", "title", "track",
];

/// Roles con "Children Presentational: True" (WAI-ARIA 1.2 §5.2.8): sus hijos no se exponen.
const PRESENTATIONAL_CHILDREN: &[&str] = &[
    "button",
    "checkbox",
    "image",
    "math",
    "menuitemcheckbox",
    "menuitemradio",
    "meter",
    "option",
    "progressbar",
    "radio",
    "scrollbar",
    "separator",
    "slider",
    "switch",
    "tab",
];

/// El árbol de accesibilidad de `doc`, con raíz en el documento (rol `document`, nombre el
/// del `title`).
pub fn accessibility_tree(doc: &Document, env: &dyn A11yEnv) -> AxNode {
    let root = doc.root();
    let mut b = Builder::new(doc, env);
    let children = b.children_of(root);
    AxNode {
        node: root,
        role: "document".into(),
        name: document_title(doc),
        description: String::new(),
        states: AxStates::default(),
        children,
    }
}

/// El subárbol de accesibilidad de `n` (`None` si `n` no tiene nodo propio en el árbol: oculto,
/// sin semántica o fuera de lo que se pinta).
pub fn accessibility_subtree(doc: &Document, n: NodeId, env: &dyn A11yEnv) -> Option<AxNode> {
    if n == doc.root() {
        return Some(accessibility_tree(doc, env));
    }
    let mut b = Builder::new(doc, env);
    if doc.element(n).is_some() {
        // Fuera del árbol: oculto (él o un ancestro), sin semántica propia, o dentro de un
        // rol cuyos hijos son presentacionales (y que no lo posee por `aria-owns`).
        let role = computed_role(doc, n, env);
        if super::is_hidden(doc, n, env) || b.transparent(n, &role) {
            return None;
        }
        let mut cur = b.owner.get(&n).copied().or_else(|| doc.flat_tree_parent(n));
        while let Some(a) = cur {
            if b.excluded(a) {
                return None;
            }
            if doc.element(a).is_some()
                && PRESENTATIONAL_CHILDREN.contains(&computed_role(doc, a, env).as_str())
            {
                return None;
            }
            cur = b.owner.get(&a).copied().or_else(|| doc.flat_tree_parent(a));
        }
    }
    let mut out = b.node(n);
    match out.len() {
        1 => out.pop(),
        _ => None,
    }
}

/// Los estados de `n` (los mismos que en el árbol, sin construirlo).
pub fn accessible_states(doc: &Document, n: NodeId, env: &dyn A11yEnv) -> AxStates {
    let role = computed_role(doc, n, env);
    states(doc, n, &role, env)
}

/// El texto del primer `title` del documento, con los espacios colapsados (HTML §3.1.3).
fn document_title(doc: &Document) -> String {
    let root = doc.root();
    doc.descendants(root)
        .into_iter()
        .find(|&d| local_name(doc, d).as_deref() == Some("title"))
        .map(|t| flatten(&doc.text_content(t)))
        .unwrap_or_default()
}

/// La serialización estable del árbol: una línea por nodo, sangrada dos espacios por nivel,
/// `rol "nombre"` y después los estados (`checked=`, `pressed=`, `expanded=`, `selected=`,
/// `disabled`, `level=`, `value="…"`, `description="…"`).
pub fn serialize(tree: &AxNode) -> String {
    let mut out = String::new();
    write_node(&mut out, tree, 0);
    out
}

fn quote(s: &str) -> String {
    let mut q = String::with_capacity(s.len() + 2);
    q.push('"');
    for c in s.chars() {
        match c {
            '"' => q.push_str("\\\""),
            '\\' => q.push_str("\\\\"),
            '\n' => q.push_str("\\n"),
            c => q.push(c),
        }
    }
    q.push('"');
    q
}

fn write_node(out: &mut String, n: &AxNode, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str(&n.role);
    if !n.name.is_empty() {
        out.push(' ');
        out.push_str(&quote(&n.name));
    }
    let s = &n.states;
    if let Some(c) = s.checked {
        let _ = write!(out, " checked={}", c.as_str());
    }
    if let Some(p) = s.pressed {
        let _ = write!(out, " pressed={}", p.as_str());
    }
    if let Some(e) = s.expanded {
        let _ = write!(out, " expanded={e}");
    }
    if let Some(sel) = s.selected {
        let _ = write!(out, " selected={sel}");
    }
    if s.disabled {
        out.push_str(" disabled");
    }
    if let Some(l) = s.level {
        let _ = write!(out, " level={l}");
    }
    if let Some(v) = &s.value {
        let _ = write!(out, " value={}", quote(v));
    }
    if !n.description.is_empty() {
        let _ = write!(out, " description={}", quote(&n.description));
    }
    out.push('\n');
    for c in &n.children {
        write_node(out, c, depth + 1);
    }
}

struct Builder<'a> {
    doc: &'a Document,
    env: &'a dyn A11yEnv,
    /// Elemento poseído → su dueño por `aria-owns`.
    owner: HashMap<NodeId, NodeId>,
    /// Dueño → lo que posee, en el orden de `aria-owns`.
    owned: HashMap<NodeId, Vec<NodeId>>,
    /// Nodos ya colocados (un nodo aparece una vez, aunque `aria-owns` lo pida dos).
    placed: HashSet<NodeId>,
}

impl<'a> Builder<'a> {
    fn new(doc: &'a Document, env: &'a dyn A11yEnv) -> Self {
        let mut b = Builder {
            doc,
            env,
            owner: HashMap::new(),
            owned: HashMap::new(),
            placed: HashSet::new(),
        };
        b.resolve_owns();
        b
    }

    /// `aria-owns` (WAI-ARIA 1.2 §6.6 y Core-AAM §5.4 "Relations"): en orden del árbol, cada
    /// id existente que no tenga ya dueño y que no sea el propio dueño ni un ancestro suyo
    /// (en el árbol de accesibilidad, contando los `aria-owns` anteriores: sin ciclos).
    fn resolve_owns(&mut self) {
        let doc = self.doc;
        let mut all = Vec::new();
        collect_flat(doc, doc.root(), &mut all);
        for o in all {
            let Some(ids) = attr(doc, o, "aria-owns") else {
                continue;
            };
            for id in ids.split(is_ascii_ws).filter(|i| !i.is_empty()) {
                let Some(t) = find_by_id(doc, o, id) else {
                    continue;
                };
                if t == o || self.owner.contains_key(&t) || self.is_ax_ancestor(t, o) {
                    continue;
                }
                self.owner.insert(t, o);
                self.owned.entry(o).or_default().push(t);
            }
        }
    }

    /// ¿`a` es ancestro inclusivo de `n` en el árbol de accesibilidad (padre plano o dueño)?
    fn is_ax_ancestor(&self, a: NodeId, n: NodeId) -> bool {
        let mut cur = Some(n);
        let mut guard = 0usize;
        while let Some(c) = cur {
            if c == a {
                return true;
            }
            guard += 1;
            if guard > 100_000 {
                return true;
            }
            cur = self
                .owner
                .get(&c)
                .copied()
                .or_else(|| self.doc.flat_tree_parent(c));
        }
        false
    }

    /// ¿Fuera del árbol con todo su subárbol?
    fn excluded(&self, n: NodeId) -> bool {
        let doc = self.doc;
        if doc.element(n).is_none() {
            return false;
        }
        if let Some(name) = local_name(doc, n) {
            if NOT_RENDERED.contains(&name.as_str()) {
                return true;
            }
            if name == "input"
                && attr(doc, n, "type").is_some_and(|t| t.trim().eq_ignore_ascii_case("hidden"))
            {
                return true;
            }
            if attr(doc, n, "hidden").is_some() {
                return true;
            }
        }
        attr(doc, n, "aria-hidden").is_some_and(|v| v.trim().eq_ignore_ascii_case("true"))
            || self.env.display_none(n)
    }

    /// Los hijos de accesibilidad de `n`: sus hijos planos (menos los que posee otro) y
    /// después lo que posee por `aria-owns`.
    fn children_of(&mut self, n: NodeId) -> Vec<AxNode> {
        let doc = self.doc;
        let mut out = Vec::new();
        for c in doc.flat_tree_children(n) {
            if self.owner.contains_key(&c) {
                continue;
            }
            out.extend(self.node(c));
        }
        let owned = self.owned.get(&n).cloned().unwrap_or_default();
        for c in owned {
            out.extend(self.node(c));
        }
        out
    }

    /// Lo que aporta `n` a la lista de hijos de su padre: su nodo, los nodos de sus hijos (si no
    /// tiene semántica propia) o nada.
    fn node(&mut self, n: NodeId) -> Vec<AxNode> {
        let doc = self.doc;
        if !self.placed.insert(n) {
            return Vec::new();
        }
        match &doc.get(n).data {
            NodeData::Text(t) | NodeData::CdataSection(t) => {
                let parent = doc.parent(n).filter(|&p| doc.element(p).is_some());
                if parent.is_some_and(|p| self.env.visibility_hidden(p)) {
                    return Vec::new();
                }
                let mut text = flatten(t.as_lossy_str());
                if text.is_empty() {
                    return Vec::new();
                }
                if let Some(how) = parent.and_then(|p| self.env.text_transform(p)) {
                    text = transform_text(&text, &how);
                }
                return vec![AxNode {
                    node: n,
                    role: "text".into(),
                    name: text,
                    description: String::new(),
                    states: AxStates::default(),
                    children: Vec::new(),
                }];
            }
            NodeData::Element(_) => {}
            _ => return Vec::new(),
        }
        if self.excluded(n) {
            return Vec::new();
        }
        let role = computed_role(doc, n, self.env);
        let invisible = self.env.visibility_hidden(n);
        if invisible || self.transparent(n, &role) {
            return self.children_of(n);
        }
        let name = accessible_name(doc, n, self.env);
        let description = accessible_description(doc, n, self.env);
        let states = states(doc, n, &role, self.env);
        let children = if PRESENTATIONAL_CHILDREN.contains(&role.as_str()) {
            // Lo que posee por `aria-owns` sí cuenta (es una relación explícita del autor).
            let owned = self.owned.get(&n).cloned().unwrap_or_default();
            owned.into_iter().flat_map(|c| self.node(c)).collect()
        } else {
            self.children_of(n)
        };
        vec![AxNode {
            node: n,
            role,
            name,
            description,
            states,
            children,
        }]
    }

    /// ¿Sin nodo propio? `none`, sin rol, o `generic` sin nombre, sin descripción, sin ARIA
    /// global y sin foco (Core-AAM §5.1.2: los navegadores no exponen esos `div`/`span`).
    fn transparent(&self, n: NodeId, role: &str) -> bool {
        let doc = self.doc;
        match role {
            "" | "none" => true,
            // El elemento `html` (rol `document`): la raíz del árbol ya es el documento.
            "document" => local_name(doc, n).as_deref() == Some("html"),
            "generic" => {
                !is_focusable(doc, n)
                    && !GLOBAL_ARIA
                        .iter()
                        .any(|a| *a != "aria-hidden" && attr(doc, n, a).is_some())
                    && attr(doc, n, "title").is_none_or(|t| t.trim().is_empty())
            }
            _ => false,
        }
    }
}

/// Todos los nodos de `n` hacia abajo por el árbol plano, en orden.
fn collect_flat(doc: &Document, n: NodeId, out: &mut Vec<NodeId>) {
    for c in doc.flat_tree_children(n) {
        if doc.element(c).is_some() {
            out.push(c);
            collect_flat(doc, c, out);
        }
    }
}

fn aria_true(doc: &Document, n: NodeId, a: &str) -> Option<bool> {
    match attr(doc, n, a)?.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn input_type(doc: &Document, n: NodeId) -> Option<String> {
    (local_name(doc, n).as_deref() == Some("input")).then(|| {
        attr(doc, n, "type")
            .unwrap_or("text")
            .trim()
            .to_ascii_lowercase()
    })
}

/// Los estados de `n` con rol `role` (WAI-ARIA 1.2 §6.6 para cada atributo; los nativos de
/// HTML-AAM §4 ganan a los de ARIA cuando existen, como pide HTML §3.2.6.4 "ARIA").
fn states(doc: &Document, n: NodeId, role: &str, env: &dyn A11yEnv) -> AxStates {
    let mut s = AxStates {
        focusable: is_focusable(doc, n),
        ..AxStates::default()
    };
    let itype = input_type(doc, n);
    let name = local_name(doc, n);
    // checked: obligatorio en checkbox/radio/switch/menuitemcheckbox/menuitemradio (sin
    // valor válido, `false`), opcional en `option`/`treeitem`.
    match itype.as_deref() {
        Some("checkbox") => {
            s.checked = Some(if env.indeterminate(n) {
                Tristate::Mixed
            } else if doc.checkedness(n) {
                Tristate::True
            } else {
                Tristate::False
            });
        }
        Some("radio") => {
            s.checked = Some(if doc.checkedness(n) {
                Tristate::True
            } else {
                Tristate::False
            });
        }
        _ => {
            let mixed = matches!(
                role,
                "checkbox" | "menuitemcheckbox" | "option" | "treeitem"
            );
            let v = attr(doc, n, "aria-checked").and_then(|v| Tristate::parse(v, mixed));
            s.checked = match role {
                "checkbox" | "radio" | "switch" | "menuitemcheckbox" | "menuitemradio" => {
                    Some(v.unwrap_or(Tristate::False))
                }
                "option" | "treeitem" => v,
                _ => None,
            };
        }
    }
    // pressed: solo los botones conmutables (`aria-pressed` con valor válido).
    if role == "button" {
        s.pressed = attr(doc, n, "aria-pressed").and_then(|v| Tristate::parse(v, true));
    }
    // expanded: `aria-expanded`; el `summary` de un `details`, su `open` (HTML-AAM §4).
    s.expanded = aria_true(doc, n, "aria-expanded");
    if s.expanded.is_none()
        && name.as_deref() == Some("summary")
        && role == "button"
        && let Some(details) = doc.parent(n)
    {
        s.expanded = Some(attr(doc, details, "open").is_some());
    }
    // selected: la selectedness del `option` de un `select`, si no `aria-selected`.
    if name.as_deref() == Some("option") && role == "option" {
        s.selected = Some(doc.option_selectedness(n));
    } else if matches!(
        role,
        "gridcell" | "option" | "row" | "tab" | "treeitem" | "columnheader" | "rowheader"
    ) {
        s.selected = aria_true(doc, n, "aria-selected");
    }
    // disabled: el nativo, o `aria-disabled="true"` en él o un ancestro (§6.6 aria-disabled:
    // "applies to the element and all focusable descendants").
    s.disabled = doc.is_disabled(n)
        || doc
            .inclusive_ancestors(n)
            .iter()
            .any(|&a| aria_true(doc, a, "aria-disabled") == Some(true));
    // level: `aria-level` (entero positivo); `hN`, N; `role=heading` sin nivel, 2 (§6.6).
    let aria_level = attr(doc, n, "aria-level")
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|&l| l > 0);
    s.level = match role {
        "heading" => aria_level.or_else(|| {
            name.as_deref()
                .and_then(|h| h.strip_prefix('h'))
                .and_then(|d| d.parse::<u32>().ok())
                .filter(|l| (1..=6).contains(l))
                .or(Some(2))
        }),
        "listitem" | "treeitem" | "row" | "comment" => aria_level,
        _ => None,
    };
    s.value = value(doc, n, role, itype.as_deref(), env);
    s
}

/// El valor de un control (HTML-AAM §4 y WAI-ARIA 1.2 §6.6 `aria-valuetext`/`aria-valuenow`).
fn value(
    doc: &Document,
    n: NodeId,
    role: &str,
    itype: Option<&str>,
    env: &dyn A11yEnv,
) -> Option<String> {
    match role {
        "textbox" | "searchbox" | "combobox" | "spinbutton" => {
            if local_name(doc, n).as_deref() == Some("select") {
                let opts: Vec<NodeId> = doc
                    .descendants(n)
                    .into_iter()
                    .filter(|&o| local_name(doc, o).as_deref() == Some("option"))
                    .collect();
                let chosen = opts
                    .iter()
                    .copied()
                    .find(|&o| doc.option_selectedness(o))
                    .or_else(|| opts.first().copied())?;
                let label = attr(doc, chosen, "label")
                    .map(str::to_owned)
                    .filter(|l| !l.is_empty())
                    .unwrap_or_else(|| flatten(&doc.text_content(chosen)));
                return Some(label);
            }
            if itype.is_some() || local_name(doc, n).as_deref() == Some("textarea") {
                let v = env.control_value(n).unwrap_or_else(|| {
                    if itype.is_some() {
                        attr(doc, n, "value").unwrap_or("").to_owned()
                    } else {
                        doc.text_content(n)
                    }
                });
                return Some(v);
            }
            range_value(doc, n)
        }
        "slider" | "progressbar" | "meter" | "scrollbar" => {
            if let Some(v) = range_value(doc, n) {
                return Some(v);
            }
            match (local_name(doc, n).as_deref(), itype) {
                (Some("input"), _) => env
                    .control_value(n)
                    .or_else(|| attr(doc, n, "value").map(str::to_owned)),
                (Some("progress" | "meter"), _) => {
                    attr(doc, n, "value").map(|v| v.trim().to_owned())
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn range_value(doc: &Document, n: NodeId) -> Option<String> {
    attr(doc, n, "aria-valuetext")
        .filter(|v| !v.trim().is_empty())
        .or_else(|| attr(doc, n, "aria-valuenow").filter(|v| !v.trim().is_empty()))
        .map(|v| v.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accessibility::NoStyle;

    fn el(d: &mut Document, parent: NodeId, name: &str, attrs: &[(&str, &str)]) -> NodeId {
        let e = d.create_element(&name.into()).expect("elemento");
        for (k, v) in attrs {
            let _ = d.set_attribute(e, &(*k).into(), (*v).into());
        }
        d.append(parent, e);
        e
    }

    fn text(d: &mut Document, parent: NodeId, t: &str) {
        let n = d.create_text_node(t.into());
        d.append(parent, n);
    }

    fn body() -> (Document, NodeId) {
        let mut d = Document::new();
        let root = d.root();
        let html = el(&mut d, root, "html", &[]);
        let body = el(&mut d, html, "body", &[]);
        (d, body)
    }

    #[test]
    fn aria_owns_moves_once_and_rejects_cycles() {
        let (mut d, b) = body();
        // `a` posee a `c` y a `b`; `b` intenta poseer a `a` (ciclo) y a `c` (ya tiene dueño).
        let a = el(
            &mut d,
            b,
            "div",
            &[("role", "list"), ("id", "a"), ("aria-owns", "c b")],
        );
        let li = el(&mut d, a, "div", &[("role", "listitem")]);
        text(&mut d, li, "uno");
        let bb = el(
            &mut d,
            b,
            "div",
            &[("role", "group"), ("id", "b"), ("aria-owns", "a c")],
        );
        text(&mut d, bb, "grupo");
        let c = el(&mut d, b, "div", &[("role", "listitem"), ("id", "c")]);
        text(&mut d, c, "dos");
        let tree = serialize(&accessibility_tree(&d, &NoStyle));
        assert_eq!(
            tree,
            "document\n  list\n    listitem\n      text \"uno\"\n    listitem\n      \
             text \"dos\"\n    group\n      text \"grupo\"\n"
        );
    }

    #[test]
    fn presentational_children_hidden_and_quotes() {
        let (mut d, b) = body();
        let btn = el(&mut d, b, "button", &[]);
        let s = el(&mut d, btn, "span", &[]);
        text(&mut d, s, "Di \"hola\"");
        let h = el(&mut d, b, "section", &[("hidden", "")]);
        text(&mut d, h, "nada");
        let none = el(&mut d, b, "ul", &[("role", "none")]);
        let li = el(&mut d, none, "li", &[]);
        text(&mut d, li, "suelto");
        let heading = el(&mut d, b, "div", &[("role", "heading")]);
        text(&mut d, heading, "Sin nivel");
        let tree = accessibility_tree(&d, &NoStyle);
        assert_eq!(
            serialize(&tree),
            "document\n  button \"Di \\\"hola\\\"\"\n  text \"suelto\"\n  \
             heading \"Sin nivel\" level=2\n    text \"Sin nivel\"\n"
        );
        // Un `generic` sin semántica no tiene nodo propio; el botón sí.
        assert!(accessibility_subtree(&d, s, &NoStyle).is_none());
        assert_eq!(
            accessibility_subtree(&d, btn, &NoStyle).map(|n| n.role),
            Some("button".to_owned())
        );
        assert!(tree.find(h).is_none());
        assert_eq!(tree.walk().len(), 5);
    }

    #[test]
    fn aria_states() {
        let (mut d, b) = body();
        let cb = el(
            &mut d,
            b,
            "div",
            &[("role", "checkbox"), ("aria-checked", "mixed")],
        );
        let sw = el(
            &mut d,
            b,
            "div",
            &[("role", "switch"), ("aria-checked", "bogus")],
        );
        let tb = el(
            &mut d,
            b,
            "div",
            &[("role", "button"), ("aria-pressed", "false")],
        );
        let plain = el(&mut d, b, "div", &[("role", "button")]);
        let fs = el(&mut d, b, "div", &[("aria-disabled", "true")]);
        let inner = el(
            &mut d,
            fs,
            "div",
            &[("role", "slider"), ("aria-valuenow", "5")],
        );
        let st = |n| accessible_states(&d, n, &NoStyle);
        assert_eq!(st(cb).checked, Some(Tristate::Mixed));
        assert_eq!(st(sw).checked, Some(Tristate::False));
        assert_eq!(st(tb).pressed, Some(Tristate::False));
        assert_eq!(st(plain).pressed, None);
        assert!(st(inner).disabled);
        assert_eq!(st(inner).value.as_deref(), Some("5"));
    }
}
