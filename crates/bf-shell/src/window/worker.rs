//! Carga y maquetación en un hilo de trabajo: fetch, parseo, estilo, imágenes, layout y lista
//! de visualización. La interfaz le manda trabajos y recibe los resultados por
//! `std::sync::mpsc`; `wake` la despierta (un evento de usuario de winit) cuando hay uno.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use bf_font::FontDb;
use bf_infra::Url;
use bf_layout::Size;
use bf_net::NetLoader;
use bf_paint::{Page, PageContext, prepare_page};

use crate::internal;
use crate::load::{ErrorKind, LoadContext, LoadError, Loaded, load_with};
use crate::theme::Theme;

/// Cargar `url` y maquetarla. `recent` y `theme` son para las páginas internas.
pub struct LoadJob {
    pub id: u64,
    pub url: Url,
    pub net: Arc<NetLoader>,
    pub recent: Vec<(String, String)>,
    pub theme: Theme,
    /// Cuerpo `POST` (formularios).
    pub post: Option<(Vec<u8>, String)>,
}

pub enum Job {
    Load(Box<LoadJob>),
    /// Volver a hacer estilo y layout de un documento ya cargado (otro viewport).
    Relayout {
        id: u64,
        loaded: Arc<Loaded>,
    },
}

pub struct Done {
    pub id: u64,
    pub loaded: Arc<Loaded>,
    pub page: Page,
    /// Viewport con el que se maquetó.
    pub viewport: Size,
}

pub struct Worker {
    jobs: Sender<Job>,
    pub done: Receiver<Done>,
    /// Viewport actual de la página (px CSS), que el hilo lee justo antes del layout.
    viewport: Arc<Mutex<Size>>,
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

impl Worker {
    pub fn spawn(fonts: Arc<FontDb>, wake: impl Fn() + Send + 'static) -> Worker {
        let (jobs, rx) = channel::<Job>();
        let (tx, done) = channel::<Done>();
        let viewport = Arc::new(Mutex::new(Size {
            width: 800.0,
            height: 600.0,
        }));
        let shared = viewport.clone();
        std::thread::Builder::new()
            .name("bf-carga".into())
            .spawn(move || {
                let current_viewport = || *shared.lock().unwrap_or_else(|e| e.into_inner());
                let mut queue: std::collections::VecDeque<Job> = std::collections::VecDeque::new();
                loop {
                    if queue.is_empty() {
                        let Ok(job) = rx.recv() else { break };
                        queue.push_back(job);
                    }
                    while let Ok(next) = rx.try_recv() {
                        queue.push_back(next);
                    }
                    // Las cargas se hacen todas (varias pestañas pueden cargar a la vez); de
                    // los re-layouts solo importa el último pedido.
                    if let Some(last) = queue
                        .iter()
                        .rposition(|j| matches!(j, Job::Relayout { .. }))
                    {
                        let mut i = 0;
                        queue.retain(|j| {
                            let keep = !matches!(j, Job::Relayout { .. }) || i == last;
                            i += 1;
                            keep
                        });
                    }
                    let Some(job) = queue.pop_front() else {
                        continue;
                    };
                    let (id, loaded, ctx) = match job {
                        Job::Load(job) => {
                            let LoadJob {
                                id,
                                url,
                                net,
                                recent,
                                theme,
                                post,
                            } = *job;
                            let ctx = LoadContext {
                                net,
                                fonts: fonts.clone(),
                                viewport: current_viewport(),
                                recent,
                                theme,
                                post,
                            };
                            let loaded = catch_unwind(AssertUnwindSafe(|| load_with(&url, &ctx)))
                                .unwrap_or_else(|e| {
                                    let err = engine_error(panic_message(&*e));
                                    internal::error_page(&url, &err, &ctx)
                                });
                            (id, Arc::new(loaded), Some(ctx))
                        }
                        Job::Relayout { id, loaded } => (id, loaded, None),
                    };
                    let viewport = current_viewport();
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
                    let (loaded, page) = match page(&loaded) {
                        Ok(p) => (loaded, p),
                        // Un error del motor con este documento: se muestra como página de
                        // error en lugar de terminar el hilo.
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
                                continue;
                            };
                            (shown, p)
                        }
                    };
                    let done = Done {
                        id,
                        loaded,
                        page,
                        viewport,
                    };
                    if tx.send(done).is_err() {
                        break;
                    }
                    wake();
                }
            })
            .expect("no se pudo crear el hilo de carga");
        Worker {
            jobs,
            done,
            viewport,
        }
    }

    pub fn send(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    pub fn set_viewport(&self, size: Size) {
        *self.viewport.lock().unwrap_or_else(|e| e.into_inner()) = size;
    }
}
