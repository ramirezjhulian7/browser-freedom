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

use crate::load::{Loaded, error_page, load_url};

pub enum Job {
    /// Cargar `url` y maquetarla.
    Load {
        id: u64,
        url: Url,
        net: Arc<NetLoader>,
    },
    /// Volver a hacer estilo y layout de un documento ya cargado (otro viewport).
    Relayout { id: u64, loaded: Arc<Loaded> },
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
                while let Ok(mut job) = rx.recv() {
                    // Solo importa el último trabajo pendiente.
                    while let Ok(next) = rx.try_recv() {
                        job = next;
                    }
                    let (id, loaded) = match job {
                        Job::Load { id, url, net } => {
                            let result =
                                catch_unwind(AssertUnwindSafe(|| load_url(&url, net.clone())))
                                    .unwrap_or_else(|e| Err(panic_message(&*e)));
                            let loaded = result.unwrap_or_else(|e| error_page(&url, &e, net));
                            (id, Arc::new(loaded))
                        }
                        Job::Relayout { id, loaded } => (id, loaded),
                    };
                    let viewport = *shared.lock().unwrap_or_else(|e| e.into_inner());
                    let page = |loaded: &Loaded| {
                        let ctx = PageContext {
                            base: &loaded.base,
                            encoding: loaded.encoding,
                            loader: loaded.loader.clone(),
                            fonts: fonts.clone(),
                            extra: &[],
                        };
                        catch_unwind(AssertUnwindSafe(|| {
                            prepare_page(&loaded.doc, &ctx, viewport)
                        }))
                    };
                    let (loaded, page) = match page(&loaded) {
                        Ok(p) => (loaded, p),
                        // Un error del motor con este documento: se muestra como página de
                        // error en lugar de terminar el hilo.
                        Err(e) => {
                            let message = format!("error del motor: {}", panic_message(&*e));
                            eprintln!("{message}");
                            let err = Arc::new(error_page(
                                &loaded.url,
                                &message,
                                loaded.loader.net.clone(),
                            ));
                            let Ok(p) = page(&err) else {
                                continue;
                            };
                            (err, p)
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
