//! `browser-freedom open [url|archivo]`: la ventana del navegador (winit + softbuffer).
//!
//! La interfaz muestra lo que el motor hace con cada página. Todo el cromo se dibuja con el
//! propio motor y el propio rasterizador, con la paleta de [`Theme`]:
//! - **Barra superior** (40 px, [`chrome`]):
//!   - la marca `bf`, atrás, adelante y recargar (detener mientras carga);
//!   - el campo de URL: sin foco, formateado (candado en `https`, host destacado); con foco,
//!     editable con cursor y selección; una línea de progreso ámbar mientras carga;
//!   - el botón del panel del motor, con un punto ámbar si la página usó algo sin soporte.
//! - **Panel del motor** (320 px a la derecha, `Ctrl+I`): tiempos por fase, datos del
//!   documento, lo que el motor ignoró y las fuentes resueltas ([`crate::internal`]).
//! - **Línea de estado** (24 px): el enlace bajo el puntero; zoom, viewport y tiempo de carga.
//! - **Superposición de depuración** (`F12`, [`overlay`]): los fragmentos sobre la página.
//! - Carga en un hilo ([`worker`]); navegación por enlaces, fragmentos e historial; scroll con
//!   `position: fixed` fuera del desplazamiento; zoom; páginas internas `about:`.
//!
//! Los atajos están en [`crate::internal::SHORTCUTS`] (y en `about:home`).

mod chrome;
mod edit;
mod hit;
mod icons;
mod menu;
mod overlay;
mod worker;

use std::cell::{Cell, RefCell};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bf_dom::NodeId;
use bf_font::FontDb;
use bf_infra::Url;
use bf_layout::fragment::Rect;
use bf_layout::{Fragment, Size};
use bf_net::NetLoader;
use bf_paint::{
    DisplayItem, DisplayList, Page, Pixmap, PixmapPaint, Radii, Transform, document_extent, render,
    render_scrolled,
};
use bf_style::values::color::Rgba;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey, SmolStr};
use winit::window::{CursorIcon, Window, WindowId};

use crate::internal;
use crate::load::{Loaded, indicated_element};
use crate::theme::Theme;
use chrome::{BAR, Chrome, PANEL, Region, STATUS, TabInfo};
use edit::{Editable, Focus};
use menu::{Menu, MenuAction, MenuItem};
use worker::{Done, Job, LoadJob, Worker};

/// Px CSS por "línea" de rueda y por pulsación de flecha.
const LINE: f32 = 40.0;

/// Niveles de zoom (como los de los navegadores).
const ZOOM_LEVELS: [f32; 15] = [
    0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
];

/// Duración de un ciclo de la línea de progreso indeterminada.
const PROGRESS_CYCLE: f32 = 1.4;
/// Periodo de la animación de la línea de progreso (~30 fps).
const PROGRESS_FRAME: Duration = Duration::from_millis(33);

/// Cuánto se muestra un aviso en la línea de estado.
const NOTICE: Duration = Duration::from_secs(4);

struct Graphics {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    // El contexto debe vivir tanto como la superficie.
    _context: softbuffer::Context<Rc<Window>>,
}

/// Una entrada del historial de sesión: la URL, el título y dónde estaba el scroll al salir.
struct Entry {
    url: Url,
    title: String,
    scroll: (f32, f32),
}

/// Qué hacer con el historial cuando termine una carga.
#[derive(Debug, Clone, Copy)]
enum Navigation {
    /// Una navegación nueva: reemplaza las entradas siguientes.
    Push,
    /// Atrás/adelante a la entrada `i`.
    Traverse(usize),
    Reload,
}

/// Una carga en curso.
struct Loading {
    id: u64,
    nav: Navigation,
    url: Url,
    started: Instant,
}

/// Dónde está el puntero.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Area {
    Bar,
    Status,
    Panel,
    Page,
}

/// El estado propio de una pestaña. La pestaña activa vive en los campos de [`App`]; las
/// demás se guardan aquí y se intercambian al cambiar de pestaña (`swap_state`).
#[derive(Default)]
struct TabState {
    current: Option<Arc<Loaded>>,
    page: Option<Page>,
    scroll: (f32, f32),
    history: Vec<Entry>,
    index: usize,
    loading: Option<Loading>,
    relayout: Option<u64>,
    hover_link: Option<String>,
    overlay_hover: Option<(Rect, bool, String)>,
    page_cache: RefCell<Option<(RasterKey, Pixmap)>>,
    page_generation: u64,
    last_layout_request: Option<Size>,
    load_time: Option<Duration>,
    raster: Cell<Option<Duration>>,
    panel_raster: Option<Duration>,
    focus: Option<Focus>,
}

struct App {
    net: Arc<NetLoader>,
    /// Estado de las pestañas inactivas (`None` en la posición de la activa).
    tabs: Vec<Option<TabState>>,
    active: usize,
    /// Control de la página con el foco de teclado (campo de texto en edición).
    focus: Option<Focus>,
    /// Menú contextual abierto.
    menu: Option<Menu>,
    fonts: Arc<FontDb>,
    worker: Worker,
    current: Option<Arc<Loaded>>,
    page: Option<Page>,
    scroll: (f32, f32),
    history: Vec<Entry>,
    index: usize,
    next_id: u64,
    loading: Option<Loading>,
    /// Re-layout en curso (id).
    relayout: Option<u64>,
    chrome: Chrome,
    zoom: f32,
    modifiers: ModifiersState,
    /// Posición del puntero en px lógicos de la ventana.
    mouse: (f32, f32),
    hover_link: Option<String>,
    cursor: CursorIcon,
    graphics: Option<Graphics>,
    /// Tamaño lógico cuando no hay ventana (sesiones sin ventana de [`session`]).
    headless: Size,
    initial: Option<Url>,
    /// Superposición de depuración activa, y el fragmento bajo el puntero (rect, si es fijo y
    /// su descripción).
    overlay: bool,
    overlay_hover: Option<(Rect, bool, String)>,
    /// Último tiempo de raster de la página y el que muestra el panel.
    raster: Cell<Option<Duration>>,
    panel_raster: Option<Duration>,
    /// Raster de la página en caché; se reutiliza mientras no cambien página, scroll, zoom,
    /// DPI ni viewport, así mover el ratón o animar la barra no vuelve a pintar la página.
    page_cache: RefCell<Option<(RasterKey, Pixmap)>>,
    page_generation: u64,
    panel_cache: RefCell<Option<(RasterKey, Pixmap)>>,
    panel_generation: u64,
    /// Último viewport con el que se pidió layout al hilo de trabajo.
    last_layout_request: Option<Size>,
    /// Al terminar la carga en curso, dejar el campo de URL enfocado y vacío (pestaña nueva).
    focus_bar_on_load: bool,
    /// Último cuadro de la animación de progreso: limita el redibujado a ~30 fps mientras
    /// carga (cada `request_redraw` produce un evento inmediato, no espera al temporizador).
    last_progress_frame: Instant,
    /// Tiempo total de la última carga (pedido → página lista).
    load_time: Option<Duration>,
    /// Aviso temporal en la línea de estado ("Guardado …").
    notice: Option<(String, Instant)>,
    clock: Instant,
}

/// Interpreta lo escrito en el campo de URL: una URL, una ruta local, un host (se antepone
/// `https://`) o, si no parece una dirección, una búsqueda en Google.
fn url_from_input(input: &str) -> Option<Url> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let path_like = s.starts_with(['/', '\\', '.', '~'])
        || (s.len() >= 2 && s.as_bytes()[1] == b':' && s.as_bytes()[0].is_ascii_alphabetic())
        || std::path::Path::new(s).exists();
    if path_like {
        return bf_infra::url_from_arg(s);
    }
    let has_scheme = s.split_once(':').is_some_and(|(scheme, rest)| {
        matches!(
            scheme.to_ascii_lowercase().as_str(),
            "http" | "https" | "file" | "data" | "about"
        ) || rest.starts_with("//")
    });
    if has_scheme && let Ok(u) = Url::parse(s, None) {
        return Some(u);
    }
    let host = s.split(['/', '?', '#']).next().unwrap_or("");
    let looks_like_host = !s.contains(char::is_whitespace)
        && (host.contains('.') || host.starts_with("localhost") || host.contains(':'));
    if looks_like_host {
        return Url::parse(&format!("https://{s}"), None).ok();
    }
    let query = bf_url::form::serialize(&[("q".to_owned(), s.to_owned())]);
    Url::parse(&format!("https://www.google.com/search?{query}"), None).ok()
}

/// ¿`a` y `b` son el mismo documento salvo el fragmento?
fn same_document(a: &Url, b: &Url) -> bool {
    a.serialize(true) == b.serialize(true)
}

/// Fecha y hora UTC `AAAAMMDD-HHMMSS` (sin dependencias: días desde la época civil).
fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant, `civil_from_days`.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Clave de una capa rasterizada en caché. Si no cambia, el pixmap anterior sirve tal cual.
#[derive(Clone, Copy, PartialEq, Eq)]
struct RasterKey {
    /// Generación del contenido (página o panel): cambia cuando se asigna uno nuevo.
    generation: u64,
    /// Scroll cuantizado a 1/16 px.
    scroll: (i64, i64),
    scale: u32,
    size: (u32, u32),
}

impl RasterKey {
    fn new(generation: u64, scroll: (f32, f32), scale: f32, size: Size) -> Self {
        Self {
            generation,
            scroll: (
                (scroll.0 * 16.0).round() as i64,
                (scroll.1 * 16.0).round() as i64,
            ),
            scale: scale.to_bits(),
            size: (size.width.to_bits(), size.height.to_bits()),
        }
    }
}

/// El icono de la ventana: el logo a 64 px.
fn window_icon() -> Option<winit::window::Icon> {
    let img = bf_image::decode(internal::LOGO_64, Some("image/png")).ok()?;
    winit::window::Icon::from_rgba(img.rgba, img.width, img.height).ok()
}

/// Dos viewports se consideran iguales si difieren menos de medio px CSS: evita que un
/// error de redondeo encadene re-layouts sin fin.
fn same_viewport(a: Size, b: Size) -> bool {
    (a.width - b.width).abs() <= 0.5 && (a.height - b.height).abs() <= 0.5
}

/// Trazas de eventos de la ventana en stderr, activadas con `BF_TRACE=1` (diagnóstico).
fn trace(msg: &str) {
    if std::env::var_os("BF_TRACE").is_some() {
        eprintln!(
            "[{:>8.3}] {msg}",
            TRACE_START
                .get_or_init(Instant::now)
                .elapsed()
                .as_secs_f32()
        );
    }
}

static TRACE_START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// El `<form>` dueño de `node`: el del atributo `form` o el ancestro más cercano.
fn form_of(doc: &bf_dom::Document, node: NodeId) -> Option<NodeId> {
    if let Some(id) = doc.element(node).and_then(|e| e.attr("form"))
        && let Some(f) = doc
            .inclusive_descendants(doc.root())
            .into_iter()
            .find(|&n| {
                doc.element(n)
                    .is_some_and(|e| e.is_html("form") && e.attr("id") == Some(id))
            })
    {
        return Some(f);
    }
    let mut cur = doc.parent(node);
    while let Some(n) = cur {
        if doc.element(n).is_some_and(|e| e.is_html("form")) {
            return Some(n);
        }
        cur = doc.parent(n);
    }
    None
}

