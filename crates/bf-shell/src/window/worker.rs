//! Un hilo por pestaña ([`DocumentThread`]): es dueño del documento de la pestaña, de su motor
//! de JavaScript ([`Engine`]) y de su bucle de eventos. Carga (fetch, parseo con ejecución de
//! scripts), estilo, imágenes, layout y lista de visualización; corre los temporizadores y las
//! tareas del documento y, si cambiaron el DOM, vuelve a maquetar (un cuadro cada ≥ 16 ms).
//!
//! La interfaz le manda [`Job`]s y recibe [`FromDoc`] por un canal compartido por todas las
//! pestañas; `wake` la despierta (un evento de usuario de winit) cuando hay algo.

use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bf_font::FontDb;
use bf_infra::Url;
use bf_js::host::ConsoleLevel;
use bf_layout::Size;
use bf_net::NetLoader;
use bf_paint::{Page, PageContext, prepare_page_animated, prepare_page_with_frames};

use crate::internal;
use crate::load::{ErrorKind, LoadContext, LoadError, Loaded, load_with};
use crate::script::{ConsoleEntry, ConsoleSink, Engine, PageEnv, PageRequest, THREAD_STACK};
use crate::theme::Theme;

/// Intervalo mínimo entre dos cuadros producidos por scripts.
const FRAME: Duration = Duration::from_millis(16);

/// Cargar `url` y maquetarla. `recent` y `theme` son para las páginas internas.
pub struct LoadJob {
    pub id: u64,
    pub url: Url,
    pub net: Arc<NetLoader>,
    pub recent: Vec<(String, String)>,
    pub theme: Theme,
    /// Cuerpo `POST` (formularios).
    pub post: Option<(Vec<u8>, String)>,
    /// Entradas del historial de la pestaña hasta la nueva inclusive y después.
    pub session_history: (u32, u32),
    /// La URL de la página desde la que se navegó (`document.referrer`).
    pub referrer: String,
    /// La pantalla (`window.screen`).
    pub screen: bf_web::bom::ScreenInfo,
    /// El hilo anterior de la pestaña entró en pánico con este mensaje: en lugar de cargar
    /// `url`, se muestra la página "Esta página falló" ([`ErrorKind::Crashed`]).
    pub crash: Option<String>,
}

pub enum Job {
    Load(Box<LoadJob>),
    /// Volver a hacer estilo y layout (otro viewport). Con `loaded`, la interfaz cambió el
    /// documento (la edición de un campo) y el hilo adopta esa versión.
    Relayout {
        id: u64,
        loaded: Option<Arc<Loaded>>,
    },
    /// Una entrada del usuario para el documento.
    Input(Box<super::events::UserInput>),
    /// La pestaña se mostró (`true`) u ocultó.
    Visibility(bool),
    /// Otra pestaña cambió el `localStorage` de un origen.
    Storage(Box<crate::script::StorageNotice>),
}

/// Un documento maquetado.
pub struct Done {
    /// El trabajo que lo pidió; 0 para un cuadro producido por scripts.
    pub id: u64,
    /// La pestaña (clave de [`Documents`]).
    pub tab: u64,
    pub loaded: Arc<Loaded>,
    pub page: Page,
    /// Viewport con el que se maquetó.
    pub viewport: Size,
}

/// Cómo ve la ventana la página de una pestaña: viewport, desplazamiento y px de dispositivo
/// por px CSS (para el `LayoutSnapshot` del CSSOM View).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub viewport: Size,
    pub scroll: (f32, f32),
    pub device_pixel_ratio: f32,
    /// Zoom de texto del navegador (E11; 1 = sin zoom).
    pub text_zoom: f32,
    /// E4: cuadros mandados a la ventana por el hilo y recibidos por ella. Mientras la
    /// ventana va atrasada no se corren cuadros de animación (HTML §8.1.7.3: el agente puede
    /// saltarse oportunidades de renderizado que no puede mostrar).
    pub frames_sent: u64,
    pub frames_taken: u64,
}

impl Default for View {
    fn default() -> Self {
        View {
            viewport: Size {
                width: 800.0,
                height: 600.0,
            },
            scroll: (0.0, 0.0),
            device_pixel_ratio: 1.0,
            text_zoom: 1.0,
            frames_sent: 0,
            frames_taken: 0,
        }
    }
}

