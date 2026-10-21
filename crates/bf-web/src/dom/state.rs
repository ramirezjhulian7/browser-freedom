//! El estado de los bindings del DOM de una ventana (realm data `DomState`): los listeners de
//! script, los event handlers, el evento actual, los `MutationObserver` y lo que retienen.

use std::collections::{HashMap, HashSet};

use bf_dom::events::EventTarget;
use bf_infra::DomString;
use bf_js::heap::Gc;
use bf_js::host::HostHandle;
use bf_js::object::Object;
use bf_js::vm::Agent;

use crate::{DocIndex, WebContextHandle};

/// Un listener de script registrado en `bf-dom` como `Listener::Script(id)`.
#[derive(Debug, Clone)]
pub enum ListenerRec {
    /// Un callback (función u objeto con `handleEvent`).
    Callback(HostHandle),
    /// El listener de un event handler (HTML §8.1.8.1): busca el valor actual del handler.
    Handler {
        doc: DocIndex,
        target: EventTarget,
        type_: DomString,
    },
}

/// El valor de un event handler.
#[derive(Debug, Clone, Default)]
pub struct HandlerState {
    /// El valor puesto por el atributo IDL (`None`: no se puso o se anuló con el de contenido).
    pub idl: Option<Option<HostHandle>>,
    /// El último texto del atributo de contenido compilado y su función.
    pub compiled: Option<(DomString, Option<HostHandle>)>,
}

#[derive(Default)]
pub struct DomState {
    pub listeners: HashMap<u64, ListenerRec>,
    /// El id del listener de cada callback (para que sumar dos veces el mismo no lo duplique).
    pub listener_ids: HashMap<Gc<Object>, u64>,
    pub next_listener: u64,
    /// Los event handlers por (documento, target, tipo).
    pub handlers: HashMap<(DocIndex, EventTarget, DomString), HandlerState>,
    /// Los handlers que ya tienen su listener registrado.
    pub handler_listeners: HashSet<(DocIndex, EventTarget, DomString)>,
    /// `window.event` (HTML §8.1.8.4, "current event").
    pub current_event: Vec<HostHandle>,
    /// ¿Se está reportando una excepción? (Evita recursión con `error`.)
    pub reporting: bool,
    /// Los callbacks de los `MutationObserver` por (documento, id).
    pub observers: HashMap<(DocIndex, u32), (HostHandle, HostHandle)>,
    /// ¿Hay un microtask de entrega de mutation records pendiente?
    pub observer_microtask: bool,
    /// Valores de script retenidos por `bf-dom` como handles opacos (`CustomEvent.detail`,
    /// razones de abort).
    pub values: HashMap<u64, HostHandle>,
    pub next_value: u64,
    /// Para los handlers de ventana: el `body`/`frameset` cuyo atributo `on…` cambió por
    /// último desde script (por tipo de evento).
    pub window_handler_source: HashMap<DomString, bf_dom::NodeId>,
    /// Los documentos de `new Document()` (su interfaz es `Document`, no `XMLDocument`).
    pub plain_documents: HashSet<DocIndex>,
    /// Los wrappers de los eventos nativos en dispatch, por (dirección, `timeStamp`).
    pub native_events: Vec<((usize, u64), HostHandle)>,
}

/// El estado del DOM de la ventana actual.
pub fn state(agent: &mut Agent) -> &mut DomState {
    let realm = WebContextHandle::current(agent).realm;
    if agent.realm_data::<DomState>(realm).is_none() {
        agent.set_realm_data(realm, DomState::default());
    }
    agent.realm_data_mut::<DomState>(realm).expect("DomState")
}
