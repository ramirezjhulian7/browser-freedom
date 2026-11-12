//! Buscar en la página (`Ctrl+F`, E11): la barra flotante arriba a la derecha de la página
//! (un documento de la interfaz, `pages/find.html`, como el menú contextual), el resaltado de
//! las coincidencias sobre el raster de la página y el paso entre ellas. La comparación está en
//! [`crate::find`] (NFC sin distinguir mayúsculas); el texto sale del índice de la selección
//! ([`super::select::TextIndex`]), sin tocar el documento.
//!
//! Teclas con la barra abierta: escribir busca mientras se escribe, `Enter` y `F3` van a la
//! siguiente, `Mayús+Enter` y `Mayús+F3` a la anterior, `Esc` cierra. Un clic en la página
//! suelta el foco sin cerrar la barra.

use std::sync::Arc;

use bf_font::FontDb;
use bf_layout::Size;
use bf_layout::fragment::Rect;
use bf_paint::{DisplayItem, DisplayList, Radii};
use bf_style::values::color::Rgba;
use winit::keyboard::{Key, NamedKey};

use super::keys;

use super::chrome::{UiDoc, UrlField};
use super::icons::{Icon, icon};
use super::select::Selection;
use super::{App, set_clipboard};
use crate::find::FindState;
use crate::internal::{self, escape, fill};
use crate::theme::Theme;

/// Ancho de la barra, el margen transparente para la sombra y la distancia al borde de la página.
const WIDTH: f32 = 400.0;
const PAD: f32 = 20.0;
const INSET: f32 = 12.0;

/// Botones de la barra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Prev,
    Next,
    Close,
    Field,
}

/// La barra de búsqueda abierta.
pub struct FindBar {
    pub field: UrlField,
    pub focused: bool,
    pub state: FindState,
    /// Para qué maquetado (pestaña, generación) valen las coincidencias.
    searched: Option<(u64, u64)>,
    hover: Option<Button>,
    doc: Option<UiDoc>,
    /// Esquina de la capa (con el margen) en px lógicos de la ventana, y la esquina superior
    /// derecha de la página con la que se calculó.
    origin: (f32, f32),
    anchor: (f32, f32),
    icons: Vec<(Icon, Rect, Rgba)>,
}

impl FindBar {
    fn new() -> FindBar {
        FindBar {
            field: UrlField::default(),
            focused: true,
            state: FindState::default(),
            searched: None,
            hover: None,
            doc: None,
            origin: (0.0, 0.0),
            anchor: (f32::NAN, f32::NAN),
            icons: Vec::new(),
        }
    }

    fn html(&self, theme: &Theme) -> String {
        let f = &self.field;
        let text = if f.text.is_empty() && !self.focused {
            "<span class=\"ph\">Buscar en la página</span>".to_owned()
        } else if self.focused {
            let chars: Vec<char> = f.text.chars().collect();
            let part = |a: usize, b: usize| escape(&chars[a..b].iter().collect::<String>());
            let caret = "<span id=\"caret\"></span>";
            match f.selection() {
                Some((s, e)) => format!(
                    "{}{}<span class=\"sel\">{}</span>{}{}",
                    part(0, s),
                    if f.cursor == s { caret } else { "" },
                    part(s, e),
                    if f.cursor == e { caret } else { "" },
                    part(e, chars.len())
                ),
                None if f.text.is_empty() => {
                    format!("{caret}<span class=\"ph\">Buscar en la página</span>")
                }
                None => format!(
                    "{}{caret}{}",
                    part(0, f.cursor),
                    part(f.cursor, chars.len())
                ),
            }
        } else {
            escape(&f.text)
        };
        let hover = |b: Button| {
            if self.hover == Some(b) {
                " hover".to_owned()
            } else {
                String::new()
            }
        };
        let none = !self.state.query.trim().is_empty() && self.state.matches.is_empty();
        let vars = [
            ("theme", theme.css_vars()),
            ("pad", PAD.to_string()),
            ("width", WIDTH.to_string()),
            (
                "bar_class",
                if self.focused { "focus" } else { "" }.to_owned(),
            ),
            ("text", text),
            ("count", escape(&self.state.counter())),
            ("count_class", if none { "none" } else { "" }.to_owned()),
            ("prev_hover", hover(Button::Prev)),
            ("next_hover", hover(Button::Next)),
            ("close_hover", hover(Button::Close)),
        ];
        fill(internal::FIND, &vars)
    }