/// El botón de envío que contiene a `node`, si lo hay.
fn submit_button_at(doc: &bf_dom::Document, node: NodeId) -> Option<NodeId> {
    let mut cur = Some(node);
    while let Some(n) = cur {
        if let Some(el) = doc.element(n)
            && el.ns == bf_infra::Namespace::Html
        {
            let ty = el.attr("type").unwrap_or("").to_ascii_lowercase();
            match el.name.as_str() {
                "input" if matches!(ty.as_str(), "submit" | "image") => return Some(n),
                "button" if matches!(ty.as_str(), "" | "submit") => return Some(n),
                "a" | "input" | "button" | "textarea" | "select" | "label" => return None,
                _ => {}
            }
        }
        cur = doc.parent(n);
    }
    None
}

/// El conjunto de datos de un formulario (HTML §4.10.21.4, sin archivos).
fn form_data(
    doc: &bf_dom::Document,
    form: NodeId,
    submitter: Option<NodeId>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for n in doc.inclusive_descendants(form) {
        let Some(el) = doc.element(n) else { continue };
        if el.ns != bf_infra::Namespace::Html || el.attr("disabled").is_some() {
            continue;
        }
        let Some(name) = el.attr("name").filter(|s| !s.is_empty()) else {
            continue;
        };
        let pair = |v: &str| (name.to_owned(), v.to_owned());
        match el.name.as_str() {
            "input" => {
                let ty = el.attr("type").unwrap_or("").to_ascii_lowercase();
                match ty.as_str() {
                    "checkbox" | "radio" => {
                        if el.attr("checked").is_some() {
                            out.push(pair(el.attr("value").unwrap_or("on")));
                        }
                    }
                    "submit" | "image" | "reset" | "button" => {
                        if submitter == Some(n) {
                            out.push(pair(el.attr("value").unwrap_or("")));
                        }
                    }
                    "file" => {}
                    _ => out.push(pair(el.attr("value").unwrap_or(""))),
                }
            }
            "textarea" => out.push(pair(&doc.text_content(n))),
            "select" => {
                let (mut selected, mut first) = (None, None);
                for o in doc.inclusive_descendants(n) {
                    let Some(oe) = doc.element(o) else { continue };
                    if !oe.is_html("option") {
                        continue;
                    }
                    let v = oe
                        .attr("value")
                        .map(str::to_owned)
                        .unwrap_or_else(|| doc.text_content(o).trim().to_owned());
                    if first.is_none() {
                        first = Some(v.clone());
                    }
                    if oe.attr("selected").is_some() {
                        selected = Some(v);
                    }
                }
                if let Some(v) = selected.or(first) {
                    out.push(pair(&v));
                }
            }
            "button" if submitter == Some(n) => {
                out.push(pair(el.attr("value").unwrap_or("")));
            }
            _ => {}
        }
    }
    out
}

/// Extensión de archivo para un `Content-Type` de imagen.
fn extension_for(content_type: Option<&str>) -> &'static str {
    match content_type
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
    {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "image/x-icon" | "image/vnd.microsoft.icon" => "ico",
        _ => "bin",
    }
}

fn clipboard_text() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

fn set_clipboard(text: &str) -> bool {
    arboard::Clipboard::new()
        .and_then(|mut c| c.set_text(text.to_owned()))
        .is_ok()
}

