//! El navegador con sus ventanas (E11): el bucle de eventos de winit reparte cada evento a la
//! ventana ([`App`]) que le toca. Todas comparten la red (las cookies, que viven en memoria)
//! y los almacenes del perfil; cada una tiene sus pestañas y sus hilos de documento.
//!
//! También guarda la sesión ([`crate::restore`]) cuando algo cambia y la restaura al abrir si
//! la opción `restore_session` está activa. Solo la guarda el navegador que se abrió sin URL:
//! `freedom <url>` (pruebas, abrir un archivo) ni restaura ni pisa la sesión de la última vez.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bf_font::FontDb;
use bf_infra::Url;
use bf_net::NetLoader;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::window::WindowId;

use super::worker::Documents;
use super::{App, Theme};
use crate::restore::{SavedSession, SavedWindow};

/// Evento de usuario del bucle: algo terminó en otro hilo.
#[derive(Debug, Clone, Copy)]
pub(super) enum Wake {
    /// Un hilo de documento tiene algo para su ventana.
    Documents,
    /// Una descarga avanzó o terminó (la barra de descargas se redibuja).
    Downloads,
}

/// Lo que una ventana le pide al navegador.
pub(super) enum WindowRequest {
    /// Abrir una ventana nueva con `url` (o la página de inicio).
    NewWindow(Option<Url>),
    /// Una pestaña cambió el `localStorage` de un origen: las de las otras ventanas reciben
    /// `storage` (HTML §12.2.1).
    Storage(crate::script::StorageNotice),
}

pub(super) struct Browser {
    windows: Vec<App>,
    fonts: Arc<FontDb>,
    net: Arc<NetLoader>,
    proxy: Arc<Mutex<EventLoopProxy<Wake>>>,
    /// La URL de la línea de comandos (para la primera ventana).
    initial: Option<Url>,
    /// Si este navegador restaura y guarda la sesión (se abrió sin URL).
    keeps_session: bool,
    /// Lo último que se escribió en el archivo de la sesión.
    last_saved: String,
    started: bool,
}

impl Browser {
    pub(super) fn new(proxy: EventLoopProxy<Wake>, initial: Option<Url>) -> Browser {
        Browser {
            windows: Vec::new(),
            fonts: FontDb::system(),
            net: App::shared_net(),
            proxy: Arc::new(Mutex::new(proxy)),
            keeps_session: initial.is_none(),
            initial,
            last_saved: String::new(),
            started: false,
        }
    }

    fn waker(&self, wake: Wake) -> Arc<dyn Fn() + Send + Sync> {
        let proxy = self.proxy.clone();
        Arc::new(move || {
            let _ = proxy
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .send_event(wake);
        })
    }

    /// Abre una ventana con `url` (o la página de inicio), o con las pestañas de `saved`.
    fn open_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        url: Option<Url>,
        saved: Option<SavedWindow>,
    ) {
        let wake = self.waker(Wake::Documents);
        let worker = Documents::new(self.fonts.clone(), move || wake());
        let mut app = App::new(
            worker,
            self.fonts.clone(),
            url,
            Theme::for_system(None),
            self.net.clone(),
        );
        app.download_wake = self.waker(Wake::Downloads);
        if !app.create_window(event_loop) {
            if self.windows.is_empty() {
                event_loop.exit();
            }
            return;
        }
        app.start(saved);
        self.windows.push(app);
    }

    fn index_of(&self, id: WindowId) -> Option<usize> {
        self.windows.iter().position(|a| a.window_id() == Some(id))
    }

    /// Atiende lo que pidieron las ventanas.
    fn serve_requests(&mut self, event_loop: &ActiveEventLoop) {
        let mut pending = Vec::new();
        for (i, app) in self.windows.iter_mut().enumerate() {
            pending.extend(app.requests.drain(..).map(|r| (i, r)));
        }
        for (from, request) in pending {
            match request {
                WindowRequest::NewWindow(url) => self.open_window(event_loop, url, None),
                WindowRequest::Storage(notice) => {
                    for (i, app) in self.windows.iter_mut().enumerate() {
                        if i != from {
                            app.storage_changed_elsewhere(&notice);
                        }
                    }
                }
            }
        }
    }

    /// Guarda la sesión si cambió algo.
    fn save_session(&mut self, force: bool) {
        // Se limpian todas las marcas (sin cortar en la primera).
        let mut dirty = false;
        for app in &mut self.windows {
            dirty |= std::mem::take(&mut app.session_dirty);
        }
        if !self.keeps_session || !(dirty || force) {
            return;
        }
        let session = SavedSession {
            windows: self.windows.iter().filter_map(App::saved_window).collect(),
        };
        let text = session.serialize();
        if text != self.last_saved {
            session.save();
            self.last_saved = text;
        }
    }
}

