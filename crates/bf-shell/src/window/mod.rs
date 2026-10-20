//! `browser-freedom open <url|archivo>`: una ventana nativa (winit + softbuffer) navegable.
//!
//! - **Barra** (36 px, [`chrome`]): ◀ ▶ ⟳ y el campo de URL. `Ctrl+L` (o `F6`, o un clic)
//!   enfoca el campo y selecciona todo; se escribe, se borra con `Retroceso`/`Supr`, se mueve
//!   el cursor con las flechas, `Inicio` y `Fin` (con `Mayús`, seleccionando) y `Intro` navega:
//!   sin esquema se antepone `https://`, y lo que parece una ruta local se abre como `file://`.
//!   `Esc` devuelve el foco a la página.
//! - **Carga** en un hilo ([`worker`]): la interfaz nunca se bloquea; la barra muestra
//!   "Cargando…" y los errores de red se muestran como una página de error interna.
//! - **Navegación**: un clic en un enlace navega (resuelto contra la URL base del documento,
//!   `<base href>` incluido); un fragmento `#id` del mismo documento solo desplaza. El cursor
//!   es una mano sobre los enlaces y respeta `cursor`. Historial con `Alt+←`/`Alt+→` y los
//!   botones; `Ctrl+R`/`F5` recarga.
//! - **Scroll**: rueda, flechas, `Re Pág`/`Av Pág`, `Inicio`/`Fin` y espacio, limitado al tamaño
//!   del documento; las cajas `position: fixed` no se desplazan. Redimensionar vuelve a hacer
//!   estilo y layout.
//! - **Zoom**: `Ctrl +`, `Ctrl -` y `Ctrl 0` cambian la escala del raster y el ancho del
//!   viewport en px CSS.

mod chrome;
mod hit;
mod worker;

use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::Arc;

use bf_font::FontDb;
use bf_infra::Url;
use bf_layout::Fragment;
use bf_layout::Size;
use bf_net::NetLoader;
use bf_paint::{Page, Pixmap, document_extent, render, render_scrolled};
use bf_style::values::color::Rgba;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey, SmolStr};
use winit::window::{CursorIcon, Window, WindowId};

use crate::load::{Loaded, indicated_element};
use chrome::{Region, Toolbar};
use worker::{Done, Job, Worker};

/// Px CSS por "línea" de rueda y por pulsación de flecha.
const LINE: f32 = 40.0;

/// Niveles de zoom (como los de los navegadores).
const ZOOM_LEVELS: [f32; 15] = [
    0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
];

struct Graphics {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    // El contexto debe vivir tanto como la superficie.
    _context: softbuffer::Context<Rc<Window>>,
}

/// Una entrada del historial de sesión: la URL y dónde estaba el scroll al salir.
struct Entry {
    url: Url,
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

struct App {
    net: Arc<NetLoader>,
    worker: Worker,
    current: Option<Arc<Loaded>>,
    page: Option<Page>,
    scroll: (f32, f32),
    history: Vec<Entry>,
    index: usize,
    next_id: u64,
    /// Carga en curso: su id, qué hacer con el historial y la URL pedida.
    loading: Option<(u64, Navigation, Url)>,
    /// Re-layout en curso (id).
    relayout: Option<u64>,
    toolbar: Toolbar,
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
}

/// Interpreta lo escrito en el campo de URL.
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
    Url::parse(&format!("https://{s}"), None).ok()
}

/// ¿`a` y `b` son el mismo documento salvo el fragmento?
fn same_document(a: &Url, b: &Url) -> bool {
    a.serialize(true) == b.serialize(true)
}