    /// Maqueta la barra con su esquina derecha en `right` (px lógicos de la ventana) y su
    /// borde superior en `top`.
    fn layout(&mut self, right: f32, top: f32, fonts: Arc<FontDb>, theme: &Theme) {
        let doc = UiDoc::build(
            &self.html(theme),
            Size {
                width: WIDTH + 2.0 * PAD,
                height: 40.0 + 2.0 * PAD,
            },
            fonts,
        );
        let enabled = !self.state.matches.is_empty();
        let muted = Theme::with_alpha(theme.text_secondary, 0.5);
        let nav = if enabled { theme.text } else { muted };
        let wanted = [
            ("icon-find", Icon::Search, theme.text_secondary),
            ("icon-prev", Icon::Up, nav),
            ("icon-next", Icon::Down, nav),
            ("icon-close", Icon::Close, theme.text),
        ];
        self.icons = wanted
            .iter()
            .filter_map(|&(id, i, c)| doc.rect(id).map(|r| (i, r, c)))
            .collect();
        self.origin = (right - INSET - WIDTH - PAD, top + INSET - PAD);
        self.anchor = (right, top);
        self.doc = Some(doc);
    }

    /// El botón bajo `(x, y)` (px lógicos de la ventana), o `Field` en el resto de la barra.
    fn button_at(&self, x: f32, y: f32) -> Option<Button> {
        let doc = self.doc.as_ref()?;
        let (lx, ly) = (x - self.origin.0, y - self.origin.1);
        let inside = |r: Rect| lx >= r.x && lx < r.right() && ly >= r.y && ly < r.bottom();
        [
            ("prev", Button::Prev),
            ("next", Button::Next),
            ("close", Button::Close),
            ("bar", Button::Field),
        ]
        .into_iter()
        .find(|&(id, _)| doc.rect(id).is_some_and(inside))
        .map(|(_, b)| b)
    }
}

impl App {
    /// `Ctrl+F`: abre la barra (o le da el foco) con lo buscado seleccionado. Si hay texto
    /// seleccionado en la página, se busca ese.
    pub(super) fn open_find(&mut self) {
        let selected = self
            .selected_text()
            .filter(|t| !t.contains('\n') && t.chars().count() <= 200);
        let bar = self.find.get_or_insert_with(FindBar::new);
        bar.focused = true;
        self.chrome.field.focused = false;
        self.chrome.clear_suggestions();
        if let Some(t) = selected {
            bar.field.set_text(t);
            bar.state.query.clear();
        }
        bar.field.select_all();
        self.search_find(true);
        self.refresh_chrome();
    }

    pub(super) fn close_find(&mut self) {
        if self.find.take().is_some() {
            self.redraw_later();
        }
    }

    /// Vuelve a buscar si cambió lo buscado o el maquetado. `jump`: llevar la vista a la
    /// coincidencia actual.
    pub(super) fn search_find(&mut self, jump: bool) {
        let key = (self.tab_key, self.page_generation);
        let index = self.text_index();
        let Some(bar) = &mut self.find else {
            return;
        };
        let query_changed = bar.state.query != bar.field.text;
        if !query_changed && bar.searched == Some(key) {
            return;
        }
        let from = bar.state.current_match().map(|(s, _)| s);
        bar.state.query = bar.field.text.clone();
        match &index {
            Some(index) => bar.state.update(&index.find_runs(), from),
            None => bar.state.update(&[], None),
        }
        bar.searched = Some(key);
        if jump {
            self.scroll_to_find();
        }
        self.layout_find();
    }

    /// Siguiente (`forward`) o anterior coincidencia.
    pub(super) fn step_find(&mut self, forward: bool) {
        if self.find.is_none() {
            return self.open_find();
        }
        self.search_find(false);
        if let Some(bar) = &mut self.find {
            bar.state.step(forward);
        }
        self.scroll_to_find();
        self.layout_find();
    }