/// Lo que manda un hilo de documento a la ventana.
pub enum FromDoc {
    Done(Box<Done>),
    /// Un mensaje de la consola del documento de la pestaña `tab`.
    Console {
        tab: u64,
        entry: ConsoleEntry,
    },
    /// Algo que el documento de la pestaña `tab` le pide a la interfaz.
    Request {
        tab: u64,
        request: Box<PageRequest>,
    },
    /// El hilo de la pestaña `tab` entró en pánico y terminó; la ventana llama a
    /// [`Documents::crashed`] y vuelve a cargar la pestaña con la página de error.
    Crashed {
        tab: u64,
        message: String,
    },
}

/// Los hilos de documento de todas las pestañas.
pub struct Documents {
    fonts: Arc<FontDb>,
    wake: Arc<dyn Fn() + Send + Sync>,
    tx: Sender<FromDoc>,
    pub rx: Receiver<FromDoc>,
    threads: HashMap<u64, DocumentThread>,
}

impl Documents {
    pub fn new(fonts: Arc<FontDb>, wake: impl Fn() + Send + Sync + 'static) -> Documents {
        let (tx, rx) = channel();
        Documents {
            fonts,
            wake: Arc::new(wake),
            tx,
            rx,
            threads: HashMap::new(),
        }
    }

    fn thread(&mut self, tab: u64) -> &DocumentThread {
        let (fonts, wake, tx) = (self.fonts.clone(), self.wake.clone(), self.tx.clone());
        self.threads
            .entry(tab)
            .or_insert_with(|| DocumentThread::spawn(tab, fonts, tx, wake))
    }

    /// Manda un trabajo al hilo de la pestaña `tab` (lo crea si hace falta).
    pub fn send(&mut self, tab: u64, job: Job) {
        let _ = self.thread(tab).jobs.send(Msg::Job(job));
    }

    pub fn set_viewport(&mut self, tab: u64, size: Size) {
        self.thread(tab)
            .view
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .viewport = size;
    }

    /// El zoom de texto de la pestaña `tab` (E11); el próximo layout lo usa.
    pub fn set_text_zoom(&mut self, tab: u64, zoom: f32) {
        self.thread(tab)
            .view
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .text_zoom = zoom;
    }

    /// E4: la ventana recibió un cuadro de la pestaña `tab` (ver [`View::frames_taken`]).
    pub fn frame_taken(&mut self, tab: u64) {
        if let Some(t) = self.threads.get(&tab) {
            t.view
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .frames_taken += 1;
        }
    }

    /// El zoom de texto con el que se maqueta la pestaña `tab`.
    pub fn text_zoom(&mut self, tab: u64) -> f32 {
        self.thread(tab)
            .view
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .text_zoom
    }

    /// El desplazamiento y la escala de la página cambiaron: el hilo actualiza el snapshot
    /// del layout (y dispara `scroll`).
    pub fn set_scroll(&mut self, tab: u64, scroll: (f32, f32), device_pixel_ratio: f32) {
        let t = self.thread(tab);
        let changed = {
            let mut v = t.view.lock().unwrap_or_else(|e| e.into_inner());
            let changed = v.scroll != scroll || v.device_pixel_ratio != device_pixel_ratio;
            v.scroll = scroll;
            v.device_pixel_ratio = device_pixel_ratio;
            changed
        };
        if changed {
            let _ = t.jobs.send(Msg::ViewChanged);
        }
    }

    /// Termina el hilo de la pestaña `tab` (se cerró).
    pub fn close(&mut self, tab: u64) {
        if let Some(t) = self.threads.remove(&tab) {
            let _ = t.jobs.send(Msg::Stop);
        }
    }

    /// Cierra los hilos de todas las pestañas (E11: se cierra la ventana).
    pub fn close_all(&mut self) {
        for (_, t) in self.threads.drain() {
            let _ = t.jobs.send(Msg::Stop);
        }
    }

    /// El hilo de la pestaña `tab` terminó por un pánico ([`FromDoc::Crashed`]): se olvida,
    /// y el próximo trabajo de la pestaña arranca uno nuevo.
    pub fn crashed(&mut self, tab: u64) {
        self.threads.remove(&tab);
    }

