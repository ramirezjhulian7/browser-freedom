//! Las descargas en la ventana (E11): empezar una ([`App::start_download`]), lo que llega del
//! documento como descarga ([`App::downloaded`]) y la barra inferior del marco con el progreso
//! (`pages/dlbar.html`, un documento de la interfaz como el menú). Los datos y el hilo de cada
//! descarga están en [`crate::downloads`].

use std::sync::Arc;
use std::time::{Duration, Instant};

use bf_infra::Url;
use bf_layout::Size;
use bf_layout::fragment::Rect;
use bf_paint::DisplayList;
use bf_style::values::color::Rgba;
use winit::window::CursorIcon;

use super::chrome::UiDoc;
use super::icons::{Icon, icon};
use super::{App, Navigation};
use crate::downloads::{self, State};
use crate::internal::{self, escape, fill};
use crate::theme::Theme;

/// Alto de la barra y ancho de cada tarjeta (con su margen).
pub const HEIGHT: f32 = 48.0;
const ITEM: f32 = 242.0;
/// Cada cuánto se redibuja el progreso mientras algo descarga.
pub const TICK: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// La tarjeta: abre la carpeta con el archivo (si terminó).
    Item(u64),
    /// Su botón: cancelar (en curso) o quitar de la barra.
    Close(u64),
    All,
    Hide,
}

/// La barra abierta.
pub struct DownloadBar {
    /// Se muestran las descargas desde este id (las de antes de abrir la ventana no).
    since: u64,
    dismissed: Vec<u64>,
    hover: Option<Target>,
    doc: Option<UiDoc>,
    origin: (f32, f32),
    icons: Vec<(Icon, Rect, Rgba)>,
    /// Lo que se mostró (id, bytes, estado), para no volver a maquetar sin cambios.
    shown: Vec<(u64, u64, State)>,
    width: f32,
}

impl DownloadBar {
    fn new(since: u64) -> DownloadBar {
        DownloadBar {
            since,
            dismissed: Vec::new(),
            hover: None,
            doc: None,
            origin: (0.0, 0.0),
            icons: Vec::new(),
            shown: Vec::new(),
            width: 0.0,
        }
    }

    /// Las descargas a mostrar: las más recientes que caben.
    fn visible(&self) -> Vec<(u64, u64, State)> {
        let fit = (((self.width - 180.0) / ITEM).floor() as usize).max(1);
        downloads::global()
            .list()
            .into_iter()
            .filter(|d| d.id >= self.since && !self.dismissed.contains(&d.id))
            .take(fit)
            .map(|d| (d.id, d.received, d.state.clone()))
            .collect()
    }

    fn layout(&mut self, area: Rect, fonts: Arc<bf_font::FontDb>, theme: &Theme) {
        self.width = area.width;
        let shown = self.visible();
        let g = downloads::global();
        let hover = |t: Target| if self.hover == Some(t) { " hover" } else { "" };
        let mut items = String::new();
        for (id, _, _) in &shown {
            let Some(d) = g.get(*id) else { continue };
            let pct = match (&d.state, d.fraction()) {
                (State::Running, Some(f)) => f * 100.0,
                (State::Running, None) => 30.0,
                (State::Done, _) => 100.0,
                _ => 0.0,
            };
            let bad = if matches!(d.state, State::Failed(_)) {
                " bad"
            } else {
                ""
            };
            items.push_str(&format!(
                "<span id=\"d-{id}\" class=\"item{}\"><span id=\"di-{id}\" class=\"icon\"></span>\
                 <span class=\"txt\"><span class=\"n\">{}</span><span class=\"s{bad}\">{}</span>\
                 <span class=\"track\"><span class=\"fill\" style=\"width:{pct:.1}%\"></span></span></span>\
                 <span id=\"dx-{id}\" class=\"btn{}\"><span id=\"dxi-{id}\" class=\"icon\"></span></span></span>",
                hover(Target::Item(*id)),
                escape(&d.name()),
                escape(&d.status()),
                hover(Target::Close(*id)),
            ));
        }
        drop(g);
        let vars = [
            ("theme", theme.css_vars()),
            ("items", items),
            ("all_hover", hover(Target::All).to_owned()),
            ("hide_hover", hover(Target::Hide).to_owned()),
        ];
        let doc = UiDoc::build(
            &fill(internal::DLBAR, &vars),
            Size {
                width: area.width,
                height: HEIGHT,
            },
            fonts,
        );
        let mut icons = Vec::new();
        for (id, _, state) in &shown {
            let color = match state {
                State::Running | State::Done => theme.accent,
                _ => theme.text_secondary,
            };
            if let Some(r) = doc.rect(&format!("di-{id}")) {
                icons.push((Icon::Download, r, color));
            }
            if let Some(r) = doc.rect(&format!("dxi-{id}")) {
                icons.push((Icon::Close, r, theme.text_secondary));
            }
        }
        if let Some(r) = doc.rect("icon-hide") {
            icons.push((Icon::Close, r, theme.text));
        }
        self.icons = icons;
        self.origin = (area.x, area.bottom() - HEIGHT);
        self.shown = shown;
        self.doc = Some(doc);
    }