    /// Desplaza la página para que la coincidencia actual quede a la vista (a un tercio del
    /// alto, como al seguir un enlace).
    fn scroll_to_find(&mut self) {
        let (Some(bar), Some(index)) = (&self.find, self.text_index()) else {
            return;
        };
        let Some((s, e)) = bar.state.current_match() else {
            return;
        };
        let rects = index.highlight(
            &Selection {
                anchor: s,
                focus: e,
            },
            self.scroll,
        );
        let Some(first) = rects.first() else {
            return;
        };
        let vp = self.viewport();
        let (x, y) = self.scroll;
        let visible = first.y >= y + 8.0
            && first.bottom() <= y + vp.height - 8.0
            && first.x >= x
            && first.right() <= x + vp.width;
        if !visible {
            self.scroll.1 = first.y - vp.height / 3.0;
            if first.right() > x + vp.width || first.x < x {
                self.scroll.0 = first.x - vp.width / 3.0;
            }
            self.clamp_scroll();
        }
        self.redraw_later();
    }

    /// Vuelve a maquetar la barra (después de cambiar texto, foco, hover o resultados).
    pub(super) fn layout_find(&mut self) {
        let area = self.page_area();
        let theme = self.chrome.theme;
        let fonts = self.fonts.clone();
        if let Some(bar) = &mut self.find {
            bar.layout(area.right(), area.y, fonts, &theme);
            self.redraw_later();
        }
    }

    /// Antes de pintar: si el maquetado cambió (otra página, otra pestaña, re-layout), las
    /// coincidencias se buscan de nuevo.
    pub(super) fn sync_find(&mut self) {
        if self.find.is_some() {
            self.search_find(false);
        }
        let area = self.page_area();
        if self
            .find
            .as_ref()
            .is_some_and(|b| b.anchor != (area.right(), area.y))
        {
            self.layout_find();
        }
    }

    /// Las teclas con la barra abierta; devuelve si se usaron.
    pub(super) fn find_key(&mut self, key: &Key, text: Option<&str>) -> bool {
        let shift = self.modifiers.shift_key();
        let primary = keys::primary(self.modifiers, keys::MAC);
        let edit = keys::edit_command(key, self.modifiers, keys::MAC);
        let command = keys::blocks_text(self.modifiers, keys::MAC);
        let Some(bar) = &mut self.find else {
            return false;
        };
        match key {
            Key::Named(NamedKey::F3) => {
                self.step_find(!shift);
                return true;
            }
            Key::Character(c) if primary && c.eq_ignore_ascii_case("g") => {
                self.step_find(!shift);
                return true;
            }
            Key::Named(NamedKey::Escape) if bar.focused || self.loading.is_none() => {
                self.close_find();
                return true;
            }
            _ if !bar.focused => return false,
            Key::Named(NamedKey::Enter) => {
                self.step_find(!shift);
                return true;
            }
            _ => {}
        }
        let field = &mut bar.field;
        match key {
            Key::Named(NamedKey::Backspace) => field.backspace(),
            Key::Named(NamedKey::Delete) => field.delete(),
            Key::Named(NamedKey::ArrowLeft) => field.left(shift),
            Key::Named(NamedKey::ArrowRight) => field.right(shift),
            Key::Named(NamedKey::Home) => field.home(shift),
            Key::Named(NamedKey::End) => field.end(shift),
            Key::Named(NamedKey::Space) => field.insert(" "),
            Key::Character(_) if edit == Some(keys::EditCommand::SelectAll) => field.select_all(),
            Key::Character(_) if edit == Some(keys::EditCommand::Copy) => {
                if let Some(t) = field.selected_text() {
                    set_clipboard(&t);
                }
            }
            Key::Character(_) if edit == Some(keys::EditCommand::Paste) => {
                if let Some(t) = super::clipboard_text() {
                    field.paste(&t);
                }
            }
            _ if command => return false,
            _ => match text {
                Some(t) if !t.chars().any(char::is_control) => field.insert(t),
                // Con el foco en la barra, las demás teclas no desplazan la página.
                _ => return true,
            },
        }
        self.search_find(true);
        self.layout_find();
        true
    }