impl App {
    fn new(worker: Worker, fonts: Arc<FontDb>, initial: Option<Url>) -> App {
        App {
            net: Arc::new(NetLoader::new()),
            worker,
            current: None,
            page: None,
            scroll: (0.0, 0.0),
            history: Vec::new(),
            index: 0,
            next_id: 0,
            loading: None,
            relayout: None,
            toolbar: Toolbar::new(fonts),
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

    /// Viewport de la página en px CSS: la ventana menos la barra, dividida por el zoom.
    fn viewport(&self) -> Size {
        let s = self.logical_size();
        Size {
            width: (s.width / self.zoom).max(1.0),
            height: ((s.height - chrome::HEIGHT) / self.zoom).max(1.0),
        }
    }

    fn redraw_later(&self) {
        if let Some(g) = &self.graphics {
            g.window.request_redraw();
        }
    }

    fn update_title(&self) {
        let Some(g) = &self.graphics else {
            return;
        };
        let name = if self.loading.is_some() {
            "Cargando…".to_owned()
        } else {
            self.current.as_ref().map_or_else(String::new, |l| {
                l.title().unwrap_or_else(|| match &l.url.path {
                    bf_url::Path::List(segments) if l.url.scheme == "file" => {
                        segments.last().cloned().unwrap_or_default()
                    }
                    _ => l.url.host_string(),
                })
            })
        };
        g.window.set_title(&format!("{name} — browser-freedom"));
    }

    fn refresh_toolbar(&mut self) {
        self.toolbar.can_back = self.index > 0 && !self.history.is_empty();
        self.toolbar.can_forward = self.index + 1 < self.history.len();
        self.toolbar.status = if self.loading.is_some() {
            "Cargando…".to_owned()
        } else {
            self.hover_link.clone().unwrap_or_default()
        };
        if !self.toolbar.field.focused {
            let shown = match &self.loading {
                Some((_, _, url)) => url.href(),
                None => self
                    .history
                    .get(self.index)
                    .map(|e| e.url.href())
                    .or_else(|| self.current.as_ref().map(|l| l.url.href()))
                    .unwrap_or_default(),
            };
            self.toolbar.field.set_text(shown);
        }
        let width = self.logical_size().width;
        self.toolbar.layout(width);
        self.redraw_later();
    }

    // --- Navegación ---

    fn start_load(&mut self, url: Url, nav: Navigation) {
        // El scroll de la entrada actual, para volver a él.
        if let Some(e) = self.history.get_mut(self.index) {
            e.scroll = self.scroll;
        }
        self.next_id += 1;
        let id = self.next_id;
        self.worker.set_viewport(self.viewport());
        self.worker.send(Job::Load {
            id,
            url: url.clone(),
            net: self.net.clone(),
        });
        self.loading = Some((id, nav, url));
        self.relayout = None;
        self.update_title();
        self.refresh_toolbar();
    }

    /// Navega a `url` (un enlace o lo escrito en la barra).
    fn navigate(&mut self, url: Url) {
        if let Some(current) = &self.current
            && url.fragment.is_some()
            && same_document(&current.url, &url)
            && self.loading.is_none()
        {
            // Navegación a un fragmento (HTML §7.4.2.3.3): solo se desplaza.
            if let Some(e) = self.history.get_mut(self.index) {
                e.scroll = self.scroll;
            }
            self.history.truncate(self.index + 1);
            self.history.push(Entry {
                url: url.clone(),
                scroll: (0.0, 0.0),
            });
            self.index = self.history.len() - 1;
            self.scroll_to_fragment(&url);
            self.refresh_toolbar();
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
            self.toolbar.field.focused = false;
            self.refresh_toolbar();
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
            (Some((_, _, url)), _) => url.clone(),
            (None, Some(c)) => c.url.clone(),
            _ => return,
        };
        self.start_load(url, Navigation::Reload);
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

    /// Resultados del hilo de trabajo.
    fn receive(&mut self) {
        let mut laid_out = None;
        while let Ok(done) = self.worker.done.try_recv() {
            laid_out = Some(done.viewport);
            self.apply(done);
        }
        // Si la ventana cambió mientras se maquetaba, se rehace con el viewport actual.
        if laid_out.is_some_and(|v| v != self.viewport())
            && self.loading.is_none()
            && self.relayout.is_none()
        {
            self.request_relayout();
        }
    }

    fn apply(&mut self, done: Done) {
        let Done {
            id, loaded, page, ..
        } = done;
        if self.relayout == Some(id) {
            self.relayout = None;
            self.page = Some(page);
            self.clamp_scroll();
            self.redraw_later();
            return;
        }
        let Some((pending, nav, _)) = &self.loading else {
            return;
        };
        if *pending != id {
            return;
        }
        let nav = *nav;
        self.loading = None;
        let url = loaded.url.clone();
        self.current = Some(loaded);
        self.page = Some(page);
        self.hover_link = None;
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
            }
            Navigation::Reload => {}
        }
        self.clamp_scroll();
        self.toolbar.field.focused = false;
        self.update_title();
        self.refresh_toolbar();
    }

    fn request_relayout(&mut self) {
        let Some(loaded) = self.current.clone() else {
            return;
        };
        self.next_id += 1;
        let id = self.next_id;
        self.worker.set_viewport(self.viewport());
        self.worker.send(Job::Relayout { id, loaded });
        self.relayout = Some(id);
    }

    /// El viewport cambió (ventana o zoom): se rehace el layout (en el hilo) y la barra.
    fn resized(&mut self) {
        if self.loading.is_some() {
            // La carga en curso lee el viewport nuevo antes del layout (o se rehace al
            // terminar).
            self.worker.set_viewport(self.viewport());
        } else {
            self.request_relayout();
        }
        self.refresh_toolbar();
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

    /// El cuadro completo (barra y página) en px de dispositivo.
    fn frame(&self) -> Pixmap {
        let dpi = self.dpi();
        let logical = self.logical_size();
        let width = (logical.width * dpi).round().max(1.0) as u32;
        let height = (logical.height * dpi).round().max(1.0) as u32;
        let mut frame = Pixmap::new(width, height).expect("tamaño de cuadro válido");
        frame.fill(bf_paint::raster_color(Rgba::WHITE));
        let bar_rows = (chrome::HEIGHT * dpi).round();
        if let Some(p) = &self.page {
            let page = render_scrolled(
                &p.display_list,
                self.viewport(),
                dpi * self.zoom,
                Rgba::WHITE,
                self.scroll,
            );
            frame.draw_pixmap(
                0,
                bar_rows as i32,
                page.as_ref(),
                &bf_paint::PixmapPaint::default(),
                bf_paint::Transform::identity(),
                None,
            );
        }
        if let Some(p) = self.toolbar.page() {
            let bar = render(
                &p.display_list,
                self.toolbar.size(),
                dpi,
                Rgba::rgb(0xDE, 0xE1, 0xE6),
            );
            frame.draw_pixmap(
                0,
                0,
                bar.as_ref(),
                &bf_paint::PixmapPaint::default(),
                bf_paint::Transform::identity(),
                None,
            );
        }
        frame
    }

    fn redraw(&mut self) {
        let frame = self.frame();
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
        buffer.fill(0x00FF_FFFF);
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
        if y < chrome::HEIGHT {
            let region = self.toolbar.region_at(x, y);
            self.set_cursor(if region == Some(Region::Field) {
                CursorIcon::Text
            } else {
                CursorIcon::Default
            });
            let hover = region.filter(|r| *r != Region::Field);
            if hover != self.toolbar.hover || self.hover_link.is_some() {
                self.toolbar.hover = hover;
                self.hover_link = None;
                self.refresh_toolbar();
            }
            return;
        }
        let mut link = None;
        let mut cursor = CursorIcon::Default;
        if let (Some(current), Some(page)) = (&self.current, &self.page) {
            let (px, py) = (x / self.zoom, (y - chrome::HEIGHT) / self.zoom);
            if let Some(f) = hit::hit_test(&page.tree, px, py, self.scroll) {
                let href = f.node.and_then(|n| hit::link_at(&current.doc, n));
                cursor = hit::cursor_for(f, href.is_some());
                link = href
                    .and_then(|(_, h)| Url::parse(&h, Some(&current.base)).ok())
                    .map(|u| u.href());
            }
        }
        self.set_cursor(cursor);
        if link != self.hover_link || self.toolbar.hover.is_some() {
            self.hover_link = link;
            self.toolbar.hover = None;
            self.refresh_toolbar();
        }
    }

    fn click(&mut self) {
        let (x, y) = self.mouse;
        if y < chrome::HEIGHT {
            match self.toolbar.region_at(x, y) {
                Some(Region::Back) => self.back(),
                Some(Region::Forward) => self.forward(),
                Some(Region::Reload) => self.reload(),
                Some(Region::Field) if !self.toolbar.field.focused => self.focus_bar(),
                _ => {}
            }
            return;
        }
        if self.toolbar.field.focused {
            self.toolbar.field.focused = false;
            self.refresh_toolbar();
        }
        let (Some(current), Some(page)) = (&self.current, &self.page) else {
            return;
        };
        let (px, py) = (x / self.zoom, (y - chrome::HEIGHT) / self.zoom);
        let target = hit::hit_test(&page.tree, px, py, self.scroll)
            .and_then(|f| f.node)
            .and_then(|n| hit::link_at(&current.doc, n))
            .and_then(|(_, href)| Url::parse(&href, Some(&current.base)).ok());
        if let Some(url) = target {
            // `javascript:` necesita el motor de JavaScript (fase 2).
            if url.scheme != "javascript" {
                self.navigate(url);
            }
        }
    }

    fn focus_bar(&mut self) {
        self.toolbar.field.focused = true;
        self.toolbar.field.select_all();
        self.refresh_toolbar();
    }

    /// Teclas con el foco en el campo de URL. Devuelve si la tecla se consumió.
    fn field_key(&mut self, key: &Key, text: Option<&str>) -> bool {
        let shift = self.modifiers.shift_key();
        let ctrl = self.modifiers.control_key();
        let field = &mut self.toolbar.field;
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
        self.refresh_toolbar();
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
        // Atajos globales.
        match key {
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("l") => return self.focus_bar(),
            Key::Named(NamedKey::F6) => return self.focus_bar(),
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("r") => return self.reload(),
            Key::Named(NamedKey::F5) => return self.reload(),
            Key::Named(NamedKey::ArrowLeft) if alt => return self.back(),
            Key::Named(NamedKey::ArrowRight) if alt => return self.forward(),
            Key::Named(NamedKey::BrowserBack) => return self.back(),
            Key::Named(NamedKey::BrowserForward) => return self.forward(),
            Key::Character(c) if ctrl && (c == "+" || c == "=") => return self.zoom_step(true),
            Key::Character(c) if ctrl && c == "-" => return self.zoom_step(false),
            Key::Character(c) if ctrl && c == "0" => return self.set_zoom(1.0),
            _ => {}
        }
        if self.toolbar.field.focused && self.field_key(key, text) {
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
            Key::Named(NamedKey::Space) if self.modifiers.shift_key() => {
                self.scroll_by(0.0, -page);
            }
            Key::Named(NamedKey::Space) => self.scroll_by(0.0, page),
            Key::Named(NamedKey::Home) => self.scroll_by(0.0, f32::NEG_INFINITY),
            Key::Named(NamedKey::End) => self.scroll_by(0.0, f32::INFINITY),
            _ => {}
        }
    }
}

/// Evento de usuario: el hilo de trabajo terminó algo.
struct Wake;

impl ApplicationHandler<Wake> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.graphics.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Cargando… — browser-freedom")
            .with_inner_size(LogicalSize::new(1024.0, 768.0));
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
        if let Some(url) = self.initial.take() {
            self.start_load(url, Navigation::Push);
        } else {
            self.refresh_toolbar();
        }
        window.request_redraw();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: Wake) {
        self.receive();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => self.resized(),
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
                self.scroll_by(dx, dy);
                self.mouse_moved();
            }
            WindowEvent::KeyboardInput { event, .. } => self.key(&event),
            _ => {}
        }
    }
}