fn blit(frame: &mut Pixmap, part: &Pixmap, x: f32, y: f32) {
    frame.draw_pixmap(
        x.round() as i32,
        y.round() as i32,
        part.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
}

impl App {
    fn new(worker: Worker, fonts: Arc<FontDb>, initial: Option<Url>, theme: Theme) -> App {
        App {
            net: Arc::new(NetLoader::new()),
            tabs: vec![None],
            active: 0,
            focus: None,
            menu: None,
            fonts: fonts.clone(),
            worker,
            current: None,
            page: None,
            scroll: (0.0, 0.0),
            history: Vec::new(),
            index: 0,
            next_id: 0,
            loading: None,
            relayout: None,
            chrome: Chrome::new(fonts, theme),
            zoom: 1.0,
            modifiers: ModifiersState::empty(),
            mouse: (0.0, 0.0),
            hover_link: None,
            cursor: CursorIcon::Default,
            graphics: None,
            headless: Size {
                width: 1024.0,
                height: 768.0,
            },
            initial,
            overlay: false,
            overlay_hover: None,
            raster: Cell::new(None),
            panel_raster: None,
            page_cache: RefCell::new(None),
            page_generation: 0,
            panel_cache: RefCell::new(None),
            panel_generation: 0,
            last_layout_request: None,
            focus_bar_on_load: false,
            last_progress_frame: Instant::now(),
            load_time: None,
            notice: None,
            clock: Instant::now(),
        }
    }

    fn dpi(&self) -> f32 {
        self.graphics
            .as_ref()
            .map_or(1.0, |g| g.window.scale_factor() as f32)
    }

    /// Tamaño lógico de la ventana.
    fn logical_size(&self) -> Size {
        let Some(g) = &self.graphics else {
            return self.headless;
        };
        let l = g
            .window
            .inner_size()
            .to_logical::<f32>(g.window.scale_factor());
        Size {
            width: l.width.max(1.0),
            height: l.height.max(1.0),
        }
    }

    /// El área de la página en px lógicos de la ventana: entre la barra y la línea de estado,
    /// sin el panel.
    fn page_area(&self) -> Rect {
        let s = self.logical_size();
        let panel = if self.chrome.panel_open { PANEL } else { 0.0 };
        Rect::new(
            0.0,
            BAR,
            (s.width - panel).max(1.0),
            (s.height - BAR - STATUS).max(1.0),
        )
    }

    /// Viewport de la página en px CSS.
    fn viewport(&self) -> Size {
        let a = self.page_area();
        Size {
            width: (a.width / self.zoom).max(1.0),
            height: (a.height / self.zoom).max(1.0),
        }
    }

    fn area_at(&self, x: f32, y: f32) -> Area {
        let s = self.logical_size();
        if y < BAR {
            Area::Bar
        } else if y >= s.height - STATUS {
            Area::Status
        } else if self.chrome.panel_open && x >= self.page_area().right() {
            Area::Panel
        } else {
            Area::Page
        }
    }

    /// Coordenadas del viewport de la página (px CSS) para un punto de la ventana.
    fn page_point(&self, x: f32, y: f32) -> (f32, f32) {
        (x / self.zoom, (y - BAR) / self.zoom)
    }

    fn redraw_later(&self) {
        if let Some(g) = &self.graphics {
            g.window.request_redraw();
        }
    }

    fn display_name(&self) -> String {
        if self.loading.is_some() {
            return "Cargando…".to_owned();
        }
        self.current.as_ref().map_or_else(String::new, |l| {
            l.title().unwrap_or_else(|| match &l.url.path {
                bf_url::Path::List(segments) if l.url.scheme == "file" => {
                    segments.last().cloned().unwrap_or_default()
                }
                _ if l.url.scheme == "about" => l.url.href(),
                _ => l.url.host_string(),
            })
        })
    }

    /// Intercambia el estado de la pestaña activa con `t`.
    fn swap_state(&mut self, t: &mut TabState) {
        use std::mem::swap;
        swap(&mut self.current, &mut t.current);
        swap(&mut self.page, &mut t.page);
        swap(&mut self.scroll, &mut t.scroll);
        swap(&mut self.history, &mut t.history);
        swap(&mut self.index, &mut t.index);
        swap(&mut self.loading, &mut t.loading);
        swap(&mut self.relayout, &mut t.relayout);
        swap(&mut self.hover_link, &mut t.hover_link);
        swap(&mut self.overlay_hover, &mut t.overlay_hover);
        swap(&mut self.page_cache, &mut t.page_cache);
        swap(&mut self.page_generation, &mut t.page_generation);
        swap(&mut self.last_layout_request, &mut t.last_layout_request);
        swap(&mut self.load_time, &mut t.load_time);
        swap(&mut self.raster, &mut t.raster);
        swap(&mut self.panel_raster, &mut t.panel_raster);
        swap(&mut self.focus, &mut t.focus);
    }

    /// Ejecuta `f` con la pestaña `i` como activa (sin cambiar la pestaña visible).
    fn with_tab<R>(&mut self, i: usize, f: impl FnOnce(&mut App) -> R) -> R {
        if i == self.active || i >= self.tabs.len() {
            return f(self);
        }
        let Some(mut state) = self.tabs[i].take() else {
            return f(self);
        };
        let focused = self.chrome.field.focused;
        self.swap_state(&mut state);
        let r = f(self);
        self.swap_state(&mut state);
        self.tabs[i] = Some(state);
        self.chrome.field.focused = focused;
        self.update_title();
        self.refresh_chrome();
        r
    }

    /// Título de la pestaña `i` para la franja.
    fn tab_title(&self, i: usize) -> String {
        if i == self.active {
            return self.display_name();
        }
        let Some(t) = self.tabs.get(i).and_then(|t| t.as_ref()) else {
            return String::new();
        };
        if t.loading.is_some() {
            return "Cargando…".to_owned();
        }
        t.current
            .as_ref()
            .and_then(|l| l.title())
            .or_else(|| t.history.get(t.index).map(|e| e.url.href()))
            .unwrap_or_default()
    }

    fn tab_loading(&self, i: usize) -> bool {
        if i == self.active {
            return self.loading.is_some();
        }
        self.tabs
            .get(i)
            .and_then(|t| t.as_ref())
            .is_some_and(|t| t.loading.is_some())
    }

    fn any_loading(&self) -> bool {
        (0..self.tabs.len()).any(|i| self.tab_loading(i))
    }

    /// La pestaña que espera el trabajo `id` del hilo de carga.
    fn tab_owning(&self, id: u64) -> Option<usize> {
        let owns = |loading: &Option<Loading>, relayout: Option<u64>| {
            loading.as_ref().is_some_and(|l| l.id == id) || relayout == Some(id)
        };
        if owns(&self.loading, self.relayout) {
            return Some(self.active);
        }
        self.tabs
            .iter()
            .position(|t| t.as_ref().is_some_and(|t| owns(&t.loading, t.relayout)))
    }

    /// Abre una pestaña a la derecha de la activa. Sin `url`, `about:home` con el campo de
    /// URL enfocado y vacío; `background` la carga sin mostrarla.
    fn new_tab(&mut self, url: Option<Url>, background: bool) {
        let i = (self.active + 1).min(self.tabs.len());
        self.tabs.insert(i, Some(TabState::default()));
        let home = url.is_none();
        let url = url.or_else(|| Url::parse("about:home", None).ok());
        if background {
            if let Some(u) = url {
                self.with_tab(i, |app| app.start_load(u, Navigation::Push));
            }
        } else {
            self.activate_tab(i);
            if let Some(u) = url {
                self.start_load(u, Navigation::Push);
            }
            if home {
                self.focus_bar_on_load = true;
                self.chrome.field.focused = true;
                self.chrome.field.set_text(String::new());
            }
        }
        self.update_title();
        self.refresh_chrome();
    }

    /// Muestra la pestaña `i`.
    fn activate_tab(&mut self, i: usize) {
        if i >= self.tabs.len() || i == self.active {
            return;
        }
        let Some(mut incoming) = self.tabs[i].take() else {
            return;
        };
        self.swap_state(&mut incoming);
        self.tabs[self.active] = Some(incoming);
        self.active = i;
        self.chrome.field.focused = false;
        self.update_title();
        self.refresh_chrome();
        self.refresh_panel();
        self.redraw_later();
    }

    /// Cierra la pestaña `i`. La última se sustituye por una pestaña nueva.
    fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        if self.tabs.len() == 1 {
            self.new_tab(None, false);
            return self.close_tab(0);
        }
        if i == self.active {
            let next = if i + 1 < self.tabs.len() {
                i + 1
            } else {
                i - 1
            };
            self.activate_tab(next);
        }
        self.tabs.remove(i);
        if self.active > i {
            self.active -= 1;
        }
        self.update_title();
        self.refresh_chrome();
    }

    /// Pasa a la pestaña `delta` posiciones a la derecha (cíclico).
    fn cycle_tab(&mut self, delta: isize) {
        let n = self.tabs.len() as isize;
        let i = (self.active as isize + delta).rem_euclid(n) as usize;
        self.activate_tab(i);
    }

    /// Teclas con un campo de la página enfocado. Devuelve si la tecla se consumió.
    fn edit_key(&mut self, key: &Key, text: Option<&str>) -> bool {
        let (Some(f), Some(current)) = (self.focus.clone(), self.current.clone()) else {
            return false;
        };
        let ctrl = self.modifiers.control_key();
        let shift = self.modifiers.shift_key();
        let is = |c: &str, k: &str| c.eq_ignore_ascii_case(k);
        let mut value: Vec<char> = edit::value(&current.doc, f.node, f.kind).chars().collect();
        let len = value.len();
        let mut caret = f.caret.min(len);
        let mut anchor = f.anchor.map(|a| a.min(len));
        let sel = f.selection().map(|(a, b)| (a.min(len), b.min(len)));
        let mut changed = false;
        let kind = f.kind;
        // Borra la selección (si hay) y deja el caret en su inicio.
        let delete_sel = |value: &mut Vec<char>, caret: &mut usize| -> bool {
            match sel {
                Some((a, b)) if a < b => {
                    value.drain(a..b);
                    *caret = a;
                    true
                }
                _ => false,
            }
        };
        let insert = |value: &mut Vec<char>, caret: &mut usize, s: &str, changed: &mut bool| {
            delete_sel(value, caret);
            for c in s.chars() {
                if kind == Editable::Input && (c == char::from(10) || c == char::from(13)) {
                    continue;
                }
                value.insert(*caret, c);
                *caret += 1;
            }
            *changed = true;
        };
        // Mover el caret: con Mayús extiende la selección.
        let moved = |caret: &mut usize, anchor: &mut Option<usize>, to: usize| {
            if shift {
                if anchor.is_none() {
                    *anchor = Some(*caret);
                }
            } else {
                *anchor = None;
            }
            *caret = to;
        };
        match key {
            Key::Named(NamedKey::Escape) => {
                self.focus = None;
                self.redraw_later();
                return true;
            }
            Key::Named(NamedKey::Tab) => return false,
            Key::Named(NamedKey::Enter) if kind == Editable::Input => {
                // Envío implícito (HTML §4.10.21.2).
                self.focus = None;
                if let Some(form) = form_of(&current.doc, f.node) {
                    self.submit_form(form, None);
                }
                return true;
            }
            Key::Named(NamedKey::Enter) => {
                let nl = String::from(char::from(10));
                insert(&mut value, &mut caret, &nl, &mut changed);
                anchor = None;
            }
            Key::Named(NamedKey::Backspace) => {
                if !delete_sel(&mut value, &mut caret) && caret > 0 {
                    value.remove(caret - 1);
                    caret -= 1;
                }
                anchor = None;
                changed = true;
            }
            Key::Named(NamedKey::Delete) => {
                if !delete_sel(&mut value, &mut caret) && caret < len {
                    value.remove(caret);
                }
                anchor = None;
                changed = true;
            }
            Key::Named(NamedKey::ArrowLeft) => {
                let to = caret.saturating_sub(1);
                moved(&mut caret, &mut anchor, to);
            }
            Key::Named(NamedKey::ArrowRight) => {
                let to = (caret + 1).min(len);
                moved(&mut caret, &mut anchor, to);
            }
            Key::Named(NamedKey::Home) => moved(&mut caret, &mut anchor, 0),
            Key::Named(NamedKey::End) => moved(&mut caret, &mut anchor, len),
            Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown) => {
                return kind == Editable::TextArea;
            }
            Key::Character(c) if ctrl && is(c, "a") => {
                anchor = Some(0);
                caret = len;
            }
            Key::Character(c) if ctrl && is(c, "c") => {
                if let Some((a, b)) = sel {
                    set_clipboard(&value[a..b].iter().collect::<String>());
                }
                return true;
            }
            Key::Character(c) if ctrl && is(c, "x") => {
                if let Some((a, b)) = sel {
                    set_clipboard(&value[a..b].iter().collect::<String>());
                    delete_sel(&mut value, &mut caret);
                    anchor = None;
                    changed = true;
                }
            }
            Key::Character(c) if ctrl && is(c, "v") => {
                if let Some(t) = clipboard_text() {
                    insert(&mut value, &mut caret, &t, &mut changed);
                    anchor = None;
                }
            }
            Key::Character(_) if ctrl => return false,
            Key::Named(NamedKey::Space) => {
                insert(&mut value, &mut caret, " ", &mut changed);
                anchor = None;
            }
            _ => match text {
                Some(t) if !t.is_empty() && !t.chars().any(char::is_control) => {
                    insert(&mut value, &mut caret, t, &mut changed);
                    anchor = None;
                }
                _ => return false,
            },
        }
        if changed {
            self.set_field_value(f.node, kind, value.iter().collect());
        }
        if let Some(fo) = &mut self.focus {
            fo.caret = caret;
            fo.anchor = anchor;
        }
        self.redraw_later();
        true
    }

    /// Escribe el valor de un control en el DOM y vuelve a maquetar.
    fn set_field_value(&mut self, node: NodeId, kind: Editable, text: String) {
        let Some(cur) = &self.current else {
            return;
        };
        let mut l: Loaded = (**cur).clone();
        edit::set_value(&mut l.doc, node, kind, &text);
        self.current = Some(Arc::new(l));
        self.request_relayout();
    }

    /// Envía el formulario del botón `button`.
    fn submit(&mut self, button: NodeId) {
        let Some(cur) = self.current.clone() else {
            return;
        };
        if let Some(form) = form_of(&cur.doc, button) {
            self.submit_form(form, Some(button));
        }
    }

    /// Envío de un formulario (HTML §4.10.21.3, GET y POST `urlencoded`).
    fn submit_form(&mut self, form: NodeId, submitter: Option<NodeId>) {
        let Some(cur) = self.current.clone() else {
            return;
        };
        let doc = &cur.doc;
        let attr = |name: &str| -> Option<String> {
            submitter
                .and_then(|s| doc.element(s))
                .and_then(|e| e.attr(&format!("form{name}")).map(str::to_owned))
                .or_else(|| {
                    doc.element(form)
                        .and_then(|e| e.attr(name).map(str::to_owned))
                })
        };
        let method = attr("method").unwrap_or_default().to_ascii_lowercase();
        let action = attr("action")
            .filter(|a| !a.trim().is_empty())
            .and_then(|a| Url::parse(a.trim(), Some(&cur.base)).ok())
            .unwrap_or_else(|| cur.url.clone());
        let data = form_data(doc, form, submitter);
        let query = bf_url::form::serialize(&data);
        if method == "post" {
            let body = (
                query.into_bytes(),
                "application/x-www-form-urlencoded".to_owned(),
            );
            self.start_load_with(action, Navigation::Push, Some(body));
        } else {
            let mut url = action;
            url.query = Some(query);
            url.fragment = None;
            self.navigate(url);
        }
    }

    /// La URL de la imagen (`<img>`) bajo el puntero.
    fn image_under_mouse(&self) -> Option<String> {
        let (x, y) = self.mouse;
        let (px, py) = self.page_point(x, y);
        let page = self.page.as_ref()?;
        let node = hit::hit_test(&page.tree, px, py, self.scroll)?.node?;
        page.images.element_url.get(&node).cloned()
    }

    /// Clic derecho en la página: el menú contextual según lo que hay bajo el puntero.
    fn context_menu(&mut self) {
        let (x, y) = self.mouse;
        if self.area_at(x, y) != Area::Page {
            return;
        }
        let link = self.link_under_mouse().map(|u| u.href());
        let image = self.image_under_mouse();
        let field = match (&self.current, &self.page) {
            (Some(current), Some(page)) => {
                let (px, py) = self.page_point(x, y);
                hit::hit_test(&page.tree, px, py, self.scroll)
                    .and_then(|f| f.node)
                    .and_then(|n| edit::editable_at(&current.doc, n))
            }
            _ => None,
        };
        let mut items = Vec::new();
        if let Some(l) = &link {
            items.push(MenuItem::new(
                "Abrir enlace en una pestaña nueva",
                MenuAction::OpenInTab(l.clone()),
            ));
            items.push(MenuItem::new(
                "Copiar dirección del enlace",
                MenuAction::CopyText(l.clone()),
            ));
            items.push(MenuItem::separator());
        }
        if let Some(i) = &image {
            items.push(MenuItem::new(
                "Guardar imagen como…",
                MenuAction::SaveResource(i.clone()),
            ));
            items.push(MenuItem::new(
                "Copiar dirección de la imagen",
                MenuAction::CopyText(i.clone()),
            ));
            items.push(MenuItem::new(
                "Abrir imagen en una pestaña nueva",
                MenuAction::OpenInTab(i.clone()),
            ));
            items.push(MenuItem::separator());
        }
        if let Some((node, kind)) = field {
            if self.focus.as_ref().map(|f| f.node) != Some(node) {
                let len = self
                    .current
                    .as_ref()
                    .map_or(0, |c| edit::value(&c.doc, node, kind).chars().count());
                self.focus = Some(Focus {
                    node,
                    kind,
                    caret: len,
                    anchor: None,
                });
            }
            let has_sel = self.focus.as_ref().is_some_and(|f| f.selection().is_some());
            items.push(MenuItem::new("Cortar", MenuAction::Cut).disabled(!has_sel));
            items.push(MenuItem::new("Copiar", MenuAction::Copy).disabled(!has_sel));
            let paste = clipboard_text().is_some_and(|t| !t.is_empty());
            items.push(MenuItem::new("Pegar", MenuAction::Paste).disabled(!paste));
            items.push(MenuItem::new("Seleccionar todo", MenuAction::SelectAll));
            items.push(MenuItem::separator());
        }
        items.push(MenuItem::new("Atrás", MenuAction::Back).disabled(!self.chrome.can_back));
        items.push(
            MenuItem::new("Adelante", MenuAction::Forward).disabled(!self.chrome.can_forward),
        );
        items.push(MenuItem::new("Recargar", MenuAction::Reload));
        items.push(MenuItem::separator());
        let page_url = self.current.as_ref().map(|c| c.url.href());
        let no_page = page_url.is_none();
        items.push(MenuItem::new("Guardar página como…", MenuAction::SavePage).disabled(no_page));
        items.push(
            MenuItem::new(
                "Copiar dirección de la página",
                MenuAction::CopyText(page_url.unwrap_or_default()),
            )
            .disabled(no_page),
        );
        items.push(MenuItem::new("Ver código fuente", MenuAction::ViewSource).disabled(no_page));
        items.push(MenuItem::new("Inspeccionar (F12)", MenuAction::Inspect));
        self.menu = Some(Menu::new(
            items,
            (x, y),
            self.logical_size(),
            self.fonts.clone(),
            self.chrome.theme,
        ));
        self.redraw_later();
    }

    /// Ejecuta un atajo de edición (`x`, `c`, `v`, `a`) sobre el campo enfocado.
    fn edit_shortcut(&mut self, letter: &str) {
        let saved = self.modifiers;
        self.modifiers = ModifiersState::CONTROL;
        self.edit_key(&Key::Character(SmolStr::new(letter)), None);
        self.modifiers = saved;
    }

    fn menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::Back => self.back(),
            MenuAction::Forward => self.forward(),
            MenuAction::Reload => self.reload(),
            MenuAction::OpenInTab(u) => {
                if let Ok(u) = Url::parse(&u, None) {
                    self.new_tab(Some(u), true);
                }
            }
            MenuAction::CopyText(t) => {
                let message = if set_clipboard(&t) {
                    "Copiado"
                } else {
                    "No se pudo copiar"
                };
                self.set_notice(message.to_owned());
            }
            MenuAction::SaveResource(u) => self.save_resource(&u),
            MenuAction::SavePage => self.save_page(),
            MenuAction::ViewSource => {
                if let Some(c) = &self.current
                    && let Ok(u) = Url::parse(&format!("about:source?{}", c.url.href()), None)
                {
                    self.new_tab(Some(u), false);
                }
            }
            MenuAction::Inspect => self.toggle_overlay(),
            MenuAction::Cut => self.edit_shortcut("x"),
            MenuAction::Copy => self.edit_shortcut("c"),
            MenuAction::Paste => self.edit_shortcut("v"),
            MenuAction::SelectAll => self.edit_shortcut("a"),
        }
    }

    /// Descarga `url` y la guarda donde el usuario elija.
    fn save_resource(&mut self, url: &str) {
        let Ok(u) = Url::parse(url, None) else {
            return;
        };
        let Some(res) = bf_infra::ResourceLoader::load(&*self.net, &u) else {
            self.set_notice("No se pudo descargar la imagen".to_owned());
            return;
        };
        let name = u
            .path_string()
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("imagen")
            .to_owned();
        let name = if name.contains('.') {
            name
        } else {
            format!("{name}.{}", extension_for(res.content_type.as_deref()))
        };
        self.save_bytes(&name, &res.bytes);
    }

    /// Guarda el HTML original de la página donde el usuario elija.
    fn save_page(&mut self) {
        let Some(cur) = self.current.clone() else {
            return;
        };
        let Some(res) = bf_infra::ResourceLoader::load(&*cur.loader, &cur.url) else {
            self.set_notice("No se pudo obtener la página".to_owned());
            return;
        };
        let mut name = cur.title().unwrap_or_else(|| cur.url.host_string());
        name.retain(|c| {
            !matches!(c, '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') && c != char::from(92)
        });
        let name = format!("{}.html", name.trim());
        self.save_bytes(&name, &res.bytes);
    }

    fn save_bytes(&mut self, name: &str, bytes: &[u8]) {
        let Some(path) = rfd::FileDialog::new().set_file_name(name).save_file() else {
            return;
        };
        let message = match std::fs::write(&path, bytes) {
            Ok(()) => format!("Guardado {}", path.display()),
            Err(e) => format!("No se pudo guardar {}: {e}", path.display()),
        };
        self.set_notice(message);
    }

    fn update_title(&self) {
        if let Some(g) = &self.graphics {
            g.window
                .set_title(&format!("«{}» — browser-freedom", self.display_name()));
        }
    }

    /// Vuelve a maquetar la barra y la línea de estado con el estado actual.
    fn refresh_chrome(&mut self) {
        let vp = self.viewport();
        let tabs: Vec<TabInfo> = (0..self.tabs.len())
            .map(|i| TabInfo {
                title: self.tab_title(i),
                loading: self.tab_loading(i),
            })
            .collect();
        let c = &mut self.chrome;
        c.tabs = tabs;
        c.active = self.active;
        c.can_back = self.index > 0 && !self.history.is_empty();
        c.can_forward = self.index + 1 < self.history.len();
        c.loading = self.loading.is_some();
        c.engine_alert = self
            .page
            .as_ref()
            .is_some_and(|p| p.report.has_unsupported());
        if !c.field.focused {
            c.url = match &self.loading {
                Some(l) => l.url.href(),
                None => self
                    .history
                    .get(self.index)
                    .map(|e| e.url.href())
                    .or_else(|| self.current.as_ref().map(|l| l.url.href()))
                    .unwrap_or_default(),
            };
            c.field.set_text(c.url.clone());
        }
        // Línea de estado.
        let notice = self
            .notice
            .as_ref()
            .filter(|(_, at)| at.elapsed() < NOTICE)
            .map(|(m, _)| m.clone());
        c.status_em = notice.is_some();
        c.status_left = notice
            .or_else(|| self.overlay_hover.as_ref().map(|h| h.2.clone()))
            .or_else(|| self.hover_link.clone())
            .unwrap_or_else(|| {
                if self.loading.is_some() {
                    "Cargando…".to_owned()
                } else {
                    String::new()
                }
            });
        let mut right = vec![
            format!("Zoom {:.0} %", self.zoom * 100.0),
            format!("{:.0} × {:.0} px", vp.width, vp.height),
        ];
        if let Some(t) = self.load_time {
            right.push(format!("{:.2} s", t.as_secs_f64()).replace('.', ","));
        }
        if self.overlay {
            right.insert(0, "F12 cajas".to_owned());
        }
        c.status_right = right;
        let width = self.logical_size().width;
        self.chrome.layout(width);
        self.redraw_later();
    }

    /// Vuelve a generar el panel del motor para la página actual.
    fn refresh_panel(&mut self) {
        if !self.chrome.panel_open {
            return;
        }
        let height = self.page_area().height;
        let body = match (&self.current, &self.page) {
            (Some(l), Some(p)) => internal::engine_body(l, p, self.raster.get(), true),
            _ => "<p class=\"muted\">Sin página.</p>".to_owned(),
        };
        self.panel_raster = self.raster.get();
        self.chrome.layout_panel(&body, height);
        self.panel_generation += 1;
        self.redraw_later();
    }

    fn set_notice(&mut self, message: String) {
        self.notice = Some((message, Instant::now()));
        self.refresh_chrome();
    }

    // --- Navegación ---

    fn start_load(&mut self, url: Url, nav: Navigation) {
        self.start_load_with(url, nav, None);
    }

    /// Como [`Self::start_load`], con un cuerpo `POST` `(bytes, Content-Type)`.
    fn start_load_with(&mut self, url: Url, nav: Navigation, post: Option<(Vec<u8>, String)>) {
        // El scroll de la entrada actual, para volver a él.
        if let Some(e) = self.history.get_mut(self.index) {
            e.scroll = self.scroll;
        }
        self.next_id += 1;
        let id = self.next_id;
        let mut recent: Vec<(String, String)> = Vec::new();
        for e in self.history.iter().rev() {
            let href = e.url.href();
            if e.url.scheme != "about" && !recent.iter().any(|(_, h)| *h == href) {
                recent.push((e.title.clone(), href));
            }
        }
        self.worker.set_viewport(self.viewport());
        self.worker.send(Job::Load(Box::new(LoadJob {
            id,
            url: url.clone(),
            net: self.net.clone(),
            recent,
            theme: self.chrome.theme,
            post,
        })));
        self.loading = Some(Loading {
            id,
            nav,
            url,
            started: Instant::now(),
        });
        self.relayout = None;
        self.update_title();
        self.refresh_chrome();
    }

    /// Detiene la carga en curso (su resultado se descarta).
    fn stop(&mut self) {
        if self.loading.take().is_some() {
            self.update_title();
            self.refresh_chrome();
        }
    }

    /// Navega a `url` (un enlace o lo escrito en la barra).
    fn navigate(&mut self, url: Url) {
        if url.scheme == "bf" {
            return self.action(&url.path_string());
        }
        // `about:engine` desde una página: el reporte de esa página.
        if url.scheme == "about"
            && url.path_string() == "engine"
            && url.query.is_none()
            && let Some(current) = &self.current
            && current.url.scheme != "about"
        {
            let target = format!("about:engine?{}", current.url.href());
            if let Ok(u) = Url::parse(&target, None) {
                return self.start_load(u, Navigation::Push);
            }
        }
        if let Some(current) = &self.current
            && url.fragment.is_some()
            && same_document(&current.url, &url)
            && self.loading.is_none()
        {
            // Navegación a un fragmento (HTML §7.4.2.3.3): solo se desplaza.
            if let Some(e) = self.history.get_mut(self.index) {
                e.scroll = self.scroll;
            }
            let title = self.display_name();
            self.history.truncate(self.index + 1);
            self.history.push(Entry {
                url: url.clone(),
                title,
                scroll: (0.0, 0.0),
            });
            self.index = self.history.len() - 1;
            self.scroll_to_fragment(&url);
            self.refresh_chrome();
            return;
        }
        self.start_load(url, Navigation::Push);
    }

    fn traverse(&mut self, to: usize) {
        let Some(entry) = self.history.get(to) else {
            return;
        };
        let url = entry.url.clone();
        let scroll = entry.scroll;
        if let Some(current) = &self.current
            && same_document(&current.url, &url)
            && self.loading.is_none()
        {
            if let Some(e) = self.history.get_mut(self.index) {
                e.scroll = self.scroll;
            }
            self.index = to;
            self.scroll = scroll;
            self.clamp_scroll();
            self.chrome.field.focused = false;
            self.refresh_chrome();
            return;
        }
        self.start_load(url, Navigation::Traverse(to));
    }

    fn back(&mut self) {
        if self.index > 0 {
            self.traverse(self.index - 1);
        }
    }

    fn forward(&mut self) {
        if self.index + 1 < self.history.len() {
            self.traverse(self.index + 1);
        }
    }

    fn reload(&mut self) {
        let url = match (&self.loading, &self.current) {
            (Some(l), _) => l.url.clone(),
            (None, Some(c)) => c.url.clone(),
            _ => return,
        };
        self.start_load(url, Navigation::Reload);
    }

    fn home(&mut self) {
        if let Ok(u) = Url::parse("about:home", None) {
            self.navigate(u);
        }
    }

    fn scroll_to_fragment(&mut self, url: &Url) {
        let (Some(current), Some(page)) = (&self.current, &self.page) else {
            return;
        };
        let fragment = url.fragment.as_deref().unwrap_or("");
        // Fragmento vacío o `top`: el principio del documento (HTML §7.4.6.3).
        let target =
            indicated_element(&current.doc, fragment).and_then(|n| hit::node_rect(&page.tree, n));
        self.scroll = match target {
            Some(r) => (0.0, r.y),
            None if fragment.is_empty() || fragment.eq_ignore_ascii_case("top") => (0.0, 0.0),
            None => self.scroll,
        };
        self.clamp_scroll();
        self.redraw_later();
    }

    /// Acciones de la interfaz (enlaces `bf:` del panel y de `about:home`, y atajos).
    fn action(&mut self, name: &str) {
        match name {
            "focus-url" => self.focus_bar(),
            "save-png" => self.save_png(),
            "dump-layout" => self.dump(true),
            "dump-dom" => self.dump(false),
            "engine-page" => {
                if let Ok(u) = Url::parse("about:engine", None) {
                    self.navigate(u);
                }
            }
            _ => {}
        }
    }

    /// Ruta junto al ejecutable con el host de la página y la fecha.
    fn output_path(&self, extension: &str) -> std::path::PathBuf {
        let host = self
            .current
            .as_ref()
            .map(|l| {
                let h = l.url.host_string();
                if h.is_empty() {
                    l.url.href().replace([':', '/', '?', '#', '\\'], "-")
                } else {
                    h
                }
            })
            .unwrap_or_else(|| "browser-freedom".to_owned());
        let dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
            .unwrap_or_default();
        dir.join(format!("{host}-{}.{extension}", timestamp()))
    }

    /// `Ctrl+S`: un PNG del cuadro completo (cromo incluido).
    fn save_png(&mut self) {
        let path = self.output_path("png");
        let message = match self.frame().save_png(&path) {
            Ok(()) => format!("Guardado {}", path.display()),
            Err(e) => format!("No se pudo guardar {}: {e}", path.display()),
        };
        self.set_notice(message);
    }

    /// Volcado del árbol de fragmentos (`layout`) o del DOM.
    fn dump(&mut self, layout: bool) {
        let (Some(current), Some(page)) = (&self.current, &self.page) else {
            return;
        };
        let (text, ext) = if layout {
            (bf_layout::dump(&current.doc, &page.tree), "layout.txt")
        } else {
            (
                bf_dom::test_serialize(&current.doc, current.doc.root()),
                "dom.txt",
            )
        };
        let path = self.output_path(ext);
        let message = match std::fs::write(&path, text) {
            Ok(()) => format!("Guardado {}", path.display()),
            Err(e) => format!("No se pudo guardar {}: {e}", path.display()),
        };
        self.set_notice(message);
    }

    /// Resultados del hilo de trabajo.
    /// Entrega un resultado del hilo de carga a la pestaña que lo pidió. Devuelve el viewport
    /// usado si era para la pestaña activa.
    fn deliver(&mut self, done: Done) -> Option<Size> {
        match self.tab_owning(done.id) {
            Some(i) if i != self.active => {
                self.with_tab(i, |app| app.apply(done));
                None
            }
            _ => {
                let viewport = done.viewport;
                self.apply(done);
                Some(viewport)
            }
        }
    }

    fn receive(&mut self) {
        let mut laid_out = None;
        while let Ok(done) = self.worker.done.try_recv() {
            if let Some(v) = self.deliver(done) {
                laid_out = Some(v);
            }
        }
        // Si la ventana cambió mientras se maquetaba, se rehace con el viewport actual.
        let viewport = self.viewport();
        if laid_out.is_some_and(|v| !same_viewport(v, viewport))
            && self.loading.is_none()
            && self.relayout.is_none()
            && self
                .last_layout_request
                .is_none_or(|r| !same_viewport(r, viewport))
        {
            self.request_relayout();
        }
    }

    fn apply(&mut self, done: Done) {
        trace(&format!(
            "done id={} viewport={:?} relayout={:?} loading={:?}",
            done.id,
            done.viewport,
            self.relayout,
            self.loading.as_ref().map(|l| l.id)
        ));
        let Done {
            id, loaded, page, ..
        } = done;
        if self.relayout == Some(id) {
            self.relayout = None;
            self.page = Some(page);
            self.page_generation += 1;
            self.raster.set(None);
            self.clamp_scroll();
            self.refresh_panel();
            self.redraw_later();
            return;
        }
        let Some(loading) = &self.loading else {
            return;
        };
        if loading.id != id {
            return;
        }
        let (nav, started) = (loading.nav, loading.started);
        self.loading = None;
        self.load_time = Some(started.elapsed());
        let url = loaded.url.clone();
        let title = loaded.title().unwrap_or_else(|| url.href());
        self.current = Some(loaded);
        self.page = Some(page);
        self.page_generation += 1;
        self.raster.set(None);
        self.hover_link = None;
        self.overlay_hover = None;
        self.focus = None;
        match nav {
            Navigation::Push => {
                self.history.truncate(self.index + 1);
                if self.history.is_empty() {
                    self.index = 0;
                } else {
                    self.index += 1;
                }
                self.history.push(Entry {
                    url: url.clone(),
                    title,
                    scroll: (0.0, 0.0),
                });
                self.scroll = (0.0, 0.0);
                if url.fragment.is_some() {
                    self.scroll_to_fragment(&url);
                }
            }
            Navigation::Traverse(i) => {
                self.index = i;
                self.scroll = self.history[i].scroll;
                // La URL final (después de redirecciones).
                self.history[i].url = url;
                self.history[i].title = title;
            }
            Navigation::Reload => {}
        }
        self.clamp_scroll();
        if std::mem::take(&mut self.focus_bar_on_load) {
            self.chrome.field.focused = true;
            self.chrome.field.set_text(String::new());
        } else {
            self.chrome.field.focused = false;
        }
        self.update_title();
        self.refresh_chrome();
        self.refresh_panel();
    }

    fn request_relayout(&mut self) {
        let Some(loaded) = self.current.clone() else {
            return;
        };
        self.next_id += 1;
        let id = self.next_id;
        let viewport = self.viewport();
        trace(&format!("request_relayout id={id} viewport={viewport:?}"));
        self.worker.set_viewport(viewport);
        self.last_layout_request = Some(viewport);
        self.worker.send(Job::Relayout { id, loaded });
        self.relayout = Some(id);
    }

    /// El viewport cambió (ventana, panel o zoom): se rehace el layout (en el hilo) y el cromo.
    fn resized(&mut self) {
        trace(&format!(
            "resized viewport={:?} loading={}",
            self.viewport(),
            self.loading.is_some()
        ));
        if self.loading.is_some() {
            // La carga en curso lee el viewport nuevo antes del layout (o se rehace al
            // terminar).
            self.worker.set_viewport(self.viewport());
        } else {
            self.request_relayout();
        }
        self.refresh_chrome();
        self.refresh_panel();
    }

    fn toggle_panel(&mut self) {
        self.chrome.panel_open = !self.chrome.panel_open;
        self.chrome.panel_scroll = 0.0;
        self.resized();
    }

    fn toggle_overlay(&mut self) {
        self.overlay = !self.overlay;
        self.overlay_hover = None;
        self.mouse_moved();
        self.refresh_chrome();
    }

    // --- Scroll ---

    fn clamp_scroll(&mut self) {
        let Some(page) = &self.page else {
            return;
        };
        let viewport = self.viewport();
        let extent = document_extent(&page.tree);
        let max_x = (extent.right() - viewport.width).max(0.0);
        let max_y = (extent.bottom() - viewport.height).max(0.0);
        self.scroll.0 = self.scroll.0.clamp(0.0, max_x);
        self.scroll.1 = self.scroll.1.clamp(0.0, max_y);
    }

    fn scroll_by(&mut self, dx: f32, dy: f32) {
        let before = self.scroll;
        self.scroll.0 += dx;
        self.scroll.1 += dy;
        self.clamp_scroll();
        if self.scroll != before {
            self.redraw_later();
        }
    }

    fn set_zoom(&mut self, zoom: f32) {
        if (zoom - self.zoom).abs() < f32::EPSILON {
            return;
        }
        // Se conserva la posición relativa del scroll.
        let k = self.zoom / zoom;
        self.zoom = zoom;
        self.scroll = (self.scroll.0 * k, self.scroll.1 * k);
        self.resized();
    }

    fn zoom_step(&mut self, up: bool) {
        let i = ZOOM_LEVELS
            .iter()
            .position(|&z| z >= self.zoom - 0.001)
            .unwrap_or(ZOOM_LEVELS.len() - 1);
        let next = if up {
            ZOOM_LEVELS[(i + 1).min(ZOOM_LEVELS.len() - 1)]
        } else {
            ZOOM_LEVELS[i.saturating_sub(1)]
        };
        self.set_zoom(next);
    }

    // --- Pintura ---

    /// Fase (0 a 1) de la animación de la línea de progreso.
    fn progress_phase(&self) -> f32 {
        (self.clock.elapsed().as_secs_f32() % PROGRESS_CYCLE) / PROGRESS_CYCLE
    }

    /// El cuadro completo (cromo y página) en px de dispositivo.
    fn frame(&self) -> Pixmap {
        let theme = self.chrome.theme;
        let dpi = self.dpi();
        let logical = self.logical_size();
        let width = (logical.width * dpi).round().max(1.0) as u32;
        let height = (logical.height * dpi).round().max(1.0) as u32;
        let mut frame = Pixmap::new(width, height).expect("tamaño de cuadro válido");
        frame.fill(bf_paint::raster_color(theme.chrome));
        let area = self.page_area();
        let viewport = self.viewport();
        if let Some(p) = &self.page {
            let key = RasterKey::new(self.page_generation, self.scroll, dpi * self.zoom, viewport);
            let mut cache = self.page_cache.borrow_mut();
            if cache.as_ref().is_none_or(|(k, _)| *k != key) {
                let start = Instant::now();
                let page = render_scrolled(
                    &p.display_list,
                    viewport,
                    dpi * self.zoom,
                    Rgba::WHITE,
                    self.scroll,
                );
                self.raster.set(Some(start.elapsed()));
                *cache = Some((key, page));
            }
            if let Some((_, page)) = cache.as_ref() {
                blit(&mut frame, page, 0.0, area.y * dpi);
            }
            if self.overlay {
                let view = Rect::new(
                    self.scroll.0,
                    self.scroll.1,
                    viewport.width,
                    viewport.height,
                );
                let vp = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let hover = self.overlay_hover.as_ref().map(|h| (h.0, h.1));
                let list = overlay::overlay(&p.tree, &theme, view, vp, hover);
                let layer = render_scrolled(
                    &list,
                    viewport,
                    dpi * self.zoom,
                    Rgba::TRANSPARENT,
                    self.scroll,
                );
                blit(&mut frame, &layer, 0.0, area.y * dpi);
            }
            if let (Some(f), Some(root), Some(current)) =
                (&self.focus, p.tree.root.as_ref(), &self.current)
            {
                let has_value = !edit::value(&current.doc, f.node, f.kind).is_empty();
                let mut list = DisplayList::default();
                if let Some((a, b)) = f.selection()
                    && let (Some(ra), Some(rb)) = (
                        edit::caret_rect(root, f.node, a, has_value),
                        edit::caret_rect(root, f.node, b, has_value),
                    )
                    && (ra.y - rb.y).abs() < 0.5
                {
                    list.push(DisplayItem::Rect {
                        rect: Rect::new(ra.x, ra.y, rb.x - ra.x, ra.height),
                        color: Theme::with_alpha(theme.accent, 0.35),
                        radii: Radii::default(),
                    });
                }
                if let Some(r) = edit::caret_rect(root, f.node, f.caret, has_value) {
                    list.push(DisplayItem::Rect {
                        rect: r,
                        color: theme.accent,
                        radii: Radii::default(),
                    });
                }
                if !list.items.is_empty() {
                    let layer = render_scrolled(
                        &list,
                        viewport,
                        dpi * self.zoom,
                        Rgba::TRANSPARENT,
                        self.scroll,
                    );
                    blit(&mut frame, &layer, 0.0, area.y * dpi);
                }
            }
        }
        if let Some(panel) = self.chrome.panel() {
            let scroll = (0.0, self.chrome.panel_scroll);
            let key = RasterKey::new(self.panel_generation, scroll, dpi, panel.size);
            let mut cache = self.panel_cache.borrow_mut();
            if cache.as_ref().is_none_or(|(k, _)| *k != key) {
                let layer =
                    render_scrolled(panel.display_list(), panel.size, dpi, theme.chrome, scroll);
                *cache = Some((key, layer));
            }
            if let Some((_, layer)) = cache.as_ref() {
                blit(&mut frame, layer, area.right() * dpi, area.y * dpi);
            }
        }
        if let Some(list) = self.chrome.bar_display_list(self.progress_phase()) {
            let bar = render(&list, self.chrome.bar_size(), dpi, theme.chrome);
            blit(&mut frame, &bar, 0.0, 0.0);
        }
        if let Some(status) = self.chrome.status() {
            let layer = render(
                status.display_list(),
                self.chrome.status_size(),
                dpi,
                theme.chrome,
            );
            blit(&mut frame, &layer, 0.0, (logical.height - STATUS) * dpi);
        }
        if let Some(menu) = &self.menu {
            let layer = render(menu.display_list(), menu.size(), dpi, Rgba::TRANSPARENT);
            blit(&mut frame, &layer, menu.origin.0 * dpi, menu.origin.1 * dpi);
        }
        frame
    }

    fn redraw(&mut self) {
        trace("redraw");
        let frame = self.frame();
        // El panel muestra el tiempo de raster en cuanto se conoce.
        if self.chrome.panel_open && self.panel_raster.is_none() && self.raster.get().is_some() {
            self.refresh_panel();
        }
        let Some(g) = &mut self.graphics else {
            return;
        };
        let size = g.window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        if g.surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buffer) = g.surface.buffer_mut() else {
            return;
        };
        let (bw, bh) = (size.width as usize, size.height as usize);
        let (fw, fh) = (frame.width() as usize, frame.height() as usize);
        let data = frame.data();
        buffer.fill(0);
        for y in 0..bh.min(fh) {
            for x in 0..bw.min(fw) {
                let i = (y * fw + x) * 4;
                // Fondo opaco: el pixmap premultiplicado ya es el color final.
                buffer[y * bw + x] = (u32::from(data[i]) << 16)
                    | (u32::from(data[i + 1]) << 8)
                    | u32::from(data[i + 2]);
            }
        }
        let _ = buffer.present();
    }

    // --- Entrada ---

    fn set_cursor(&mut self, cursor: CursorIcon) {
        if cursor != self.cursor {
            self.cursor = cursor;
            if let Some(g) = &self.graphics {
                g.window.set_cursor(cursor);
            }
        }
    }

    fn mouse_moved(&mut self) {
        let (x, y) = self.mouse;
        if let Some(menu) = &mut self.menu {
            let item = menu.item_at(x, y);
            if menu.set_hover(item) {
                self.redraw_later();
            }
            self.set_cursor(CursorIcon::Default);
            return;
        }
        let mut cursor = CursorIcon::Default;
        let mut link = None;
        let mut bar_hover = None;
        let mut overlay_hover = None;
        match self.area_at(x, y) {
            Area::Bar => {
                let region = self.chrome.region_at(x, y);
                if region == Some(Region::Field) {
                    cursor = CursorIcon::Text;
                } else if region.is_some() {
                    cursor = CursorIcon::Pointer;
                }
                bar_hover = region.filter(|r| *r != Region::Field);
            }
            Area::Status => {}
            Area::Panel => {
                let px = x - self.page_area().right();
                if let Some(panel) = self.chrome.panel()
                    && let Some(href) = panel.link_at(px, y - BAR, self.chrome.panel_scroll)
                {
                    cursor = CursorIcon::Pointer;
                    if !href.starts_with("bf:") {
                        link = Some(href);
                    }
                }
            }
            Area::Page => {
                if let (Some(current), Some(page)) = (&self.current, &self.page) {
                    let (px, py) = self.page_point(x, y);
                    if let Some(f) = hit::hit_test(&page.tree, px, py, self.scroll) {
                        let href = f.node.and_then(|n| hit::link_at(&current.doc, n));
                        cursor = hit::cursor_for(f, href.is_some());
                        link = href
                            .and_then(|(_, h)| Url::parse(&h, Some(&current.base)).ok())
                            .filter(|u| u.scheme != "bf")
                            .map(|u| u.href());
                        if self.overlay {
                            overlay_hover = Some((
                                f.rect,
                                fixed_ancestry(page.tree.root.as_ref(), f),
                                overlay::describe(&current.doc, f),
                            ));
                        }
                    }
                }
            }
        }
        self.set_cursor(cursor);
        let overlay_changed =
            overlay_hover.as_ref().map(|h| h.0) != self.overlay_hover.as_ref().map(|h| h.0);
        if link != self.hover_link || bar_hover != self.chrome.hover || overlay_changed {
            self.hover_link = link;
            self.chrome.hover = bar_hover;
            self.overlay_hover = overlay_hover;
            self.refresh_chrome();
        }
    }

    fn click(&mut self) {
        let (x, y) = self.mouse;
        if let Some(menu) = &self.menu {
            let action = menu
                .item_at(x, y)
                .and_then(|i| menu.items[i].action.clone());
            self.menu = None;
            self.redraw_later();
            if let Some(action) = action {
                self.menu_action(action);
            }
            return;
        }
        match self.area_at(x, y) {
            Area::Bar => match self.chrome.region_at(x, y) {
                Some(Region::Back) => self.back(),
                Some(Region::Forward) => self.forward(),
                Some(Region::Reload) if self.loading.is_some() => self.stop(),
                Some(Region::Reload) => self.reload(),
                Some(Region::Engine) => self.toggle_panel(),
                Some(Region::Field) if !self.chrome.field.focused => self.focus_bar(),
                Some(Region::Tab(i)) => self.activate_tab(i),
                Some(Region::CloseTab(i)) => self.close_tab(i),
                Some(Region::NewTab) => self.new_tab(None, false),
                _ => {}
            },
            Area::Status => {}
            Area::Panel => {
                let px = x - self.page_area().right();
                let href = self
                    .chrome
                    .panel()
                    .and_then(|p| p.link_at(px, y - BAR, self.chrome.panel_scroll));
                if let Some(url) = href.and_then(|h| Url::parse(&h, None).ok()) {
                    self.navigate(url);
                }
            }
            Area::Page => self.page_click(),
        }
    }

    /// Clic en la página: enlaces, campos de texto (foco y caret) y botones de envío.
    fn page_click(&mut self) {
        if self.chrome.field.focused {
            self.chrome.field.focused = false;
            self.refresh_chrome();
        }
        if let Some(url) = self.link_under_mouse() {
            if self.modifiers.control_key() {
                self.new_tab(Some(url), true);
            } else {
                self.navigate(url);
            }
            return;
        }
        enum Hit {
            Field(NodeId, Editable, usize),
            Submit(NodeId),
            Nothing,
        }
        let (x, y) = self.mouse;
        let (px, py) = self.page_point(x, y);
        let hit = match (&self.current, &self.page) {
            (Some(current), Some(page)) => {
                let node = hit::hit_test(&page.tree, px, py, self.scroll).and_then(|f| f.node);
                match node {
                    Some(n) => {
                        if let Some((node, kind)) = edit::editable_at(&current.doc, n) {
                            let caret = page.tree.root.as_ref().map_or(0, |r| {
                                edit::caret_from_point(
                                    r,
                                    node,
                                    px + self.scroll.0,
                                    py + self.scroll.1,
                                )
                            });
                            Hit::Field(node, kind, caret)
                        } else if let Some(b) = submit_button_at(&current.doc, n) {
                            Hit::Submit(b)
                        } else {
                            Hit::Nothing
                        }
                    }
                    None => Hit::Nothing,
                }
            }
            _ => Hit::Nothing,
        };
        match hit {
            Hit::Field(node, kind, caret) => {
                let shift = self.modifiers.shift_key();
                let anchor = match &self.focus {
                    Some(f) if f.node == node && shift => Some(f.anchor.unwrap_or(f.caret)),
                    _ => None,
                };
                self.focus = Some(Focus {
                    node,
                    kind,
                    caret,
                    anchor,
                });
                self.redraw_later();
            }
            Hit::Submit(button) => {
                self.focus = None;
                self.submit(button);
            }
            Hit::Nothing => {
                if self.focus.take().is_some() {
                    self.redraw_later();
                }
            }
        }
    }

    /// La URL del enlace bajo el puntero (en la página), sin `javascript:`.
    fn link_under_mouse(&self) -> Option<Url> {
        let (x, y) = self.mouse;
        let (current, page) = (self.current.as_ref()?, self.page.as_ref()?);
        let (px, py) = self.page_point(x, y);
        hit::hit_test(&page.tree, px, py, self.scroll)
            .and_then(|f| f.node)
            .and_then(|n| hit::link_at(&current.doc, n))
            .and_then(|(_, href)| Url::parse(&href, Some(&current.base)).ok())
            // `javascript:` necesita el motor de JavaScript (fase 2).
            .filter(|u| u.scheme != "javascript")
    }

    /// Botón central: cierra la pestaña bajo el puntero o abre un enlace en una nueva.
    fn middle_click(&mut self) {
        let (x, y) = self.mouse;
        match self.area_at(x, y) {
            Area::Bar => {
                if let Some(Region::Tab(i) | Region::CloseTab(i)) = self.chrome.region_at(x, y) {
                    self.close_tab(i);
                }
            }
            Area::Page => {
                if let Some(url) = self.link_under_mouse() {
                    self.new_tab(Some(url), true);
                }
            }
            _ => {}
        }
    }

    fn wheel(&mut self, dx: f32, dy: f32) {
        let (x, y) = self.mouse;
        if self.area_at(x, y) == Area::Panel {
            self.chrome.panel_scroll += dy;
            self.chrome.clamp_panel_scroll();
            self.redraw_later();
            return;
        }
        self.scroll_by(dx, dy);
        self.mouse_moved();
    }

    fn focus_bar(&mut self) {
        self.chrome.field.focused = true;
        self.chrome.field.set_text(self.chrome.url.clone());
        self.chrome.field.select_all();
        self.refresh_chrome();
    }

    /// Teclas con el foco en el campo de URL. Devuelve si la tecla se consumió.
    fn field_key(&mut self, key: &Key, text: Option<&str>) -> bool {
        let shift = self.modifiers.shift_key();
        let ctrl = self.modifiers.control_key();
        let field = &mut self.chrome.field;
        match key {
            Key::Named(NamedKey::Escape) => {
                field.focused = false;
            }
            Key::Named(NamedKey::Enter) => {
                let input = field.text.clone();
                field.focused = false;
                if let Some(url) = url_from_input(&input) {
                    self.navigate(url);
                    return true;
                }
            }
            Key::Named(NamedKey::Backspace) => field.backspace(),
            Key::Named(NamedKey::Delete) => field.delete(),
            Key::Named(NamedKey::ArrowLeft) => field.left(shift),
            Key::Named(NamedKey::ArrowRight) => field.right(shift),
            Key::Named(NamedKey::Home) => field.home(shift),
            Key::Named(NamedKey::End) => field.end(shift),
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("a") => field.select_all(),
            Key::Named(NamedKey::Space) => field.insert(" "),
            _ if ctrl || self.modifiers.alt_key() => return false,
            _ => match text {
                Some(t) if !t.chars().any(char::is_control) => field.insert(t),
                _ => return false,
            },
        }
        self.refresh_chrome();
        true
    }

    fn key(&mut self, event: &KeyEvent) {
        if event.state == ElementState::Pressed {
            self.key_press(&event.logical_key, event.text.as_deref());
        }
    }

    /// Una tecla pulsada (`text`: lo que escribe, si escribe algo).
    fn key_press(&mut self, key: &Key, text: Option<&str>) {
        let ctrl = self.modifiers.control_key();
        let alt = self.modifiers.alt_key();
        let shift = self.modifiers.shift_key();
        let is = |c: &str, k: &str| c.eq_ignore_ascii_case(k);
        if self.menu.take().is_some() {
            self.redraw_later();
            if matches!(key, Key::Named(NamedKey::Escape)) {
                return;
            }
        }
        // Atajos globales.
        match key {
            Key::Character(c) if ctrl && is(c, "t") => return self.new_tab(None, false),
            Key::Character(c) if ctrl && is(c, "w") => return self.close_tab(self.active),
            Key::Named(NamedKey::Tab) if ctrl && shift => return self.cycle_tab(-1),
            Key::Named(NamedKey::Tab) if ctrl => return self.cycle_tab(1),
            Key::Named(NamedKey::PageDown) if ctrl => return self.cycle_tab(1),
            Key::Named(NamedKey::PageUp) if ctrl => return self.cycle_tab(-1),
            Key::Character(c)
                if ctrl && c.len() == 1 && c.as_bytes()[0].is_ascii_digit() && c != "0" =>
            {
                let d = usize::from(c.as_bytes()[0] - b'0');
                let last = self.tabs.len() - 1;
                return self.activate_tab(if d == 9 { last } else { (d - 1).min(last) });
            }
            Key::Character(c) if ctrl && is(c, "l") => return self.focus_bar(),
            Key::Named(NamedKey::F6) => return self.focus_bar(),
            Key::Character(c) if ctrl && is(c, "i") => return self.toggle_panel(),
            Key::Named(NamedKey::F12) => return self.toggle_overlay(),
            Key::Character(c) if ctrl && is(c, "r") => return self.reload(),
            Key::Named(NamedKey::F5) => return self.reload(),
            Key::Character(c) if ctrl && shift && is(c, "d") => return self.dump(true),
            Key::Character(c) if ctrl && is(c, "s") => return self.save_png(),
            Key::Character(c) if ctrl && is(c, "h") => return self.home(),
            Key::Named(NamedKey::Escape) if self.loading.is_some() => return self.stop(),
            Key::Named(NamedKey::ArrowLeft) if alt => return self.back(),
            Key::Named(NamedKey::ArrowRight) if alt => return self.forward(),
            Key::Named(NamedKey::BrowserBack) => return self.back(),
            Key::Named(NamedKey::BrowserForward) => return self.forward(),
            Key::Character(c) if ctrl && (c == "+" || c == "=") => return self.zoom_step(true),
            Key::Character(c) if ctrl && c == "-" => return self.zoom_step(false),
            Key::Character(c) if ctrl && c == "0" => return self.set_zoom(1.0),
            _ => {}
        }
        if self.chrome.field.focused && self.field_key(key, text) {
            return;
        }
        if self.focus.is_some() && self.edit_key(key, text) {
            return;
        }
        // En `about:home`, escribir va al campo de URL.
        let on_home = self
            .current
            .as_ref()
            .is_some_and(|l| l.url.href() == "about:home");
        if on_home
            && !ctrl
            && !alt
            && let Some(t) =
                text.filter(|t| !t.chars().any(char::is_control) && !t.trim().is_empty())
        {
            self.chrome.field.focused = true;
            self.chrome.field.set_text(String::new());
            self.chrome.field.insert(t);
            self.refresh_chrome();
            return;
        }
        let page = self.viewport().height - LINE;
        match key {
            Key::Named(NamedKey::ArrowDown) => self.scroll_by(0.0, LINE),
            Key::Named(NamedKey::ArrowUp) => self.scroll_by(0.0, -LINE),
            Key::Named(NamedKey::ArrowRight) => self.scroll_by(LINE, 0.0),
            Key::Named(NamedKey::ArrowLeft) => self.scroll_by(-LINE, 0.0),
            Key::Named(NamedKey::PageDown) => self.scroll_by(0.0, page),
            Key::Named(NamedKey::PageUp) => self.scroll_by(0.0, -page),
            Key::Named(NamedKey::Space) if shift => self.scroll_by(0.0, -page),
            Key::Named(NamedKey::Space) => self.scroll_by(0.0, page),
            Key::Named(NamedKey::Home) => self.scroll_by(0.0, f32::NEG_INFINITY),
            Key::Named(NamedKey::End) => self.scroll_by(0.0, f32::INFINITY),
            _ => {}
        }
    }
}