    fn target_at(&self, x: f32, y: f32) -> Option<Option<Target>> {
        let doc = self.doc.as_ref()?;
        let (lx, ly) = (x - self.origin.0, y - self.origin.1);
        if !(0.0..HEIGHT).contains(&ly) || lx < 0.0 || lx >= doc.size.width {
            return None;
        }
        let inside = |id: &str| {
            doc.rect(id)
                .is_some_and(|r| lx >= r.x && lx < r.right() && ly >= r.y && ly < r.bottom())
        };
        let mut found = None;
        for (id, _, _) in &self.shown {
            if inside(&format!("dx-{id}")) {
                found = Some(Target::Close(*id));
                break;
            }
            if inside(&format!("d-{id}")) {
                found = Some(Target::Item(*id));
                break;
            }
        }
        if found.is_none() {
            if inside("all") {
                found = Some(Target::All);
            } else if inside("hide") {
                found = Some(Target::Hide);
            }
        }
        // `Some(None)`: sobre la barra, fuera de los botones.
        Some(found)
    }
}

impl App {
    /// Abre la barra (si no está) para mostrar la descarga `id` y las siguientes.
    fn show_downloads(&mut self, id: u64) {
        let bar = self.downloads.get_or_insert_with(|| DownloadBar::new(id));
        bar.since = bar.since.min(id);
        bar.shown.clear();
        self.layout_downloads();
    }

    /// Descarga `url` a la carpeta de descargas (con progreso en la barra), sin dejar la
    /// página actual.
    pub(super) fn start_download(&mut self, url: Url, name: Option<String>) {
        let id = downloads::start(&url, name.as_deref(), self.net.clone(), Arc::new(|| {}));
        self.show_downloads(id);
        self.set_notice(format!(
            "Descargando {}",
            downloads::global()
                .get(id)
                .map_or_else(String::new, downloads::Download::name)
        ));
    }

    /// Una navegación terminó en una respuesta que se guardó como descarga (`load.rs` la
    /// desvía a [`internal::download_response`]). Si había una página, se vuelve a ella (el
    /// hilo del documento ya la descargó de memoria, así que se pide de nuevo); devuelve si se
    /// encargó de la navegación.
    pub(super) fn downloaded(&mut self, saved: &str) -> bool {
        let last = downloads::global().list().first().map(|d| d.id);
        if let Some(id) = last {
            self.show_downloads(id);
        }
        self.set_notice(format!("Descargado en {saved}"));
        let Some(entry) = self.history.get(self.index) else {
            return false;
        };
        let url = entry.url.clone();
        self.start_load(url, Navigation::Traverse(self.index));
        true
    }

