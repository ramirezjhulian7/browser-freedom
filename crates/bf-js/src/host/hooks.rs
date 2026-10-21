//! La interfaz con el embebedor ([`HostHooks`]): los servicios del entorno que el motor
//! necesita (reloj, consola, user agent, carga de módulos, reporte de errores y rechazos) y
//! las notificaciones para que el bucle de eventos del embebedor sepa cuándo despertar.
//!
//! Las colas de tareas y los temporizadores viven en el agente (`crate::host::event_loop`);
//! el embebedor solo aporta `now()` y decide cuándo llamar a `run_one_task`, usando
//! `next_wake()` y estas notificaciones.

use std::time::Instant;

use crate::heap::Gc;
use crate::object::Object;

/// Las *task sources* de HTML (§8.1.7.4) que usa el motor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskSource {
    DomManipulation,
    UserInteraction,
    Networking,
    Timers,
    PostedMessage,
    /// Cualquier otra fuente del embebedor.
    Other,
}

impl TaskSource {
    pub const ALL: [TaskSource; 6] = [
        TaskSource::DomManipulation,
        TaskSource::UserInteraction,
        TaskSource::Networking,
        TaskSource::Timers,
        TaskSource::PostedMessage,
        TaskSource::Other,
    ];

    pub(crate) fn index(self) -> usize {
        match self {
            TaskSource::DomManipulation => 0,
            TaskSource::UserInteraction => 1,
            TaskSource::Networking => 2,
            TaskSource::Timers => 3,
            TaskSource::PostedMessage => 4,
            TaskSource::Other => 5,
        }
    }
}

/// El nivel de un mensaje de `console` (Console Standard §1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleLevel {
    Log,
    Info,
    Warn,
    Error,
    Debug,
}

/// La operación de HostPromiseRejectionTracker (§27.2.1.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectionOperation {
    Reject,
    Handle,
}

/// Lo que el embebedor (el navegador) implementa. Todos los métodos tienen una
/// implementación por defecto razonable para un shell sin ventana.
pub trait HostHooks {
    /// El reloj monotónico en milisegundos (el `now()` de los temporizadores y de
    /// `performance.now()`).
    fn now(&mut self) -> f64 {
        start().elapsed().as_secs_f64() * 1000.0
    }

    /// Un mensaje de `console` ya formateado.
    fn console(&mut self, level: ConsoleLevel, message: &str) {
        match level {
            ConsoleLevel::Error | ConsoleLevel::Warn => eprintln!("{message}"),
            _ => println!("{message}"),
        }
    }

    /// Una excepción sin atrapar (HTML "report an exception", §8.1.4.6).
    fn report_exception(&mut self, message: &str, stack: &str) {
        eprintln!("Uncaught {message}");
        if !stack.is_empty() {
            eprint!("{stack}");
        }
    }

    /// HostPromiseRejectionTracker (§27.2.1.9).
    fn promise_rejection(&mut self, _promise: Gc<Object>, _operation: RejectionOperation) {}

    /// El valor de `navigator.userAgent`.
    fn user_agent(&self) -> String {
        bf_infra::identity::USER_AGENT.to_owned()
    }

    /// HostLoadImportedModule (§16.2.1.10): el texto del módulo en `url`.
    fn load_module(&mut self, url: &str) -> Result<String, String> {
        Err(format!("no se pueden cargar módulos: {url}"))
    }

    /// HostGetImportMetaProperties (§16.2.1.12): las propiedades de `import.meta`.
    fn import_meta(&mut self, url: &str) -> Vec<(String, String)> {
        vec![("url".to_owned(), url.to_owned())]
    }

    /// Se encoló una tarea en `source` (el embebedor puede despertar su bucle).
    fn task_queued(&mut self, _source: TaskSource) {}

    /// Se programó un temporizador que vence en `at` (milisegundos de `now()`).
    fn timer_scheduled(&mut self, _at: f64) {}

    /// Se pidió un animation frame (el embebedor debería actualizar el render).
    fn animation_frame_requested(&mut self) {}
}

fn start() -> Instant {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// Los hooks por defecto (reloj real, consola en stdout/stderr).
#[derive(Debug, Default)]
pub struct DefaultHooks;

impl HostHooks for DefaultHooks {}