/// ¿`target` está dentro de una caja con `position: fixed`?
fn fixed_ancestry(root: Option<&Fragment>, target: &Fragment) -> bool {
    fn walk(f: &Fragment, target: &Fragment, fixed: bool) -> Option<bool> {
        let fixed = fixed || f.style.position == bf_style::properties::types::Position::Fixed;
        if std::ptr::eq(f, target) {
            return Some(fixed);
        }
        f.children.iter().find_map(|c| walk(c, target, fixed))
    }
    root.and_then(|r| walk(r, target, false)).unwrap_or(false)
}

/// Evento de usuario: el hilo de trabajo terminó algo.
struct Wake;

impl ApplicationHandler<Wake> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.graphics.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_window_icon(window_icon())
            .with_title("«Cargando…» — browser-freedom")
            .with_inner_size(LogicalSize::new(1180.0, 800.0));
        let window = match event_loop.create_window(attributes) {
            Ok(w) => Rc::new(w),
            Err(e) => {
                eprintln!("no se pudo crear la ventana: {e}");
                event_loop.exit();
                return;
            }
        };
        let graphics = softbuffer::Context::new(window.clone()).and_then(|context| {
            let surface = softbuffer::Surface::new(&context, window.clone())?;
            Ok(Graphics {
                window: window.clone(),
                surface,
                _context: context,
            })
        });
        match graphics {
            Ok(g) => self.graphics = Some(g),
            Err(e) => {
                eprintln!("no se pudo crear la superficie de dibujo: {e}");
                event_loop.exit();
                return;
            }
        }
        // Tema del sistema (`prefers-color-scheme`).
        let dark = window.theme().map(|t| t == winit::window::Theme::Dark);
        self.chrome.theme = Theme::for_system(dark);
        let url = self
            .initial
            .take()
            .or_else(|| Url::parse("about:home", None).ok());
        if let Some(url) = url {
            self.start_load(url, Navigation::Push);
        } else {
            self.refresh_chrome();
        }
        window.request_redraw();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: Wake) {
        self.receive();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // La línea de progreso se anima mientras carga; el aviso se borra al vencer.
        if self.loading.is_some() {
            let next = self.last_progress_frame + PROGRESS_FRAME;
            if Instant::now() >= next {
                self.last_progress_frame = Instant::now();
                self.redraw_later();
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(next.max(Instant::now())));
        } else if let Some((_, at)) = &self.notice {
            let until = *at + NOTICE;
            if Instant::now() >= until {
                self.notice = None;
                self.refresh_chrome();
                event_loop.set_control_flow(ControlFlow::Wait);
            } else {
                event_loop.set_control_flow(ControlFlow::WaitUntil(until));
            }
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => self.resized(),
            WindowEvent::ThemeChanged(t) => {
                self.chrome.theme = Theme::for_system(Some(t == winit::window::Theme::Dark));
                self.refresh_chrome();
                self.refresh_panel();
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::CursorMoved { position, .. } => {
                let dpi = f64::from(self.dpi());
                self.mouse = ((position.x / dpi) as f32, (position.y / dpi) as f32);
                self.mouse_moved();
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => match button {
                MouseButton::Left => self.click(),
                MouseButton::Middle => self.middle_click(),
                MouseButton::Right => self.context_menu(),
                MouseButton::Back => self.back(),
                MouseButton::Forward => self.forward(),
                _ => {}
            },
            WindowEvent::MouseWheel { delta, .. } => {
                if self.modifiers.control_key() {
                    let up = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y > 0.0,
                        MouseScrollDelta::PixelDelta(p) => p.y > 0.0,
                    };
                    self.zoom_step(up);
                    return;
                }
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (-x * LINE, -y * LINE),
                    MouseScrollDelta::PixelDelta(p) => {
                        let s = f64::from(self.dpi() * self.zoom);
                        ((-p.x / s) as f32, (-p.y / s) as f32)
                    }
                };
                self.wheel(dx, dy);
            }
            WindowEvent::KeyboardInput { event, .. } => self.key(&event),
            _ => {}
        }
    }
}

