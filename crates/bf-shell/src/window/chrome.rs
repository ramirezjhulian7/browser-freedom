//! La barra de herramientas de la ventana: botones ◀ ▶ ⟳, el campo de URL (texto, cursor y
//! selección) y el estado de la carga. Se describe como un documento HTML generado que
//! maquetan y pintan el mismo motor y el mismo rasterizador que las páginas.

use std::sync::Arc;

use bf_dom::{Document, NodeId};
use bf_font::FontDb;
use bf_infra::{FileLoader, Url};
use bf_layout::Size;
use bf_layout::fragment::{Fragment, Rect};
use bf_paint::{Page, PageContext, prepare_page};

/// Alto de la barra en px lógicos.
pub const HEIGHT: f32 = 36.0;

/// Zonas activas de la barra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Back,
    Forward,
    Reload,
    Field,
}

const REGIONS: [(&str, Region); 4] = [
    ("back", Region::Back),
    ("fwd", Region::Forward),
    ("reload", Region::Reload),
    ("field", Region::Field),
];

/// El campo de URL: texto editable con cursor y una selección (de `anchor` a `cursor`, en
/// caracteres).
#[derive(Debug, Clone, Default)]
pub struct UrlField {
    pub text: String,
    pub cursor: usize,
    pub anchor: Option<usize>,
    pub focused: bool,
    /// Desplazamiento horizontal del texto para que el cursor quede a la vista.
    scroll: f32,
}

impl UrlField {
    fn len(&self) -> usize {
        self.text.chars().count()
    }

    fn byte(&self, chars: usize) -> usize {
        self.text
            .char_indices()
            .nth(chars)
            .map_or(self.text.len(), |(i, _)| i)
    }