    /// Manda un trabajo solo si la pestaña `tab` ya tiene hilo (E11: una pestaña restaurada
    /// que todavía no se mostró no arranca uno para un aviso).
    pub fn send_existing(&mut self, tab: u64, job: Job) {
        if let Some(t) = self.threads.get(&tab) {
            let _ = t.jobs.send(Msg::Job(job));
        }
    }
}

enum Msg {
    Job(Job),
    /// Llegó una respuesta de red (`fetch`/XHR): hay tareas que correr.
    NetWake,
    /// Cambió el desplazamiento o la escala (ver [`Documents::set_scroll`]).
    ViewChanged,
    Stop,
}

/// El hilo de una pestaña.
struct DocumentThread {
    jobs: Sender<Msg>,
    /// Cómo ve la ventana la página; el hilo lee el viewport justo antes del layout.
    view: Arc<Mutex<View>>,
}

fn panic_message(e: &(dyn std::any::Any + Send)) -> String {
    e.downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| e.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "error interno".to_owned())
}

fn engine_error(message: String) -> LoadError {
    LoadError {
        kind: ErrorKind::Engine,
        message,
    }
}

/// El documento cargado de una pestaña, con su motor.
struct Current {
    loaded: Arc<Loaded>,
    engine: Option<Engine>,
    /// `Document::version` del último cuadro mandado.
    version: u64,
    /// E1: los bitmaps de los `<canvas>` y su versión (dibujar no cambia el documento).
    canvases: Vec<(bf_dom::NodeId, Arc<bf_paint::Pixmap>)>,
    canvas_version: u64,
    /// E4: ¿había animaciones o transiciones CSS en curso en el último cuadro?
    animating: bool,
    /// E4: el último cuadro cambió los estilos animados: hay que volver a maquetar y pintar
    /// aunque el documento no haya cambiado.
    animation_frame: bool,
}

/// El estado del hilo.
struct State {
    tab: u64,
    fonts: Arc<FontDb>,
    tx: Sender<FromDoc>,
    wake: Arc<dyn Fn() + Send + Sync>,
    /// El canal de este mismo hilo (para que la red lo despierte).
    me: Sender<Msg>,
    view: Arc<Mutex<View>>,
    current: Option<Current>,
    last_frame: Instant,
    /// E4: el último cuadro de `requestAnimationFrame`.
    last_raf: Instant,
    /// Puntero y foco del documento actual.
    input: super::events::InputState,
    /// El viewport y el scroll del último `resize`/`scroll` despachado.
    last_viewport: Option<Size>,
    last_scroll: (f32, f32),
    /// E4: el desplazamiento de cada marco (por su camino de `<iframe>`).
    frame_scroll: HashMap<Vec<bf_dom::NodeId>, (f32, f32)>,
}