/// Abre una ventana con el documento (o `about:home`) y corre el bucle de eventos hasta que
/// se cierre.
pub fn open(arg: Option<&str>) -> Result<(), String> {
    let url = match arg {
        Some(a) => {
            Some(bf_infra::url_from_arg(a).ok_or_else(|| format!("URL o ruta inválida: {a}"))?)
        }
        None => None,
    };
    let event_loop = EventLoop::<Wake>::with_user_event()
        .build()
        .map_err(|e| format!("no se pudo iniciar la ventana: {e}"))?;
    let proxy: EventLoopProxy<Wake> = event_loop.create_proxy();
    let fonts = FontDb::system();
    let worker = Worker::spawn(fonts.clone(), move || {
        let _ = proxy.send_event(Wake);
    });
    let mut app = App::new(worker, fonts, url, Theme::for_system(None));
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("error en el bucle de eventos: {e}"))
}

impl App {
    /// Espera a que el hilo de trabajo termine lo pendiente (sesiones sin ventana).
    fn settle(&mut self) {
        while self.any_loading() || self.relayout.is_some() {
            let Ok(done) = self
                .worker
                .done
                .recv_timeout(std::time::Duration::from_secs(120))
            else {
                break;
            };
            if let Some(viewport) = self.deliver(done)
                && viewport != self.viewport()
                && self.loading.is_none()
                && self.relayout.is_none()
            {
                self.request_relayout();
            }
        }
        // Un primer cuadro para que el panel conozca el tiempo de raster.
        if self.chrome.panel_open && self.panel_raster.is_none() {
            let _ = self.frame();
            self.refresh_panel();
        }
    }