    /// La selección como `(inicio, fin)` si no está vacía.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        (a != self.cursor).then(|| (a.min(self.cursor), a.max(self.cursor)))
    }

    pub fn set_text(&mut self, text: String) {
        self.text = text;
        self.cursor = self.len();
        self.anchor = None;
        self.scroll = 0.0;
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.len();
    }

    fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection() else {
            return false;
        };
        let (bs, be) = (self.byte(start), self.byte(end));
        self.text.replace_range(bs..be, "");
        self.cursor = start;
        self.anchor = None;
        true
    }

    /// Escribe `s` en el cursor (reemplazando la selección).
    pub fn insert(&mut self, s: &str) {
        self.delete_selection();
        let at = self.byte(self.cursor);
        self.text.insert_str(at, s);
        self.cursor += s.chars().count();
    }

    pub fn backspace(&mut self) {
        if !self.delete_selection() && self.cursor > 0 {
            let (bs, be) = (self.byte(self.cursor - 1), self.byte(self.cursor));
            self.text.replace_range(bs..be, "");
            self.cursor -= 1;
        }
    }

    pub fn delete(&mut self) {
        if !self.delete_selection() && self.cursor < self.len() {
            let (bs, be) = (self.byte(self.cursor), self.byte(self.cursor + 1));
            self.text.replace_range(bs..be, "");
        }
    }

    /// Mueve el cursor a `to`; con `extend` (Mayús) amplía la selección.
    pub fn move_to(&mut self, to: usize, extend: bool) {
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = to.min(self.len());
    }

    pub fn left(&mut self, extend: bool) {
        match self.selection() {
            Some((start, _)) if !extend => self.move_to(start, false),
            _ => self.move_to(self.cursor.saturating_sub(1), extend),
        }
    }

    pub fn right(&mut self, extend: bool) {
        match self.selection() {
            Some((_, end)) if !extend => self.move_to(end, false),
            _ => self.move_to(self.cursor + 1, extend),
        }
    }

    pub fn home(&mut self, extend: bool) {
        self.move_to(0, extend);
    }

    pub fn end(&mut self, extend: bool) {
        self.move_to(self.len(), extend);
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Estado de la barra y su última maqueta.
pub struct Toolbar {
    pub field: UrlField,
    pub can_back: bool,
    pub can_forward: bool,
    /// Texto de estado a la derecha ("Cargando…", el destino del enlace bajo el puntero).
    pub status: String,
    pub hover: Option<Region>,
    page: Option<(Document, Page)>,
    regions: Vec<(Region, Rect)>,
    fonts: Arc<FontDb>,
    width: f32,
}

impl Toolbar {
    pub fn new(fonts: Arc<FontDb>) -> Toolbar {
        Toolbar {
            field: UrlField::default(),
            can_back: false,
            can_forward: false,
            status: String::new(),
            hover: None,
            page: None,
            regions: Vec::new(),
            fonts,
            width: 0.0,
        }
    }

    fn html(&self) -> String {
        let f = &self.field;
        let chars: Vec<char> = f.text.chars().collect();
        let part = |a: usize, b: usize| escape(&chars[a..b].iter().collect::<String>());
        let caret = "<span id=caret></span>";
        let mut text = String::new();
        match (f.focused, f.selection()) {
            (true, Some((s, e))) => {
                text.push_str(&part(0, s));
                if f.cursor == s {
                    text.push_str(caret);
                }
                text.push_str(&format!("<span class=sel>{}</span>", part(s, e)));
                if f.cursor == e {
                    text.push_str(caret);
                }
                text.push_str(&part(e, chars.len()));
            }
            (true, None) => {
                text.push_str(&part(0, f.cursor));
                text.push_str(caret);
                text.push_str(&part(f.cursor, chars.len()));
            }
            (false, _) => text.push_str(&part(0, chars.len())),
        }
        let button = |id: &str, label: &str, enabled: bool, region: Region| {
            let mut class = String::from("btn");
            if !enabled {
                class.push_str(" off");
            } else if self.hover == Some(region) {
                class.push_str(" hover");
            }
            format!("<span id={id} class=\"{class}\">{label}</span>")
        };
        format!(
            "<!DOCTYPE html><html><style>\
             html,body{{margin:0;height:{h}px;overflow:hidden;background:#dee1e6}}\
             body{{font:14px 'Segoe UI',sans-serif;color:#202124}}\
             .bar{{display:flex;align-items:center;height:{h}px;padding:0 6px;\
             border-bottom:1px solid #c4c7cc;box-sizing:border-box}}\
             .btn{{width:28px;height:28px;line-height:28px;text-align:center;\
             border-radius:14px;font:15px 'Segoe UI Symbol','Segoe UI',sans-serif;\
             color:#3c4043;margin-right:2px;flex:none}}\
             .btn.hover{{background:#c8cbd0}}\
             .btn.off{{color:#a8abb0}}\
             #field{{flex:1;height:26px;line-height:26px;margin:0 6px;padding:0 10px;\
             border-radius:13px;background:{field_bg};white-space:pre;overflow:hidden;\
             border:{border}}}\
             .sel{{background:#b3d7ff}}\
             #caret{{display:inline-block;width:1px;height:16px;background:#202124;\
             vertical-align:-3px;margin:0 -1px 0 0}}\
             #status{{flex:none;max-width:40%;overflow:hidden;white-space:pre;\
             font-size:12px;color:#5f6368;margin-right:4px}}\
             </style><div class=bar>{back}{fwd}{reload}\
             <div id=field><span style=\"margin-left:{scroll}px\">{text}</span></div>\
             <span id=status>{status}</span></div></html>",
            h = HEIGHT,
            field_bg = if f.focused { "#fff" } else { "#f1f3f4" },
            border = if f.focused {
                "1px solid #1a73e8"
            } else {
                "1px solid transparent"
            },
            back = button("back", "◀", self.can_back, Region::Back),
            fwd = button("fwd", "▶", self.can_forward, Region::Forward),
            reload = button("reload", "⟳", true, Region::Reload),
            scroll = -f.scroll,
            text = text,
            status = escape(&self.status),
        )
    }

    fn build(&mut self) -> (Document, Page) {
        let (doc, _) = bf_html::parse_document(&self.html(), &bf_html::ParseOptions::default());
        let base = Url::parse("about:blank", None).expect("URL válida");
        let ctx = PageContext {
            base: &base,
            encoding: bf_infra::encoding::Encoding::Utf8,
            loader: Arc::new(FileLoader::new()),
            fonts: self.fonts.clone(),
            extra: &[],
        };
        let size = Size {
            width: self.width,
            height: HEIGHT,
        };
        let page = prepare_page(&doc, &ctx, size);
        (doc, page)
    }

    /// Vuelve a maquetar la barra con `width` px lógicos de ancho.
    pub fn layout(&mut self, width: f32) {
        self.width = width.max(1.0);
        let (mut doc, mut page) = self.build();
        // Desplaza el texto del campo para que el cursor se vea.
        let field = self.field_content(&doc, &page);
        let caret = find(&doc, &page, "caret");
        if let (true, Some(field), Some(caret)) = (self.field.focused, field, caret) {
            let mut scroll = self.field.scroll;
            if caret.right() > field.right() {
                scroll += caret.right() - field.right() + 2.0;
            } else if caret.x < field.x {
                scroll = (scroll - (field.x - caret.x) - 2.0).max(0.0);
            }
            if scroll != self.field.scroll {
                self.field.scroll = scroll;
                (doc, page) = self.build();
            }
        } else if !self.field.focused {
            self.field.scroll = 0.0;
        }
        self.regions = REGIONS
            .iter()
            .filter_map(|&(id, region)| find(&doc, &page, id).map(|r| (region, r)))
            .collect();
        self.page = Some((doc, page));
    }

    fn field_content(&self, doc: &Document, page: &Page) -> Option<Rect> {
        let node = element_by_id(doc, "field")?;
        fragment_of(page.tree.root.as_ref()?, node).map(Fragment::content_rect)
    }

    /// La zona activa en `(x, y)` (px lógicos de la ventana).
    pub fn region_at(&self, x: f32, y: f32) -> Option<Region> {
        self.regions
            .iter()
            .find(|(_, r)| x >= r.x && x < r.right() && y >= r.y && y < r.bottom())
            .map(|(region, _)| *region)
    }

    pub fn page(&self) -> Option<&Page> {
        self.page.as_ref().map(|(_, p)| p)
    }

    pub fn size(&self) -> Size {
        Size {
            width: self.width,
            height: HEIGHT,
        }
    }
}

fn element_by_id(doc: &Document, id: &str) -> Option<NodeId> {
    doc.inclusive_descendants(doc.root())
        .into_iter()
        .find(|&n| doc.element(n).is_some_and(|e| e.attr("id") == Some(id)))
}

fn fragment_of(f: &Fragment, node: NodeId) -> Option<&Fragment> {
    if f.node == Some(node) {
        return Some(f);
    }
    f.children.iter().find_map(|c| fragment_of(c, node))
}

fn find(doc: &Document, page: &Page, id: &str) -> Option<Rect> {
    let node = element_by_id(doc, id)?;
    fragment_of(page.tree.root.as_ref()?, node).map(|f| f.rect)
}