    /// Vuelve a maquetar la barra.
    pub(super) fn layout_downloads(&mut self) {
        let area = self.page_area();
        let theme = self.chrome.theme;
        let fonts = self.fonts.clone();
        if let Some(bar) = &mut self.downloads {
            bar.layout(area, fonts, &theme);
            self.redraw_later();
        }
    }

    /// Antes de pintar y en cada vuelta del bucle: si cambió el avance o el tamaño, vuelve a
    /// maquetar. Devuelve si hay descargas en curso (para seguir redibujando).
    pub(super) fn tick_downloads(&mut self) -> bool {
        let running = downloads::global().running() > 0;
        let area = self.page_area();
        if let Some(bar) = &self.downloads
            && (bar.visible() != bar.shown
                || bar.width != area.width
                || bar.origin.1 != area.bottom() - HEIGHT)
        {
            self.layout_downloads();
        }
        running
    }

    /// Un clic sobre la barra: devuelve `true` si era sobre ella.
    pub(super) fn downloads_click(&mut self, x: f32, y: f32) -> bool {
        let Some(bar) = &mut self.downloads else {
            return false;
        };
        let Some(target) = bar.target_at(x, y) else {
            return false;
        };
        match target {
            Some(Target::Item(id)) => {
                let g = downloads::global();
                if let Some(d) = g.get(id).filter(|d| d.state == State::Done) {
                    downloads::reveal(&d.path);
                }
            }
            Some(Target::Close(id)) => {
                let running = downloads::global()
                    .get(id)
                    .is_some_and(|d| d.state == State::Running);
                if running {
                    downloads::global().cancel(id);
                } else {
                    bar.dismissed.push(id);
                    if bar.visible().is_empty() {
                        self.downloads = None;
                    }
                }
                self.layout_downloads();
                self.redraw_later();
            }
            Some(Target::All) => {
                if let Ok(u) = Url::parse("freedom://downloads", None) {
                    self.new_tab(Some(u), false);
                }
            }
            Some(Target::Hide) => {
                self.downloads = None;
                self.redraw_later();
            }
            None => {}
        }
        true
    }

    /// El puntero sobre la barra: resalta y devuelve `true`.
    pub(super) fn downloads_hover(&mut self, x: f32, y: f32) -> bool {
        let Some(bar) = &mut self.downloads else {
            return false;
        };
        let Some(target) = bar.target_at(x, y) else {
            if bar.hover.take().is_some() {
                self.layout_downloads();
            }
            return false;
        };
        if target != bar.hover {
            bar.hover = target;
            self.layout_downloads();
        }
        self.set_cursor(if target.is_some() {
            CursorIcon::Pointer
        } else {
            CursorIcon::Default
        });
        true
    }

    /// La capa de la barra: su lista con los iconos, su esquina y su tamaño.
    pub(super) fn downloads_layer(&self) -> Option<(DisplayList, (f32, f32), Size)> {
        let bar = self.downloads.as_ref()?;
        let doc = bar.doc.as_ref()?;
        let mut list = doc.display_list().clone();
        for &(i, rect, color) in &bar.icons {
            list.items.extend(icon(i, rect, color));
        }
        Some((list, bar.origin, doc.size))
    }

    /// Sesiones sin ventana: espera (hasta 2 min) a que terminen las descargas en curso y
    /// deja la barra al día.
    pub(super) fn settle_downloads(&mut self) {
        let until = Instant::now() + Duration::from_secs(120);
        while downloads::global().running() > 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        self.tick_downloads();
    }

    /// Las descargas de la barra para la línea de estado de `freedom session`.
    pub(super) fn downloads_line(&self) -> String {
        let Some(bar) = &self.downloads else {
            return String::new();
        };
        let g = downloads::global();
        let items: Vec<String> = bar
            .shown
            .iter()
            .filter_map(|(id, _, _)| g.get(*id))
            .map(|d| format!("{} ({})", d.name(), d.status()))
            .collect();
        format!(" descargas={items:?}")
    }

    /// Cuándo volver a mirar el progreso.
    pub(super) fn downloads_deadline(&self) -> Instant {
        Instant::now() + TICK
    }
}