    /// Centro (px lógicos de la ventana) del primer texto de un enlace que contiene `needle`.
    fn link_point(&self, needle: &str) -> Option<(f32, f32)> {
        /// Textos que contienen `needle`, con si están dentro de una caja fija.
        fn walk<'f>(
            f: &'f Fragment,
            needle: &str,
            fixed: bool,
            out: &mut Vec<(&'f Fragment, bool)>,
        ) {
            let fixed = fixed || f.style.position == bf_style::properties::types::Position::Fixed;
            if let Some((text, _)) = &f.text
                && text.contains(needle)
            {
                out.push((f, fixed));
            }
            for c in &f.children {
                walk(c, needle, fixed, out);
            }
        }
        let (current, page) = (self.current.as_ref()?, self.page.as_ref()?);
        let mut found = Vec::new();
        walk(page.tree.root.as_ref()?, needle, false, &mut found);
        let (f, fixed) = found
            .into_iter()
            .find(|(f, _)| f.node.and_then(|n| hit::link_at(&current.doc, n)).is_some())?;
        let (cx, cy) = (
            f.rect.x + f.rect.width / 2.0,
            f.rect.y + f.rect.height / 2.0,
        );
        let (sx, sy) = if fixed { (0.0, 0.0) } else { self.scroll };
        Some(((cx - sx) * self.zoom, (cy - sy) * self.zoom + BAR))
    }