/// Abre una ventana con el documento y corre el bucle de eventos hasta que se cierre.
pub fn open(arg: &str) -> Result<(), String> {
    let url = bf_infra::url_from_arg(arg).ok_or_else(|| format!("URL o ruta inválida: {arg}"))?;
    let event_loop = EventLoop::<Wake>::with_user_event()
        .build()
        .map_err(|e| format!("no se pudo iniciar la ventana: {e}"))?;
    let proxy: EventLoopProxy<Wake> = event_loop.create_proxy();
    let fonts = FontDb::system();
    let worker = Worker::spawn(fonts.clone(), move || {
        let _ = proxy.send_event(Wake);
    });
    let mut app = App::new(worker, fonts, Some(url));
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("error en el bucle de eventos: {e}"))
}

impl App {
    /// Espera a que el hilo de trabajo termine lo pendiente (sesiones sin ventana).
    fn settle(&mut self) {
        while self.loading.is_some() || self.relayout.is_some() {
            let Ok(done) = self
                .worker
                .done
                .recv_timeout(std::time::Duration::from_secs(120))
            else {
                break;
            };
            let viewport = done.viewport;
            self.apply(done);
            if viewport != self.viewport() && self.loading.is_none() && self.relayout.is_none() {
                self.request_relayout();
            }
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
        Some((
            (cx - sx) * self.zoom,
            (cy - sy) * self.zoom + chrome::HEIGHT,
        ))
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
        let focus = if self.toolbar.field.focused {
            " (con foco)"
        } else {
            ""
        };
        format!(
            "url={url} título={title:?} scroll={:.0},{:.0} historial={}/{} zoom={} barra={:?}{focus} estado={:?}",
            self.scroll.0,
            self.scroll.1,
            self.index + 1,
            self.history.len(),
            self.zoom,
            self.toolbar.field.text,
            self.toolbar.status,
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

/// `browser-freedom session <url> <paso>... -o <prefijo>`: la lógica de la ventana sin abrir
/// una (1024×768, escala 1), para probarla. Cada paso deja `<prefijo>-NN.png` con el cuadro
/// completo (barra y página) e imprime el estado. Pasos: `click:X,Y`, `move:X,Y`, `link:texto`
/// (clic en el enlace con ese texto), `type:texto`, `key:[ctrl+|alt+|shift+]tecla`,
/// `resize:W,H` y `wait`.
pub fn session(arg: &str, steps: &[String], out: &str) -> Result<(), String> {
    let url = bf_infra::url_from_arg(arg).ok_or_else(|| format!("URL o ruta inválida: {arg}"))?;
    let fonts = FontDb::system();
    let worker = Worker::spawn(fonts.clone(), || {});
    let mut app = App::new(worker, fonts, None);
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
}