/// El mensaje de un pánico.
fn panic_message(e: &(dyn std::any::Any + Send)) -> String {
    e.downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| e.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "error interno".to_owned())
}

/// E6: corre `f` (un evento del bucle) capturando un pánico; si lo hubo, llama a `on_panic`
/// con el mensaje.
fn guarded(f: impl FnOnce(), on_panic: impl FnOnce(String)) {
    if let Err(e) = catch_unwind(AssertUnwindSafe(f)) {
        on_panic(panic_message(&*e));
    }
}

impl Browser {
    /// E6: un pánico en el hilo de la ventana. El estado de las ventanas puede haber quedado
    /// a medias (un pánico dentro de `App::with_tab` deja intercambiado el estado de dos
    /// pestañas), así que seguir no es seguro. Se guarda la sesión con lo que haya, se cierran
    /// los hilos de documento y se sale del bucle; al abrir de nuevo se restaura la sesión.
    /// Cada paso va protegido: un segundo pánico no impide salir.
    fn crashed(&mut self, event_loop: &ActiveEventLoop, message: String) {
        eprintln!(
            "Freedom: error interno en la ventana ({message}); se guarda la sesión y se cierra"
        );
        let _ = catch_unwind(AssertUnwindSafe(|| self.save_session(true)));
        for app in &mut self.windows {
            let _ = catch_unwind(AssertUnwindSafe(|| app.shutdown()));
        }
        event_loop.exit();
    }
}

impl ApplicationHandler<Wake> for Browser {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let mut crash = None;
        guarded(|| self.resumed_inner(event_loop), |m| crash = Some(m));
        if let Some(m) = crash {
            self.crashed(event_loop, m);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Wake) {
        let mut crash = None;
        guarded(
            || self.user_event_inner(event_loop, event),
            |m| crash = Some(m),
        );
        if let Some(m) = crash {
            self.crashed(event_loop, m);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let mut crash = None;
        guarded(|| self.about_to_wait_inner(event_loop), |m| crash = Some(m));
        if let Some(m) = crash {
            self.crashed(event_loop, m);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let mut crash = None;
        guarded(
            || self.window_event_inner(event_loop, id, event),
            |m| crash = Some(m),
        );
        if let Some(m) = crash {
            self.crashed(event_loop, m);
        }
    }
}

impl Browser {
    fn resumed_inner(&mut self, event_loop: &ActiveEventLoop) {
        if std::mem::replace(&mut self.started, true) {
            return;
        }
        let restore = self.keeps_session && crate::settings::global().restore_session;
        let session = if restore {
            SavedSession::load()
        } else {
            SavedSession::default()
        };
        if session.is_empty() {
            let url = self.initial.take();
            self.open_window(event_loop, url, None);
        } else {
            for w in session.windows {
                self.open_window(event_loop, None, Some(w));
            }
        }
        if self.windows.is_empty() {
            event_loop.exit();
        }
    }

    fn user_event_inner(&mut self, event_loop: &ActiveEventLoop, event: Wake) {
        match event {
            Wake::Documents => {
                for app in &mut self.windows {
                    app.receive();
                }
            }
            Wake::Downloads => {
                for app in &mut self.windows {
                    app.downloads_changed();
                }
            }
        }
        self.serve_requests(event_loop);
    }

    fn about_to_wait_inner(&mut self, event_loop: &ActiveEventLoop) {
        self.save_session(false);
        let next: Option<Instant> = self.windows.iter_mut().filter_map(App::next_deadline).min();
        event_loop.set_control_flow(match next {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }

    fn window_event_inner(
        &mut self,
        event_loop: &ActiveEventLoop,
        id: WindowId,
        event: WindowEvent,
    ) {
        let Some(i) = self.index_of(id) else {
            return;
        };
        if matches!(event, WindowEvent::CloseRequested) {
            if self.windows.len() == 1 {
                // La última ventana: la sesión queda como estaba al cerrarla.
                self.save_session(true);
                self.windows[0].shutdown();
                event_loop.exit();
            } else {
                let mut app = self.windows.remove(i);
                app.shutdown();
                self.save_session(true);
            }
            return;
        }
        self.windows[i].window_event(event);
        self.serve_requests(event_loop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarded_reports_panics() {
        let mut got = None;
        guarded(|| panic!("fallo de prueba"), |m| got = Some(m));
        assert_eq!(got.as_deref(), Some("fallo de prueba"));
        let mut called = false;
        let mut ran = false;
        guarded(|| ran = true, |_| called = true);
        assert!(ran && !called);
    }
}