    /// Un clic: en la barra (botones o el campo) se usa y devuelve `true`; fuera de ella, la
    /// barra pierde el foco y el clic sigue su camino.
    pub(super) fn find_click(&mut self, x: f32, y: f32) -> bool {
        let Some(bar) = &mut self.find else {
            return false;
        };
        match bar.button_at(x, y) {
            Some(Button::Prev) => self.step_find(false),
            Some(Button::Next) => self.step_find(true),
            Some(Button::Close) => self.close_find(),
            Some(Button::Field) => {
                if !bar.focused {
                    bar.focused = true;
                    bar.field.select_all();
                    self.chrome.field.focused = false;
                    self.chrome.clear_suggestions();
                    self.refresh_chrome();
                    self.layout_find();
                }
            }
            None => {
                if bar.focused {
                    bar.focused = false;
                    self.layout_find();
                }
                return false;
            }
        }
        true
    }

    /// El puntero sobre la barra: resalta el botón y devuelve `true` (la página no lo ve).
    pub(super) fn find_hover(&mut self, x: f32, y: f32) -> bool {
        let Some(bar) = &mut self.find else {
            return false;
        };
        let at = bar.button_at(x, y);
        let hover = at.filter(|b| *b != Button::Field);
        if hover != bar.hover {
            bar.hover = hover;
            self.layout_find();
        }
        let cursor = match at {
            None => return false,
            Some(Button::Field) => winit::window::CursorIcon::Text,
            Some(_) => winit::window::CursorIcon::Pointer,
        };
        self.set_cursor(cursor);
        true
    }

    /// ¿Las teclas van a la barra (y no al documento)?
    pub(super) fn find_focused(&self) -> bool {
        self.find.as_ref().is_some_and(|b| b.focused)
    }

    /// El resaltado de las coincidencias en coordenadas de la página: todas en ámbar tenue y
    /// la actual más fuerte, con un borde.
    pub(super) fn find_highlights(&self) -> Option<DisplayList> {
        let bar = self.find.as_ref()?;
        if bar.state.matches.is_empty() {
            return None;
        }
        let index = self.text_index()?;
        let accent = self.chrome.theme.accent;
        let mut list = DisplayList::default();
        let current = bar.state.current;
        for (i, &(s, e)) in bar.state.matches.iter().enumerate() {
            let sel = Selection {
                anchor: s,
                focus: e,
            };
            for rect in index.highlight(&sel, self.scroll) {
                let alpha = if current == Some(i) { 0.55 } else { 0.25 };
                list.push(DisplayItem::Rect {
                    rect,
                    color: Theme::with_alpha(accent, alpha),
                    radii: Radii([(2.0, 2.0); 4]),
                });
                if current == Some(i) {
                    // El borde: cuatro tiras de 1.5 px.
                    let w = 1.5;
                    for r in [
                        Rect::new(rect.x - w, rect.y - w, rect.width + 2.0 * w, w),
                        Rect::new(rect.x - w, rect.bottom(), rect.width + 2.0 * w, w),
                        Rect::new(rect.x - w, rect.y, w, rect.height),
                        Rect::new(rect.right(), rect.y, w, rect.height),
                    ] {
                        list.push(DisplayItem::Rect {
                            rect: r,
                            color: accent,
                            radii: Radii::default(),
                        });
                    }
                }
            }
        }
        Some(list)
    }

    /// La capa de la barra: su lista con los iconos, su esquina y su tamaño.
    pub(super) fn find_layer(&self) -> Option<(DisplayList, (f32, f32), Size)> {
        let bar = self.find.as_ref()?;
        let doc = bar.doc.as_ref()?;
        let mut list = doc.display_list().clone();
        for &(i, rect, color) in &bar.icons {
            list.items.extend(icon(i, rect, color));
        }
        Some((list, bar.origin, doc.size))
    }

    /// Para las sesiones sin ventana: lo que muestra la barra.
    pub(super) fn find_line(&self) -> String {
        self.find.as_ref().map_or_else(String::new, |b| {
            format!(" buscar={:?} {:?}", b.field.text, b.state.counter())
        })
    }
}
