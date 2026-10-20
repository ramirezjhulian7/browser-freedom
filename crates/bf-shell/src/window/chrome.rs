//! El cromo de la ventana: la barra superior (marca, ◀ ▶ ⟳, campo de URL y botón del panel del
//! motor), la línea de estado y el panel del motor. Cada uno es un documento HTML generado
//! (plantillas de `pages/`) que maquetan y pintan el mismo motor y el mismo rasterizador que
//! las páginas; los iconos se agregan como paths ([`icons`](super::icons)) en los huecos que
//! deja la maqueta, y todos los colores salen del [`Theme`].

use std::sync::Arc;

use bf_dom::{Document, NodeId};
use bf_font::FontDb;
use bf_infra::{FileLoader, Url};
use bf_layout::Size;
use bf_layout::fragment::{Fragment, Rect};
use bf_paint::{DisplayItem, DisplayList, Page, PageContext, Radii, document_extent, prepare_page};

use super::hit;
use super::icons::{Icon, icon};
use crate::internal::{self, escape, fill};
use crate::theme::Theme;

/// Alto de la barra superior, de la línea de estado y ancho del panel, en px lógicos.
pub const BAR: f32 = 40.0;
pub const STATUS: f32 = 24.0;
pub const PANEL: f32 = 320.0;

/// Zonas activas de la barra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Back,
    Forward,
    Reload,
    Field,
    Engine,
}

const REGIONS: [(&str, Region); 5] = [
    ("back", Region::Back),
    ("forward", Region::Forward),
    ("reload", Region::Reload),
    ("field", Region::Field),
    ("engine", Region::Engine),
];

/// Un documento de la interfaz ya maquetado.
pub struct UiDoc {
    doc: Document,
    pub page: Page,
    pub size: Size,
}

impl UiDoc {
    pub fn build(html: &str, size: Size, fonts: Arc<FontDb>) -> UiDoc {
        let (doc, _) = bf_html::parse_document(html, &bf_html::ParseOptions::default());
        let base = Url::parse("about:blank", None).expect("URL válida");
        let ctx = PageContext {
            base: &base,
            encoding: bf_infra::encoding::Encoding::Utf8,
            loader: Arc::new(FileLoader::new()),
            fonts,
            extra: &[],
        };
        let page = prepare_page(&doc, &ctx, size);
        UiDoc { doc, page, size }
    }

    fn node(&self, id: &str) -> Option<NodeId> {
        let doc = &self.doc;
        doc.inclusive_descendants(doc.root())
            .into_iter()
            .find(|&n| doc.element(n).is_some_and(|e| e.attr("id") == Some(id)))
    }

    fn fragment(&self, id: &str) -> Option<&Fragment> {
        fn find(f: &Fragment, node: NodeId) -> Option<&Fragment> {
            if f.node == Some(node) {
                return Some(f);
            }
            f.children.iter().find_map(|c| find(c, node))
        }
        find(self.page.tree.root.as_ref()?, self.node(id)?)
    }

    /// Border box del elemento con ese `id`.
    pub fn rect(&self, id: &str) -> Option<Rect> {
        self.fragment(id).map(|f| f.rect)
    }

    /// El `href` del enlace en `(x, y)` (coordenadas del documento menos `scroll`).
    pub fn link_at(&self, x: f32, y: f32, scroll: f32) -> Option<String> {
        let f = hit::hit_test(&self.page.tree, x, y, (0.0, scroll))?;
        hit::link_at(&self.doc, f.node?).map(|(_, h)| h)
    }

    /// Alto del contenido.
    pub fn content_height(&self) -> f32 {
        document_extent(&self.page.tree).height
    }

    pub fn display_list(&self) -> &DisplayList {
        &self.page.display_list
    }
}

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

/// La URL sin foco, formateada: esquema y ruta en texto secundario, host en el principal.
fn formatted_url(url: &str) -> (bool, String) {
    let Ok(u) = Url::parse(url, None) else {
        return (false, escape(url));
    };
    let href = u.href();
    let host = u.host_string();
    let secure = u.scheme == "https";
    if host.is_empty() {
        return (false, format!("<span>{}</span>", escape(&href)));
    }
    match href.split_once(&host) {
        Some((before, after)) => (
            secure,
            format!(
                "<span class=\"sec\">{}</span><span>{}</span><span class=\"sec\">{}</span>",
                escape(before),
                escape(&host),
                escape(after)
            ),
        ),
        None => (secure, escape(&href)),
    }
}