    fn state_line(&self) -> String {
        let url = self
            .history
            .get(self.index)
            .map_or_else(String::new, |e| e.url.href());
        let title = self
            .current
            .as_ref()
            .and_then(|l| l.title())
            .unwrap_or_default();
        let focus = if self.chrome.field.focused {
            " (con foco)"
        } else {
            ""
        };
        let field = match (&self.focus, &self.current) {
            (Some(f), Some(c)) => format!(
                " campo={:?}@{}",
                edit::value(&c.doc, f.node, f.kind),
                f.caret
            ),
            _ => String::new(),
        };
        format!(
            "pestaña={}/{} url={url} título={title:?} scroll={:.0},{:.0} historial={}/{} zoom={} barra={:?}{focus}{field} estado={:?} panel={} cajas={}",
            self.active + 1,
            self.tabs.len(),
            self.scroll.0,
            self.scroll.1,
            self.index + 1,
            self.history.len(),
            self.zoom,
            self.chrome.field.text,
            self.chrome.status_left,
            self.chrome.panel_open,
            self.overlay,
        )
    }

    /// Un paso de una sesión sin ventana.
    fn step(&mut self, step: &str) -> Result<(), String> {
        let bad = || format!("paso inválido: {step}");
        let point = |v: &str| -> Option<(f32, f32)> {
            let (x, y) = v.split_once(',')?;
            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
        };
        let (verb, arg) = step.split_once(':').unwrap_or((step, ""));
        match verb {
            "wait" => {}
            "newtab" => self.new_tab(None, false),
            "field" => {
                let Some(cur) = self.current.clone() else {
                    return Err(bad());
                };
                let node = cur
                    .doc
                    .inclusive_descendants(cur.doc.root())
                    .into_iter()
                    .find(|&n| {
                        cur.doc
                            .element(n)
                            .is_some_and(|e| e.attr("name") == Some(arg))
                    })
                    .ok_or_else(|| format!("no hay un campo con name={arg:?}"))?;
                let (node, kind) = edit::editable_at(&cur.doc, node)
                    .ok_or_else(|| format!("{arg:?} no es un campo editable"))?;
                let caret = edit::value(&cur.doc, node, kind).chars().count();
                self.focus = Some(Focus {
                    node,
                    kind,
                    caret,
                    anchor: None,
                });
            }
            "rightclick" => {
                self.mouse = point(arg).ok_or_else(bad)?;
                self.mouse_moved();
                self.context_menu();
            }
            "closetab" => self.close_tab(self.active),
            "tab" => {
                let n: usize = arg.trim().parse().map_err(|_| bad())?;
                self.activate_tab(n.checked_sub(1).ok_or_else(bad)?);
            }
            "move" => {
                self.mouse = point(arg).ok_or_else(bad)?;
                self.mouse_moved();
            }
            "click" => {
                self.mouse = point(arg).ok_or_else(bad)?;
                self.mouse_moved();
                self.click();
            }
            "link" => {
                self.mouse = self
                    .link_point(arg)
                    .ok_or_else(|| format!("no hay un enlace con el texto {arg:?}"))?;
                self.mouse_moved();
                self.click();
            }
            "resize" => {
                let (w, h) = point(arg).ok_or_else(bad)?;
                self.headless = Size {
                    width: w,
                    height: h,
                };
                self.resized();
            }
            "type" => {
                for c in arg.chars() {
                    let s = c.to_string();
                    self.key_press(&Key::Character(SmolStr::new(&s)), Some(&s));
                }
            }
            "key" => {
                let mut name = arg;
                let mut modifiers = ModifiersState::empty();
                while let Some((m, rest)) = name.split_once('+').filter(|(_, r)| !r.is_empty()) {
                    modifiers |= match m {
                        "ctrl" => ModifiersState::CONTROL,
                        "alt" => ModifiersState::ALT,
                        "shift" => ModifiersState::SHIFT,
                        _ => return Err(bad()),
                    };
                    name = rest;
                }
                let named = match name {
                    "enter" => Some(NamedKey::Enter),
                    "escape" => Some(NamedKey::Escape),
                    "backspace" => Some(NamedKey::Backspace),
                    "delete" => Some(NamedKey::Delete),
                    "left" => Some(NamedKey::ArrowLeft),
                    "right" => Some(NamedKey::ArrowRight),
                    "up" => Some(NamedKey::ArrowUp),
                    "down" => Some(NamedKey::ArrowDown),
                    "home" => Some(NamedKey::Home),
                    "end" => Some(NamedKey::End),
                    "pageup" => Some(NamedKey::PageUp),
                    "pagedown" => Some(NamedKey::PageDown),
                    "space" => Some(NamedKey::Space),
                    "f5" => Some(NamedKey::F5),
                    "f6" => Some(NamedKey::F6),
                    "f12" => Some(NamedKey::F12),
                    "tab" => Some(NamedKey::Tab),
                    _ => None,
                };
                self.modifiers = modifiers;
                match named {
                    Some(k) => self.key_press(&Key::Named(k), None),
                    None if name.chars().count() == 1 => {
                        let text = modifiers.is_empty().then_some(name);
                        self.key_press(&Key::Character(SmolStr::new(name)), text);
                    }
                    None => return Err(bad()),
                }
                self.modifiers = ModifiersState::empty();
            }
            _ => return Err(bad()),
        }
        self.settle();
        Ok(())
    }
}

