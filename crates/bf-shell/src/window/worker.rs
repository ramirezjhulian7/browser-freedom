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
use bf_paint::{Page, PageContext, prepare_page};

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
}

enum Msg {
    Job(Job),
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
}

/// El estado del hilo.
struct State {
    tab: u64,
    fonts: Arc<FontDb>,
    tx: Sender<FromDoc>,
    wake: Arc<dyn Fn() + Send + Sync>,
    view: Arc<Mutex<View>>,
    current: Option<Current>,
    last_frame: Instant,
    /// Puntero y foco del documento actual.
    input: super::events::InputState,
    /// El viewport y el scroll del último `resize`/`scroll` despachado.
    last_viewport: Option<Size>,
    last_scroll: (f32, f32),
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
                    view: shared,
                    current: None,
                    last_frame: Instant::now(),
                    input: super::events::InputState::default(),
                    last_viewport: None,
                    last_scroll: (0.0, 0.0),
                };
                state.run(rx);
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
            let mut loaded = (*current.loaded).clone();
            loaded.doc = doc;
            current.loaded = Arc::new(loaded);
        }
        if current.loaded.doc.version() != current.version {
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
                // Dormir hasta el próximo temporizador o el próximo mensaje.
                let wait = self
                    .current
                    .as_mut()
                    .and_then(|c| c.engine.as_mut())
                    .and_then(Engine::next_wake);
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
        } = job;
        let env = PageEnv {
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
        let mut engine = Engine::new(self.console_sink());
        let loaded = catch_unwind(AssertUnwindSafe(|| {
            load_with(&url, &ctx, Some((&mut engine, &env)))
        }))
        .unwrap_or_else(|e| {
            let err = engine_error(panic_message(&*e));
            internal::error_page(&url, &err, &ctx)
        });
        let version = loaded.doc.version();
        self.current = Some(Current {
            loaded: Arc::new(loaded),
            engine: Some(engine),
            version,
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
        let Some(current) = &self.current else {
            return;
        };
        let viewport = self.viewport();
        let fonts = self.fonts.clone();
        let page = |loaded: &Loaded| {
            let pctx = PageContext {
                base: &loaded.base,
                encoding: loaded.encoding,
                loader: loaded.loader.clone(),
                fonts: fonts.clone(),
                extra: &[],
            };
            catch_unwind(AssertUnwindSafe(|| {
                prepare_page(&loaded.doc, &pctx, viewport)
            }))
        };
        let loaded = current.loaded.clone();
        let (loaded, page) = match page(&loaded) {
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
                let Ok(p) = page(&shown) else {
                    return;
                };
                if let Some(c) = &mut self.current {
                    c.loaded = shown.clone();
                    c.engine = None;
                }
                (shown, p)
            }
        };
        let view = self.view();
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
            (self.wake)();
        }
    }

    /// Las tareas vencidas del documento; si cambiaron el DOM, un cuadro nuevo (como mucho
    /// uno cada [`FRAME`]).
    fn run_tasks(&mut self) {
        let Some(current) = &mut self.current else {
            return;
        };
        let Some(engine) = &mut current.engine else {
            return;
        };
        let ran = engine.run_due_tasks();
        if let Some((x, y)) = engine.take_scroll_request() {
            let _ = self.tx.send(FromDoc::Request {
                tab: self.tab,
                request: Box::new(PageRequest::ScrollTo(x, y)),
            });
            (self.wake)();
        }
        if ran == 0 {
            return;
        }
        self.refresh();
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