/// Estado del cromo y sus maquetas.
pub struct Chrome {
    pub theme: Theme,
    fonts: Arc<FontDb>,
    pub field: UrlField,
    /// La URL que se muestra sin foco.
    pub url: String,
    pub can_back: bool,
    pub can_forward: bool,
    pub loading: bool,
    pub hover: Option<Region>,
    /// La página usó algo que el motor aún no soporta (punto ámbar en el botón del panel).
    pub engine_alert: bool,
    pub panel_open: bool,
    /// Texto de la izquierda de la línea de estado y si se resalta.
    pub status_left: String,
    pub status_em: bool,
    /// Partes de la derecha de la línea de estado (zoom, viewport, tiempo de carga).
    pub status_right: Vec<String>,
    width: f32,
    bar: Option<UiDoc>,
    regions: Vec<(Region, Rect)>,
    icons: Vec<(Icon, Rect, bool)>,
    field_rect: Option<Rect>,
    status: Option<UiDoc>,
    panel: Option<UiDoc>,
    pub panel_scroll: f32,
}

impl Chrome {
    pub fn new(fonts: Arc<FontDb>, theme: Theme) -> Chrome {
        Chrome {
            theme,
            fonts,
            field: UrlField::default(),
            url: String::new(),
            can_back: false,
            can_forward: false,
            loading: false,
            hover: None,
            engine_alert: false,
            panel_open: false,
            status_left: String::new(),
            status_em: false,
            status_right: Vec::new(),
            width: 0.0,
            bar: None,
            regions: Vec::new(),
            icons: Vec::new(),
            field_rect: None,
            status: None,
            panel: None,
            panel_scroll: 0.0,
        }
    }

    fn bar_html(&self) -> String {
        let f = &self.field;
        let button = |id: &str, enabled: bool, region: Region, on: bool| {
            let mut class = String::from("btn");
            if enabled && self.hover == Some(region) {
                class.push_str(" hover");
            }
            if on {
                class.push_str(" on");
            }
            let dot = if region == Region::Engine && self.engine_alert {
                "<span class=\"dot\"></span>"
            } else {
                ""
            };
            format!(
                "<span id=\"{id}\" class=\"{class}\"><span id=\"icon-{id}\" class=\"icon\"></span>{dot}</span>"
            )
        };
        let (lock, text) = if f.focused {
            let chars: Vec<char> = f.text.chars().collect();
            let part = |a: usize, b: usize| escape(&chars[a..b].iter().collect::<String>());
            let caret = "<span id=\"caret\"></span>";
            let mut text = String::new();
            match f.selection() {
                Some((s, e)) => {
                    text.push_str(&part(0, s));
                    if f.cursor == s {
                        text.push_str(caret);
                    }
                    text.push_str(&format!("<span class=\"sel\">{}</span>", part(s, e)));
                    if f.cursor == e {
                        text.push_str(caret);
                    }
                    text.push_str(&part(e, chars.len()));
                }
                None => {
                    text.push_str(&part(0, f.cursor));
                    text.push_str(caret);
                    text.push_str(&part(f.cursor, chars.len()));
                }
            }
            (false, text)
        } else {
            formatted_url(&self.url)
        };
        let vars = [
            ("theme", self.theme.css_vars()),
            ("brand", "<span class=\"brand\">bf</span>".to_owned()),
            ("back", button("back", self.can_back, Region::Back, false)),
            (
                "forward",
                button("forward", self.can_forward, Region::Forward, false),
            ),
            ("reload", button("reload", true, Region::Reload, false)),
            (
                "engine",
                button("engine", true, Region::Engine, self.panel_open),
            ),
            (
                "field_class",
                if f.focused { "focus" } else { "" }.to_owned(),
            ),
            (
                "lock",
                if lock {
                    "<span id=\"icon-lock\" class=\"icon\"></span>".to_owned()
                } else {
                    String::new()
                },
            ),
            ("scroll", format!("{}", -f.scroll)),
            ("text", text),
        ];
        fill(internal::BAR, &vars)
    }

    fn build_bar(&mut self) -> UiDoc {
        UiDoc::build(
            &self.bar_html(),
            Size {
                width: self.width,
                height: BAR,
            },
            self.fonts.clone(),
        )
    }

