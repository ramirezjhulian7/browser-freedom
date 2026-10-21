//! Los diálogos de la página (`alert`, `confirm`, `prompt`, HTML §8.8.1): un documento HTML de
//! la interfaz (`pages/dialog.html`) que ocupa la ventana, oscurece la página y se compone
//! encima de todo en [`super::App::frame`]. Es modal: mientras está abierto la ventana no
//! manda nada a la página y el hilo del documento espera la respuesta.

use std::sync::Arc;
use std::sync::mpsc::Sender;

use bf_font::FontDb;
use bf_layout::Size;
use bf_paint::DisplayList;

use super::chrome::UiDoc;
use crate::internal::{self, escape, fill};
use crate::script::DialogKind;
use crate::theme::Theme;

/// Un diálogo abierto.
pub struct Dialog {
    pub kind: DialogKind,
    title: String,
    message: String,
    /// El texto del campo (`prompt`).
    pub input: String,
    /// El botón bajo el puntero (`true`: Aceptar).
    hover: Option<bool>,
    /// A quién responder; `None` una vez respondido.
    reply: Option<Sender<Option<String>>>,
    doc: UiDoc,
    size: Size,
    fonts: Arc<FontDb>,
    theme: Theme,
}

impl Dialog {
    /// Un diálogo para la página de `host`, del tamaño de la ventana `window`.
    pub fn new(
        kind: DialogKind,
        host: &str,
        message: &str,
        reply: Sender<Option<String>>,
        window: Size,
        fonts: Arc<FontDb>,
        theme: Theme,
    ) -> Dialog {
        let input = match &kind {
            DialogKind::Prompt(default) => default.clone(),
            _ => String::new(),
        };
        let mut d = Dialog {
            kind,
            title: format!("{host} dice"),
            message: message.to_owned(),
            input,
            hover: None,
            reply: Some(reply),
            doc: UiDoc::build("", window, fonts.clone()),
            size: window,
            fonts,
            theme,
        };
        d.rebuild();
        d
    }

    fn rebuild(&mut self) {
        let button = |id: &str, label: &str, ok: bool, hover: bool| {
            let mut class = String::from("button");
            if ok {
                class.push_str(" ok");
            } else if hover {
                class.push_str(" hover");
            }
            format!("<span id=\"{id}\" class=\"{class}\">{label}</span>")
        };
        let mut buttons = String::new();
        if !matches!(self.kind, DialogKind::Alert) {
            buttons.push_str(&button(
                "cancel",
                "Cancelar",
                false,
                self.hover == Some(false),
            ));
        }
        buttons.push_str(&button("ok", "Aceptar", true, self.hover == Some(true)));
        let field = if matches!(self.kind, DialogKind::Prompt(_)) {
            format!(
                "<div id=\"field\" class=\"field\"><span>{}</span><span class=\"caret\"></span></div>",
                escape(&self.input)
            )
        } else {
            String::new()
        };
        let vars = [
            ("theme", self.theme.css_vars()),
            ("title", escape(&self.title)),
            ("message", escape(&self.message)),
            ("field", field),
            ("buttons", buttons),
        ];
        self.doc = UiDoc::build(
            &fill(internal::DIALOG, &vars),
            self.size,
            self.fonts.clone(),
        );
    }

    /// La ventana cambió de tamaño.
    pub fn resize(&mut self, window: Size) {
        if window != self.size {
            self.size = window;
            self.rebuild();
        }
    }

    pub fn size(&self) -> Size {
        self.size
    }

    pub fn display_list(&self) -> &DisplayList {
        self.doc.display_list()
    }

    /// El botón en `(x, y)` (px lógicos de la ventana): `Some(true)` Aceptar,
    /// `Some(false)` Cancelar.
    pub fn button_at(&self, x: f32, y: f32) -> Option<bool> {
        let inside = |id: &str| {
            self.doc
                .rect(id)
                .is_some_and(|r| x >= r.x && x < r.right() && y >= r.y && y < r.bottom())
        };
        if inside("ok") {
            Some(true)
        } else if inside("cancel") {
            Some(false)
        } else {
            None
        }
    }

    /// Cambia el botón resaltado; devuelve si hay que redibujar.
    pub fn set_hover(&mut self, hover: Option<bool>) -> bool {
        if hover == self.hover {
            return false;
        }
        self.hover = hover;
        self.rebuild();
        true
    }

    /// Escribe en el campo (`prompt`).
    pub fn type_text(&mut self, text: &str) {
        if matches!(self.kind, DialogKind::Prompt(_)) {
            self.input.extend(text.chars().filter(|c| !c.is_control()));
            self.rebuild();
        }
    }

    /// Borra el último carácter del campo.
    pub fn backspace(&mut self) {
        if matches!(self.kind, DialogKind::Prompt(_)) && self.input.pop().is_some() {
            self.rebuild();
        }
    }

    /// La respuesta: aceptar (`ok`) o cancelar. Para `prompt`, el texto del campo; para
    /// `alert` y `confirm`, `Some("")` es aceptar.
    pub fn answer(mut self, ok: bool) {
        let value = match (&self.kind, ok) {
            (_, false) => None,
            (DialogKind::Prompt(_), true) => Some(self.input.clone()),
            (_, true) => Some(String::new()),
        };
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(value);
        }
    }
}

impl Drop for Dialog {
    /// Un diálogo que se cierra sin respuesta (se cerró la pestaña) se cancela: el hilo del
    /// documento no queda esperando.
    fn drop(&mut self) {
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use super::*;

    fn dialog(kind: DialogKind) -> (Dialog, std::sync::mpsc::Receiver<Option<String>>) {
        let (tx, rx) = channel();
        let window = Size {
            width: 800.0,
            height: 600.0,
        };
        let d = Dialog::new(
            kind,
            "example.test",
            "¿Seguro?",
            tx,
            window,
            FontDb::system(),
            Theme::for_system(Some(false)),
        );
        (d, rx)
    }

    #[test]
    fn prompt_edits_and_answers() {
        let (mut d, rx) = dialog(DialogKind::Prompt("Ana".into()));
        d.type_text("lía");
        d.backspace();
        assert_eq!(d.input, "Analí");
        d.answer(true);
        assert_eq!(rx.recv().unwrap(), Some("Analí".to_owned()));
    }

    #[test]
    fn buttons_are_hit_testable_and_drop_cancels() {
        let (d, rx) = dialog(DialogKind::Confirm);
        let ok = d.doc.rect("ok").expect("botón Aceptar");
        let cancel = d.doc.rect("cancel").expect("botón Cancelar");
        assert_eq!(d.button_at(ok.x + 2.0, ok.y + 2.0), Some(true));
        assert_eq!(d.button_at(cancel.x + 2.0, cancel.y + 2.0), Some(false));
        assert_eq!(d.button_at(1.0, 1.0), None);
        drop(d);
        assert_eq!(rx.recv().unwrap(), None);
    }

    #[test]
    fn alert_has_only_ok() {
        let (d, rx) = dialog(DialogKind::Alert);
        assert!(d.doc.rect("cancel").is_none());
        d.answer(true);
        assert_eq!(rx.recv().unwrap(), Some(String::new()));
    }
}
