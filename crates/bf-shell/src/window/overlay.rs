//! Superposición de depuración (`F12`): los rectángulos de los fragmentos encima de la página
//! (bloques en el acento, inline en verde, texto en gris; borde de 1 px y relleno al 6%) y el
//! fragmento bajo el puntero resaltado. Es el inspector mínimo del navegador.

use bf_dom::Document;
use bf_layout::fragment::{Edges, Fragment, FragmentKind, FragmentTree, Rect};
use bf_paint::{DisplayItem, DisplayList, Radii};
use bf_style::properties::types::{BorderStyle, Position};
use bf_style::values::color::Rgba;
use bf_style::values::fmt_number;

use crate::theme::Theme;

fn intersects(a: &Rect, b: &Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

fn color_for(theme: &Theme, kind: FragmentKind) -> Option<Rgba> {
    match kind {
        FragmentKind::Line => None,
        FragmentKind::Inline => Some(theme.overlay_inline),
        FragmentKind::Text => Some(theme.overlay_text),
        _ => Some(theme.overlay_block),
    }
}

fn boxed(list: &mut DisplayList, rect: Rect, color: Rgba, fill: f32, width: f32) {
    list.push(DisplayItem::Rect {
        rect,
        color: Theme::with_alpha(color, fill),
        radii: Radii::default(),
    });
    list.push(DisplayItem::Border {
        rect,
        widths: Edges {
            top: width,
            right: width,
            bottom: width,
            left: width,
        },
        colors: [color; 4],
        styles: [BorderStyle::Solid; 4],
        radii: Radii::default(),
    });
}

/// La superposición para la parte visible de la página: `view` en coordenadas de la página
/// (el viewport desplazado) y `viewport` en las del viewport (para lo fijo). `hover` es el
/// fragmento resaltado (su rect y si es fijo).
pub fn overlay(
    tree: &FragmentTree,
    theme: &Theme,
    view: Rect,
    viewport: Rect,
    hover: Option<(Rect, bool)>,
) -> DisplayList {
    fn walk(
        f: &Fragment,
        fixed: bool,
        ctx: &(Theme, Rect, Rect),
        scrolled: &mut DisplayList,
        pinned: &mut DisplayList,
    ) {
        let (theme, view, viewport) = ctx;
        let is_box = !matches!(f.kind, FragmentKind::Line | FragmentKind::Text);
        let fixed = fixed || (is_box && f.style.position == Position::Fixed);
        let area = if fixed { viewport } else { view };
        if let Some(color) = color_for(theme, f.kind)
            && f.rect.width > 0.0
            && f.rect.height > 0.0
            && intersects(&f.rect, area)
        {
            boxed(
                if fixed { pinned } else { scrolled },
                f.rect,
                color,
                0.06,
                1.0,
            );
        }
        for c in &f.children {
            walk(c, fixed, ctx, scrolled, pinned);
        }
    }
    let mut scrolled = DisplayList::default();
    let mut pinned = DisplayList::default();
    if let Some(root) = &tree.root {
        walk(
            root,
            false,
            &(*theme, view, viewport),
            &mut scrolled,
            &mut pinned,
        );
    }
    if let Some((rect, fixed)) = hover {
        boxed(
            if fixed { &mut pinned } else { &mut scrolled },
            rect,
            theme.accent,
            0.25,
            2.0,
        );
    }
    if !pinned.items.is_empty() {
        scrolled.push(DisplayItem::PushFixed);
        scrolled.items.extend(pinned.items);
        scrolled.push(DisplayItem::PopFixed);
    }
    scrolled
}

/// `<tag.class> x,y w×h` del fragmento.
pub fn describe(doc: &Document, f: &Fragment) -> String {
    let mut label = match f.node.and_then(|n| doc.element(n)) {
        Some(el) => {
            let mut s = format!("<{}", el.name);
            if let Some(id) = el.attr("id") {
                s.push('#');
                s.push_str(id);
            }
            if let Some(class) = el.attr("class") {
                for c in class.split_ascii_whitespace().take(3) {
                    s.push('.');
                    s.push_str(c);
                }
            }
            s.push('>');
            s
        }
        None => "<anónimo>".to_owned(),
    };
    let kind = match f.kind {
        FragmentKind::Block => "bloque",
        FragmentKind::InlineBlock => "inline-block",
        FragmentKind::Inline => "inline",
        FragmentKind::Line => "línea",
        FragmentKind::Text => "texto",
        FragmentKind::Replaced => "reemplazado",
        FragmentKind::Marker => "marcador",
    };
    let r = f.rect;
    label.push_str(&format!(
        "  {kind}  {},{}  {}×{}",
        fmt_number(r.x),
        fmt_number(r.y),
        fmt_number(r.width),
        fmt_number(r.height)
    ));
    label
}