/// `browser-freedom session [url] <paso>... -o <prefijo>`: la lógica de la ventana sin abrir
/// una (1180×800, escala 1), para probarla. Cada paso deja `<prefijo>-NN.png` con el cuadro
/// completo (cromo y página) e imprime el estado. Pasos: `click:X,Y`, `move:X,Y`,
/// `link:texto` (clic en el enlace con ese texto), `type:texto`,
/// `key:[ctrl+|alt+|shift+]tecla`, `resize:W,H` y `wait`.
pub fn session(arg: &str, steps: &[String], out: &str) -> Result<(), String> {
    let url = bf_infra::url_from_arg(arg).ok_or_else(|| format!("URL o ruta inválida: {arg}"))?;
    let fonts = FontDb::system();
    let worker = Worker::spawn(fonts.clone(), || {});
    let mut app = App::new(worker, fonts, None, Theme::for_system(None));
    app.headless = Size {
        width: 1180.0,
        height: 800.0,
    };
    let save = |app: &App, n: usize| -> Result<(), String> {
        let path = format!("{out}-{n:02}.png");
        app.frame()
            .save_png(&path)
            .map_err(|e| format!("no se pudo escribir {path}: {e}"))
    };
    app.start_load(url, Navigation::Push);
    app.settle();
    println!("00 abrir {arg}: {}", app.state_line());
    save(&app, 0)?;
    for (i, step) in steps.iter().enumerate() {
        app.step(step)?;
        println!("{:02} {step}: {}", i + 1, app.state_line());
        save(&app, i + 1)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_to_url() {
        let href = |s: &str| url_from_input(s).map(|u| u.href());
        assert_eq!(href("example.com").as_deref(), Some("https://example.com/"));
        assert_eq!(
            href("  https://es.wikipedia.org/wiki/A ").as_deref(),
            Some("https://es.wikipedia.org/wiki/A")
        );
        assert_eq!(
            href("localhost:8000/x").as_deref(),
            Some("https://localhost:8000/x")
        );
        assert_eq!(
            href("data:text/html,hi").as_deref(),
            Some("data:text/html,hi")
        );
        assert_eq!(href("about:home").as_deref(), Some("about:home"));
        assert_eq!(
            href("motor de navegador").as_deref(),
            Some("https://www.google.com/search?q=motor+de+navegador")
        );
        assert!(
            href("C:\\Windows\\x.html")
                .unwrap()
                .starts_with("file:///C:/Windows/x.html")
        );
        assert_eq!(href(""), None);
    }

    #[test]
    fn same_document_ignores_fragment() {
        let a = Url::parse("https://a.test/p?q#x", None).unwrap();
        let b = Url::parse("https://a.test/p?q#y", None).unwrap();
        let c = Url::parse("https://a.test/p?r#x", None).unwrap();
        assert!(same_document(&a, &b));
        assert!(!same_document(&a, &c));
    }

    #[test]
    fn timestamp_format() {
        let t = timestamp();
        assert_eq!(t.len(), 15);
        assert_eq!(&t[8..9], "-");
    }
}