impl DocumentThread {
    fn spawn(
        tab: u64,
        fonts: Arc<FontDb>,
        tx: Sender<FromDoc>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> DocumentThread {
        let (jobs, rx) = channel::<Msg>();
        let view = Arc::new(Mutex::new(View::default()));
        let shared = view.clone();
        let me = jobs.clone();
        // Para avisar a la ventana si el hilo entra en pánico (el estado se pierde con él).
        let (crash_tx, crash_wake) = (tx.clone(), wake.clone());
        std::thread::Builder::new()
            .name(format!("freedom-pestana-{tab}"))
            .stack_size(THREAD_STACK)
            .spawn(move || {
                // El motor de JavaScript no es `Send`: el estado nace en el hilo.
                let state = State {
                    tab,
                    fonts,
                    tx,
                    wake,
                    me,
                    view: shared,
                    current: None,
                    last_frame: Instant::now(),
                    last_raf: Instant::now(),
                    input: super::events::InputState::default(),
                    last_viewport: None,
                    last_scroll: (0.0, 0.0),
                    frame_scroll: HashMap::new(),
                };
                // Un pánico fuera de la carga y del layout (un evento, un temporizador, un
                // cuadro) terminaría el hilo y dejaría la pestaña muerta: se captura aquí y la
                // ventana muestra "Esta página falló" con otro hilo. El resto de las pestañas
                // y la ventana no se enteran.
                if let Err(e) = catch_unwind(AssertUnwindSafe(|| state.run(rx))) {
                    let message = panic_message(&*e);
                    eprintln!("[pestaña {tab}] el hilo falló: {message}");
                    if crash_tx.send(FromDoc::Crashed { tab, message }).is_ok() {
                        crash_wake();
                    }
                }
            })
            .expect("no se pudo crear el hilo de la pestaña");
        DocumentThread { jobs, view }
    }
}

/// De los trabajos pendientes, solo importan la última carga y el último re-layout (con el
/// documento más reciente que haya mandado la interfaz).
fn coalesce(queue: &mut VecDeque<Job>) {
    if let Some(last) = queue.iter().rposition(|j| matches!(j, Job::Load(_))) {
        queue.drain(..last);
    }
    // De varios `mousemove` seguidos solo cuenta el último.
    let is_move = |j: &Job| {
        matches!(j, Job::Input(i) if matches!(**i, super::events::UserInput::Mouse {
            kind: super::events::MouseKind::Move,
            ..
        }))
    };
    let mut i = 0;
    while i + 1 < queue.len() {
        if is_move(&queue[i]) && is_move(&queue[i + 1]) {
            queue.remove(i);
        } else {
            i += 1;
        }
    }
    let Some(last) = queue
        .iter()
        .rposition(|j| matches!(j, Job::Relayout { .. }))
    else {
        return;
    };
    let adopted = queue.iter().rev().find_map(|j| match j {
        Job::Relayout {
            loaded: Some(l), ..
        } => Some(l.clone()),
        _ => None,
    });
    let mut i = 0;
    queue.retain(|j| {
        let keep = !matches!(j, Job::Relayout { .. }) || i == last;
        i += 1;
        keep
    });
    if let Some(l) = adopted
        && let Some(Job::Relayout { loaded, .. }) =
            queue.iter_mut().find(|j| matches!(j, Job::Relayout { .. }))
    {
        *loaded = Some(l);
    }
}

impl State {
    fn view(&self) -> View {
        *self.view.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn viewport(&self) -> Size {
        self.view().viewport
    }

    /// E4: ¿la ventana todavía no recibió el último cuadro que se le mandó?
    fn window_behind(&self) -> bool {
        let v = self.view();
        v.frames_sent > v.frames_taken
    }

    /// El desplazamiento o la escala cambiaron: el snapshot del layout se pone al día y, si
    /// cambió el scroll, el documento recibe `scroll` (CSSOM View §13.1).
    fn view_changed(&mut self) {
        let view = self.view();
        let scrolled = view.scroll != self.last_scroll;
        self.last_scroll = view.scroll;
        if let Some(engine) = self.current.as_mut().and_then(|c| c.engine.as_mut()) {
            engine.update_view(view.scroll, view.device_pixel_ratio);
            if scrolled && let Some(root) = engine.document().map(bf_dom::Document::root) {
                let mut ev = bf_dom::events::Event::new(
                    "scroll",
                    bf_dom::events::EventInit {
                        bubbles: true,
                        ..Default::default()
                    },
                );
                engine.dispatch(bf_dom::events::EventTarget::Node(root), &mut ev);
            }
        }
        self.refresh();
    }

    /// Una entrada del usuario: el dispatch, lo que pide a la ventana y, si cambió el
    /// documento, un cuadro nuevo.
    fn input(&mut self, input: super::events::UserInput) {
        let Some(current) = &mut self.current else {
            return;
        };
        // E4: la rueda sobre un marco que puede desplazarse lo desplaza a él.
        if let super::events::UserInput::FrameScroll { frame, scroll } = &input {
            if self.frame_scroll.get(frame) != Some(scroll) {
                self.frame_scroll.insert(frame.clone(), *scroll);
                self.layout(0, None);
            }
            return;
        }
        let requests = super::events::handle(
            current.engine.as_mut(),
            &mut current.loaded,
            &mut self.input,
            input,
        );
        for request in requests {
            let _ = self.tx.send(FromDoc::Request {
                tab: self.tab,
                request: Box::new(request),
            });
            (self.wake)();
        }
        self.refresh();
    }

    /// Si los scripts (o una entrada) cambiaron el documento, un cuadro nuevo (como mucho uno
    /// cada [`FRAME`]).
    fn refresh(&mut self) {
        let Some(current) = &mut self.current else {
            return;
        };
        if let Some(engine) = &mut current.engine
            && engine.take_dirty()
            && let Some(doc) = engine.render_copy()
        {
            // Los `<iframe>` nuevos o con otro `src`.
            engine.sync_frames();
            let mut loaded = (*current.loaded).clone();
            loaded.doc = doc;
            current.loaded = Arc::new(loaded);
        }
        // E1: un `<canvas>` dibujado pide un cuadro nuevo aunque el documento no cambie.
        let mut canvas_changed = false;
        if let Some(engine) = &mut current.engine {
            let v = engine.canvas_version();
            if v != current.canvas_version {
                current.canvas_version = v;
                current.canvases = engine.canvases();
                canvas_changed = true;
            }
            // E4: lo mismo si cambió el documento de un marco o si el cuadro movió las
            // animaciones CSS.
            canvas_changed |= engine.take_frames_dirty();
            canvas_changed |= std::mem::take(&mut current.animation_frame);
        }
        if canvas_changed || current.loaded.doc.version() != current.version {
            let wait = FRAME.saturating_sub(self.last_frame.elapsed());
            if !wait.is_zero() {
                std::thread::sleep(wait);
            }
            self.layout(0, None);
        }
    }

    /// El bucle del hilo: trabajos de la interfaz y, entre ellos, las tareas del documento.
    fn run(mut self, rx: Receiver<Msg>) {
        let mut queue: VecDeque<Job> = VecDeque::new();
        loop {
            if queue.is_empty() {
                // Dormir hasta el próximo temporizador, el próximo cuadro (si hay callbacks de
                // `requestAnimationFrame`; sin ellos no hay cuadros y el hilo duerme) o el
                // próximo mensaje.
                // E4: con la ventana atrasada, el próximo intento es dentro de un cuadro.
                let to_frame = if self.window_behind() {
                    FRAME
                } else {
                    FRAME.saturating_sub(self.last_raf.elapsed())
                };
                let wait = self
                    .current
                    .as_mut()
                    .and_then(|c| c.engine.as_mut())
                    .and_then(|e| {
                        let frame = e.wants_frame().then_some(to_frame);
                        match (e.next_wake(), frame) {
                            (Some(a), Some(b)) => Some(a.min(b)),
                            (a, b) => a.or(b),
                        }
                    });
                let msg = match wait {
                    Some(d) => match rx.recv_timeout(d) {
                        Ok(m) => Some(m),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => break,
                    },
                    None => match rx.recv() {
                        Ok(m) => Some(m),
                        Err(_) => break,
                    },
                };
                match msg {
                    Some(Msg::Stop) => {
                        self.unload_current();
                        break;
                    }
                    Some(Msg::ViewChanged) => {
                        self.view_changed();
                        continue;
                    }
                    Some(Msg::NetWake) => {
                        self.run_tasks();
                        continue;
                    }
                    Some(Msg::Job(j)) => queue.push_back(j),
                    None => {
                        self.run_tasks();
                        continue;
                    }
                }
            }
            let mut stop = false;
            while let Ok(next) = rx.try_recv() {
                match next {
                    Msg::Job(j) => queue.push_back(j),
                    Msg::ViewChanged => self.view_changed(),
                    Msg::NetWake => {}
                    Msg::Stop => stop = true,
                }
            }
            if stop {
                self.unload_current();
                break;
            }
            coalesce(&mut queue);
            let Some(job) = queue.pop_front() else {
                continue;
            };
            match job {
                Job::Load(job) => self.load(*job),
                Job::Input(input) => self.input(*input),
                Job::Visibility(visible) => {
                    if let Some(e) = self.current.as_mut().and_then(|c| c.engine.as_mut()) {
                        e.set_visibility(visible);
                    }
                    self.refresh();
                }
                Job::Storage(notice) => {
                    if let Some(e) = self.current.as_mut().and_then(|c| c.engine.as_mut()) {
                        e.storage_event(&notice);
                    }
                    self.refresh();
                }
                Job::Relayout { id, loaded } => {
                    if let (Some(l), Some(c)) = (loaded, self.current.as_mut()) {
                        if let Some(engine) = &mut c.engine {
                            engine.set_document(l.doc.clone());
                        }
                        c.loaded = l;
                    }
                    self.layout(id, None);
                }
            }
            // Lo que haya vencido mientras tanto.
            self.run_tasks();
        }
    }

    /// Los mensajes de la consola van a la ventana (y los errores, también a stderr).
    fn console_sink(&self) -> ConsoleSink {
        let (tx, wake, tab) = (self.tx.clone(), self.wake.clone(), self.tab);
        Box::new(move |entry: ConsoleEntry| {
            if entry.level == ConsoleLevel::Error {
                eprintln!("[pestaña {tab}] {}", entry.text);
            }
            if tx.send(FromDoc::Console { tab, entry }).is_ok() {
                wake();
            }
        })
    }

    /// Las peticiones del documento a la interfaz.
    fn request_sink(&self) -> crate::script::RequestSink {
        let (tx, wake, tab) = (self.tx.clone(), self.wake.clone(), self.tab);
        std::rc::Rc::new(move |request: PageRequest| {
            if tx
                .send(FromDoc::Request {
                    tab,
                    request: Box::new(request),
                })
                .is_ok()
            {
                wake();
            }
        })
    }

    fn load(&mut self, job: LoadJob) {
        let LoadJob {
            id,
            url,
            net,
            recent,
            theme,
            post,
            session_history,
            referrer,
            screen,
            crash,
        } = job;
        let net_waker: Arc<dyn Fn() + Send + Sync> = {
            let me = Mutex::new(self.me.clone());
            Arc::new(move || {
                let _ = me
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .send(Msg::NetWake);
            })
        };
        let env = PageEnv {
            net_waker: Some(net_waker),
            defer_load: true,
            fonts: Some(self.fonts.clone()),
            net: net.clone(),
            requests: Some(self.request_sink()),
            session_history,
            referrer,
            screen,
        };
        let ctx = LoadContext {
            net,
            fonts: self.fonts.clone(),
            viewport: self.viewport(),
            recent,
            theme,
            post,
        };
        // El documento anterior recibe `unload` y se descarta (con su motor).
        self.unload_current();
        self.current = None;
        self.input.reset();
        self.last_viewport = None;
        self.frame_scroll.clear();
        // E4: el "site for cookies" de lo que pida este hilo es el del documento de la
        // pestaña (RFC 6265bis §5.2).
        set_cookie_site(&url);
        let mut engine = Engine::new(self.console_sink());
        let loaded = match crash {
            Some(message) => {
                let err = LoadError {
                    kind: ErrorKind::Crashed,
                    message,
                };
                internal::error_page(&url, &err, &ctx)
            }
            None => {
                #[cfg(test)]
                crash_test::maybe_panic(&url);
                catch_unwind(AssertUnwindSafe(|| {
                    load_with(&url, &ctx, Some((&mut engine, &env)))
                }))
                .unwrap_or_else(|e| {
                    let err = engine_error(panic_message(&*e));
                    internal::error_page(&url, &err, &ctx)
                })
            }
        };
        set_cookie_site(&loaded.url);
        let version = loaded.doc.version();
        self.current = Some(Current {
            loaded: Arc::new(loaded),
            engine: Some(engine),
            version,
            canvases: Vec::new(),
            canvas_version: 0,
            animating: false,
            animation_frame: false,
        });
        self.layout(id, Some(ctx));
        // El primer layout cargó imágenes y hojas: ya puede salir `load` (13.2.7 paso 8).
        if let Some(e) = self.current.as_mut().and_then(|c| c.engine.as_mut()) {
            e.resources_loaded();
        }
        self.refresh();
    }

    /// `beforeunload`, `pagehide` y `unload` en el documento actual.
    fn unload_current(&mut self) {
        if let Some(e) = self.current.as_mut().and_then(|c| c.engine.as_mut()) {
            e.unload();
        }
    }

    /// Estilo, layout y lista de visualización del documento actual; `Done` a la ventana.
    fn layout(&mut self, id: u64, ctx: Option<LoadContext>) {
        let is_load = ctx.is_some();
        let viewport = self.viewport();
        let view_zoom = self.view().text_zoom;
        let fonts = self.fonts.clone();
        let Some(current) = &mut self.current else {
            return;
        };
        let canvases = current.canvases.clone();
        // E4: los documentos de los `<iframe>`, que se pintan dentro de su caja.
        let frames = current
            .engine
            .as_ref()
            .map(|e| e.frame_docs(&self.frame_scroll))
            .unwrap_or_default();
        // E4: con motor, los estilos de la cascada pasan por sus animaciones y transiciones
        // CSS (el "style change event" del cuadro) antes de maquetar.
        let page = |loaded: &Loaded, engine: Option<&mut Engine>| {
            let pctx = PageContext {
                base: &loaded.base,
                encoding: loaded.encoding,
                loader: loaded.loader.clone(),
                fonts: fonts.clone(),
                extra: &[],
                text_zoom: view_zoom,
            };
            catch_unwind(AssertUnwindSafe(|| match engine {
                Some(engine) => prepare_page_animated(
                    &loaded.doc,
                    &pctx,
                    viewport,
                    &canvases,
                    &frames,
                    &mut |styles| engine.animated_styles(styles),
                ),
                None => prepare_page_with_frames(&loaded.doc, &pctx, viewport, &canvases, &frames),
            }))
        };
        let loaded = current.loaded.clone();
        let (loaded, mut page) = match page(&loaded, current.engine.as_mut()) {
            Ok(p) => (loaded, p),
            // Un error del motor con este documento: se muestra como página de error en
            // lugar de terminar el hilo.
            Err(e) => {
                let message = format!("error del motor: {}", panic_message(&*e));
                eprintln!("{message}");
                let ctx = ctx.unwrap_or_else(|| LoadContext {
                    net: loaded.loader.net.clone(),
                    fonts: fonts.clone(),
                    viewport,
                    recent: Vec::new(),
                    theme: Theme::for_system(None),
                    post: None,
                });
                let err = engine_error(message);
                let shown = Arc::new(internal::error_page(&loaded.url, &err, &ctx));
                let Ok(p) = page(&shown, None) else {
                    return;
                };
                if let Some(c) = &mut self.current {
                    c.loaded = shown.clone();
                    c.engine = None;
                }
                (shown, p)
            }
        };
        // E4: el desglose de la carga para medir la primera pintura (`BF_TRACE`).
        if is_load && super::tracing() {
            let scripts = self
                .current
                .as_ref()
                .and_then(|c| c.engine.as_ref())
                .map_or(Duration::ZERO, Engine::script_time);
            let t = page.timings;
            super::trace(&format!(
                "perf carga {}: red/caché {} · parseo {} · scripts {} · hojas {} · cascada {} \
                 · imágenes {} · maqueta {} · lista {} ({} nodos)",
                loaded.url,
                super::ms(loaded.fetch),
                super::ms(loaded.parse.saturating_sub(scripts)),
                super::ms(scripts),
                super::ms(t.style.saturating_sub(t.cascade)),
                super::ms(t.cascade),
                super::ms(t.images),
                super::ms(t.layout),
                super::ms(t.paint),
                page.report.dom_nodes,
            ));
        }
        let view = self.view();
        // E12: las posiciones de scroll de los elementos y lo `sticky` del viewport.
        if let Some(engine) = self.current.as_mut().and_then(|c| c.engine.as_mut()) {
            let offsets = engine.element_scroll_offsets();
            page.apply_scroll(&loaded.doc, &offsets, view.scroll);
        }
        let resized = self
            .last_viewport
            .is_some_and(|v| v.width != viewport.width || v.height != viewport.height);
        self.last_viewport = Some(viewport);
        if let Some(c) = &mut self.current {
            c.version = loaded.doc.version();
            if let Some(engine) = &mut c.engine {
                if resized {
                    // `resize` en la ventana (CSSOM View §13.1).
                    let mut ev =
                        bf_dom::events::Event::new("resize", bf_dom::events::EventInit::default());
                    engine.dispatch(bf_dom::events::EventTarget::Window, &mut ev);
                }
                engine.set_layout(
                    &page,
                    loaded.doc.version(),
                    view.scroll,
                    view.device_pixel_ratio,
                );
            }
        }
        self.last_frame = Instant::now();
        let done = Done {
            id,
            tab: self.tab,
            loaded,
            page,
            viewport,
        };
        if self.tx.send(FromDoc::Done(Box::new(done))).is_ok() {
            self.view
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .frames_sent += 1;
            (self.wake)();
        }
    }

    /// Las tareas vencidas del documento; si cambiaron el DOM, un cuadro nuevo (como mucho
    /// uno cada [`FRAME`]).
    fn run_tasks(&mut self) {
        let behind = self.window_behind();
        let Some(current) = &mut self.current else {
            return;
        };
        let Some(engine) = &mut current.engine else {
            return;
        };
        let mut ran = engine.run_due_tasks();
        // E4: "update the rendering" (HTML §8.1.7.3): como mucho un cuadro cada [`FRAME`].
        if engine.wants_frame() && self.last_raf.elapsed() >= FRAME && !behind {
            self.last_raf = Instant::now();
            engine.run_animation_frames();
            ran += 1;
            // E4: mientras haya animaciones o transiciones CSS (y en el cuadro en que terminan,
            // para mostrar su estado final), cada cuadro se vuelve a maquetar y pintar. Sin
            // ellas no se piden cuadros (`wants_frame`) y el hilo duerme.
            let animating = engine.is_animating();
            current.animation_frame |= animating || current.animating;
            current.animating = animating;
        }
        if let Some((x, y)) = engine.take_scroll_request() {
            let _ = self.tx.send(FromDoc::Request {
                tab: self.tab,
                request: Box::new(PageRequest::ScrollTo(x, y)),
            });
            (self.wake)();
        }
        let changes = engine.take_element_scroll_changes();
        if !changes.is_empty() {
            let _ = self.tx.send(FromDoc::Request {
                tab: self.tab,
                request: Box::new(PageRequest::ScrollElements(changes)),
            });
            (self.wake)();
        }
        if ran == 0 {
            return;
        }
        self.refresh();
    }
}

/// E4: declara `url` como el documento de nivel superior de este hilo para las cookies
/// `SameSite` (RFC 6265bis §5.2, "site for cookies").
pub fn set_cookie_site(url: &Url) {
    bf_net::cookies::set_thread_context(Some(bf_net::cookies::CookieContext {
        site_for_cookies: bf_net::cookies::site_of(url),
        top_level_navigation: false,
    }));
}

/// Para los tests: una URL que hace entrar en pánico al hilo de la pestaña fuera de las
/// capturas de la carga y del layout, como un fallo en un evento o un temporizador.
#[cfg(test)]
pub mod crash_test {
    pub const URL: &str = "about:blank#panico-de-prueba";

