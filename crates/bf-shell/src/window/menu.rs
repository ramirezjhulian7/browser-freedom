//! Menú contextual (clic derecho): un documento HTML de la interfaz, maquetado y pintado por
//! el motor, que se compone encima de la página en [`super::App::frame`].

use std::sync::Arc;

use bf_font::FontDb;
use bf_layout::Size;
use bf_paint::DisplayList;

use super::chrome::UiDoc;
use crate::internal::{self, escape, fill};
use crate::theme::Theme;

/// Lo que hace cada entrada del menú.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    Back,
    Forward,
    Reload,
    /// Abrir `url` en una pestaña nueva en segundo plano.
    OpenInTab(String),
    /// Copiar un texto al portapapeles (direcciones, selección).
    CopyText(String),
    /// Guardar el recurso `url` (una imagen) con un diálogo de archivo.
    SaveResource(String),
    SavePage,
    ViewSource,
    Inspect,
    /// En un campo de texto.
    Cut,
    Copy,
    Paste,
    SelectAll,
}

pub struct MenuItem {
    pub label: &'static str,
    /// `None` es un separador.
    pub action: Option<MenuAction>,
    pub enabled: bool,
}

impl MenuItem {
    pub fn new(label: &'static str, action: MenuAction) -> MenuItem {
        MenuItem {
            label,
            action: Some(action),
            enabled: true,
        }
    }

    pub fn disabled(mut self, off: bool) -> MenuItem {
        self.enabled = !off;
        self
    }

    pub fn separator() -> MenuItem {
        MenuItem {
            label: "",
            action: None,
            enabled: false,
        }
    }
}

const WIDTH: f32 = 260.0;

pub struct Menu {
    pub items: Vec<MenuItem>,
    /// Esquina superior izquierda, en px lógicos de la ventana.
    pub origin: (f32, f32),
    pub hover: Option<usize>,
    doc: UiDoc,
    fonts: Arc<FontDb>,
    theme: Theme,
}

impl Menu {
    /// Un menú con `items` en `at`, movido si no cabe en una ventana de `window`.
    pub fn new(
        items: Vec<MenuItem>,
        at: (f32, f32),
        window: Size,
        fonts: Arc<FontDb>,
        theme: Theme,
    ) -> Menu {
        let doc = Self::build(&items, None, &fonts, &theme);
        let height = doc.content_height();
        let x = at.0.min((window.width - WIDTH - 4.0).max(0.0));
        let y = at.1.min((window.height - height - 4.0).max(0.0));
        Menu {
            items,
            origin: (x, y),
            hover: None,
            doc,
            fonts,
            theme,
        }
    }

    fn build(
        items: &[MenuItem],
        hover: Option<usize>,
        fonts: &Arc<FontDb>,
        theme: &Theme,
    ) -> UiDoc {
        let mut html = String::new();
        for (i, item) in items.iter().enumerate() {
            if item.action.is_none() {
                html.push_str("<div class=\"sep\"></div>");
                continue;
            }
            let mut class = String::from("item");
            if !item.enabled {
                class.push_str(" off");
            } else if hover == Some(i) {
                class.push_str(" hover");
            }
            html.push_str(&format!(
                "<span id=\"mi-{i}\" class=\"{class}\">{}</span>",
                escape(item.label)
            ));
        }
        let vars = [("theme", theme.css_vars()), ("items", html)];
        let size = Size {
            width: WIDTH,
            height: 2000.0,
        };
        UiDoc::build(&fill(internal::MENU, &vars), size, fonts.clone())
    }

    pub fn size(&self) -> Size {
        Size {
            width: WIDTH,
            height: self.doc.content_height(),
        }
    }

    /// Entrada habilitada bajo `(x, y)` (px lógicos de la ventana).
    pub fn item_at(&self, x: f32, y: f32) -> Option<usize> {
        let (lx, ly) = (x - self.origin.0, y - self.origin.1);
        (0..self.items.len())
            .filter(|&i| self.items[i].enabled)
            .find(|&i| {
                self.doc
                    .rect(&format!("mi-{i}"))
                    .is_some_and(|r| lx >= r.x && lx < r.right() && ly >= r.y && ly < r.bottom())
            })
    }

    /// Cambia la entrada resaltada; devuelve si cambió (y hay que redibujar).
    pub fn set_hover(&mut self, hover: Option<usize>) -> bool {
        if hover == self.hover {
            return false;
        }
        self.hover = hover;
        self.doc = Self::build(&self.items, hover, &self.fonts, &self.theme);
        true
    }

    pub fn display_list(&self) -> &DisplayList {
        self.doc.display_list()
    }
}