    /// Vuelve a maquetar la barra y la línea de estado con `width` px lógicos de ancho.
    pub fn layout(&mut self, width: f32) {
        self.width = width.max(1.0);
        let mut bar = self.build_bar();
        // Desplaza el texto del campo para que el cursor quede a la vista.
        let field = bar.fragment("field").map(Fragment::content_rect);
        if let (true, Some(field), Some(caret)) = (self.field.focused, field, bar.rect("caret")) {
            let mut scroll = self.field.scroll;
            if caret.right() > field.right() {
                scroll += caret.right() - field.right() + 2.0;
            } else if caret.x < field.x {
                scroll = (scroll - (field.x - caret.x) - 2.0).max(0.0);
            }
            if scroll != self.field.scroll {
                self.field.scroll = scroll;
                bar = self.build_bar();
            }
        } else if !self.field.focused {
            self.field.scroll = 0.0;
        }
        self.regions = REGIONS
            .iter()
            .filter_map(|&(id, region)| bar.rect(id).map(|r| (region, r)))
            .collect();
        let reload = if self.loading {
            Icon::Stop
        } else {
            Icon::Reload
        };
        self.icons = [
            ("icon-back", Icon::Back, self.can_back),
            ("icon-forward", Icon::Forward, self.can_forward),
            ("icon-reload", reload, true),
            ("icon-lock", Icon::Lock, true),
            ("icon-engine", Icon::Engine, true),
        ]
        .into_iter()
        .filter_map(|(id, i, enabled)| bar.rect(id).map(|r| (i, r, enabled)))
        .collect();
        self.field_rect = bar.rect("field");
        self.bar = Some(bar);
        self.layout_status();
    }

    fn layout_status(&mut self) {
        let right: String = self
            .status_right
            .iter()
            .map(|p| format!("<span>{}</span>", escape(p)))
            .collect();
        let vars = [
            ("theme", self.theme.css_vars()),
            (
                "left_class",
                if self.status_em { "l em" } else { "l" }.to_owned(),
            ),
            ("left", escape(&self.status_left)),
            ("right", right),
        ];
        self.status = Some(UiDoc::build(
            &fill(internal::STATUS, &vars),
            Size {
                width: self.width,
                height: STATUS,
            },
            self.fonts.clone(),
        ));
    }

    /// Maqueta el panel del motor con el cuerpo `body` y `height` px de alto.
    pub fn layout_panel(&mut self, body: &str, height: f32) {
        let html = fill(
            internal::PANEL,
            &[
                ("theme", self.theme.css_vars()),
                ("style", internal::STYLE.to_owned()),
                ("body", body.to_owned()),
            ],
        );
        self.panel = Some(UiDoc::build(
            &html,
            Size {
                width: PANEL,
                height: height.max(1.0),
            },
            self.fonts.clone(),
        ));
        self.clamp_panel_scroll();
    }

    pub fn clamp_panel_scroll(&mut self) {
        let max = self
            .panel
            .as_ref()
            .map_or(0.0, |p| (p.content_height() - p.size.height).max(0.0));
        self.panel_scroll = self.panel_scroll.clamp(0.0, max);
    }

    pub fn panel(&self) -> Option<&UiDoc> {
        self.panel.as_ref().filter(|_| self.panel_open)
    }

    pub fn status(&self) -> Option<&UiDoc> {
        self.status.as_ref()
    }

    /// La lista de visualización de la barra con los iconos y, si está cargando, la línea de
    /// progreso indeterminada en la fase `phase` (0 a 1).
    pub fn bar_display_list(&self, phase: f32) -> Option<DisplayList> {
        let bar = self.bar.as_ref()?;
        let mut list = bar.display_list().clone();
        for &(i, rect, enabled) in &self.icons {
            let color = match (i, enabled) {
                (Icon::Lock, _) => self.theme.text_secondary,
                (Icon::Engine, _) if self.panel_open => self.theme.accent,
                (_, true) => self.theme.text,
                (_, false) => Theme::with_alpha(self.theme.text_secondary, 0.5),
            };
            list.items.extend(icon(i, rect, color));
        }
        if self.loading
            && let Some(field) = self.field_rect
        {
            // Un tramo del 30% que recorre el ancho del campo, en el borde inferior de la barra.
            let band = Rect::new(field.x, BAR - 3.0, field.width, 2.0);
            let w = field.width * 0.3;
            let x = field.x - w + (field.width + w) * phase;
            list.push(DisplayItem::PushClip {
                rect: band,
                radii: Radii::default(),
            });
            list.push(DisplayItem::Rect {
                rect: Rect::new(x, band.y, w, band.height),
                color: self.theme.accent,
                radii: Radii([(1.0, 1.0); 4]),
            });
            list.push(DisplayItem::PopClip);
        }
        Some(list)
    }

    /// La zona activa de la barra en `(x, y)`.
    pub fn region_at(&self, x: f32, y: f32) -> Option<Region> {
        self.regions
            .iter()
            .find(|(_, r)| x >= r.x && x < r.right() && y >= r.y && y < r.bottom())
            .map(|(region, _)| *region)
    }

    pub fn bar_size(&self) -> Size {
        Size {
            width: self.width,
            height: BAR,
        }
    }

    pub fn status_size(&self) -> Size {
        Size {
            width: self.width,
            height: STATUS,
        }
    }
}
