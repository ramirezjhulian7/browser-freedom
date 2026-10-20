//! Edición de campos de texto de la página (`<input>` de texto y `<textarea>`): foco, caret,
//! selección dentro del campo, y el valor, que se guarda en el DOM (el atributo `value` o el
//! contenido del `<textarea>`) para que el layout lo muestre.

use bf_dom::{Document, NodeData, NodeId};
use bf_layout::fragment::{Fragment, FragmentKind, Rect};

/// Qué clase de control editable es.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editable {
    Input,
    TextArea,
}

/// El control con el foco de teclado y su estado de edición (índices en caracteres).
#[derive(Debug, Clone)]
pub struct Focus {
    pub node: NodeId,
    pub kind: Editable,
    pub caret: usize,
    /// Extremo fijo de la selección, si hay.
    pub anchor: Option<usize>,
}

impl Focus {
    pub fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        if a == self.caret {
            return None;
        }
        Some((a.min(self.caret), a.max(self.caret)))
    }
}

/// Tipos de `<input>` que se editan como texto.
fn text_type(ty: &str) -> bool {
    matches!(
        ty,
        "" | "text" | "search" | "url" | "email" | "password" | "tel" | "number"
    )
}

/// El control editable que contiene a `node` (él mismo o un ancestro), si lo hay.
pub fn editable_at(doc: &Document, node: NodeId) -> Option<(NodeId, Editable)> {
    let mut current = Some(node);
    while let Some(n) = current {
        if let Some(el) = doc.element(n)
            && el.ns == bf_infra::Namespace::Html
        {
            if el.name.as_str() == "textarea" {
                return (el.attr("disabled").is_none() && el.attr("readonly").is_none())
                    .then_some((n, Editable::TextArea));
            }
            if el.name.as_str() == "input" {
                let ty = el.attr("type").unwrap_or("").to_ascii_lowercase();
                return (text_type(&ty)
                    && el.attr("disabled").is_none()
                    && el.attr("readonly").is_none())
                .then_some((n, Editable::Input));
            }
        }
        current = doc.parent(n);
    }
    None
}

/// El valor actual del control.
pub fn value(doc: &Document, node: NodeId, kind: Editable) -> String {
    match kind {
        Editable::Input => doc
            .element(node)
            .and_then(|e| e.attr("value"))
            .map(|v| v.chars().filter(|c| !matches!(c, '\n' | '\r')).collect())
            .unwrap_or_default(),
        Editable::TextArea => doc.text_content(node),
    }
}

/// Escribe el valor en el DOM.
pub fn set_value(doc: &mut Document, node: NodeId, kind: Editable, text: &str) {
    match kind {
        Editable::Input => {
            if let Some(el) = doc.get_mut(node).element_mut() {
                if let Some(a) = el.attrs.iter_mut().find(|a| a.name.as_str() == "value") {
                    a.value = text.into();
                } else {
                    el.attrs.push(bf_dom::Attribute::new("value", text));
                }
            }
        }
        Editable::TextArea => {
            let children: Vec<NodeId> = doc.children(node).collect();
            for c in children {
                doc.remove(c);
            }
            let t = doc.create(NodeData::Text(text.into()));
            doc.append(node, t);
        }
    }
}

/// Los fragmentos de texto generados por el control `node`, en orden de documento.
fn text_fragments<'a>(root: &'a Fragment, node: NodeId, out: &mut Vec<&'a Fragment>) {
    if root.kind == FragmentKind::Text && root.node == Some(node) {
        out.push(root);
    }
    for c in &root.children {
        text_fragments(c, node, out);
    }
}

/// El fragmento (caja) del propio control.
fn control_fragment(root: &Fragment, node: NodeId) -> Option<&Fragment> {
    if root.node == Some(node) && root.kind != FragmentKind::Text && root.kind != FragmentKind::Line
    {
        return Some(root);
    }
    root.children.iter().find_map(|c| control_fragment(c, node))
}

/// Ancho en px de los primeros `chars` caracteres de un fragmento de texto.
fn advance_to(f: &Fragment, chars: usize) -> f32 {
    let Some((text, glyphs)) = &f.text else {
        return 0.0;
    };
    let byte = text
        .char_indices()
        .nth(chars)
        .map_or(text.len(), |(i, _)| i);
    glyphs
        .iter()
        .filter(|g| g.cluster < byte)
        .map(|g| g.advance)
        .sum()
}

/// Posición del caret (x, y, alto) en px de la página, para el carácter `caret` del valor.
/// Si el control no tiene texto (vacío o placeholder), al principio de su content box.
pub fn caret_rect(root: &Fragment, node: NodeId, caret: usize, has_value: bool) -> Option<Rect> {
    let mut frags = Vec::new();
    if has_value {
        text_fragments(root, node, &mut frags);
    }
    if frags.is_empty() {
        let c = control_fragment(root, node)?;
        let r = c.content_rect();
        let line = c
            .children
            .iter()
            .find(|f| f.kind == FragmentKind::Line)
            .map_or(r.height, |l| l.rect.height);
        return Some(Rect::new(r.x, r.y, 1.0, line.max(1.0)));
    }
    // Líneas: el caret va en la línea cuyo texto contiene el índice.
    let mut offset = 0;
    for (i, f) in frags.iter().enumerate() {
        let len = f.text.as_ref().map_or(0, |(t, _)| t.chars().count());
        let last = i + 1 == frags.len();
        if caret <= offset + len && (caret < offset + len || last || true) {
            let x = f.rect.x + advance_to(f, caret - offset);
            return Some(Rect::new(x, f.rect.y, 1.0, f.rect.height));
        }
        offset += len;
    }
    let f = frags.last()?;
    Some(Rect::new(f.rect.right(), f.rect.y, 1.0, f.rect.height))
}

/// Índice de carácter más cercano al punto `(x, y)` (px de la página) dentro del control.
pub fn caret_from_point(root: &Fragment, node: NodeId, x: f32, y: f32) -> usize {
    let mut frags = Vec::new();
    text_fragments(root, node, &mut frags);
    let mut offset = 0;
    let mut best: Option<(f32, usize)> = None;
    for f in &frags {
        let Some((text, _)) = &f.text else { continue };
        let len = text.chars().count();
        let dy = if y < f.rect.y {
            f.rect.y - y
        } else if y >= f.rect.bottom() {
            y - f.rect.bottom() + 0.001
        } else {
            0.0
        };
        if best.as_ref().is_some_and(|(d, _)| *d < dy) {
            offset += len;
            continue;
        }
        // Dentro de la línea: el borde de glifo más cercano a x.
        let mut idx = len;
        let mut prev = f.rect.x;
        for i in 0..=len {
            let ax = f.rect.x + advance_to(f, i);
            if x < (prev + ax) / 2.0 && i > 0 {
                idx = i - 1;
                break;
            }
            if x < ax {
                idx = i;
                break;
            }
            prev = ax;
        }
        if best.as_ref().is_none_or(|(d, _)| dy < *d) || dy == 0.0 {
            best = Some((dy, offset + idx));
        }
        offset += len;
    }
    best.map_or(0, |(_, i)| i)
}