    pub fn maybe_panic(url: &bf_infra::Url) {
        if url.href() == URL {
            panic!("pánico de prueba en el hilo de la pestaña");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesce_keeps_last_load_and_relayout() {
        let net = Arc::new(NetLoader::new());
        let load = |id| {
            Job::Load(Box::new(LoadJob {
                id,
                url: Url::parse("about:blank", None).unwrap(),
                net: net.clone(),
                recent: Vec::new(),
                theme: Theme::for_system(None),
                post: None,
                session_history: (1, 0),
                referrer: String::new(),
                screen: bf_web::bom::ScreenInfo::default(),
                crash: None,
            }))
        };
        let mut q: VecDeque<Job> = VecDeque::from([
            load(1),
            Job::Relayout {
                id: 2,
                loaded: None,
            },
            load(3),
            Job::Relayout {
                id: 4,
                loaded: None,
            },
            Job::Relayout {
                id: 5,
                loaded: None,
            },
        ]);
        coalesce(&mut q);
        let ids: Vec<u64> = q
            .iter()
            .map(|j| match j {
                Job::Load(l) => l.id,
                Job::Relayout { id, .. } => *id,
                Job::Input(_) | Job::Visibility(_) | Job::Storage(_) => 0,
            })
            .collect();
        assert_eq!(ids, [3, 5]);
    }
}
