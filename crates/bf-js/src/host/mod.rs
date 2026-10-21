//! Objetos host y API de incrustación: lo que el navegador usa para exponer el DOM y las
//! Web APIs a JavaScript.
//!
//! - [`HostObject`] (`ObjectKind::Host`): un objeto de plataforma con su clase, la clave de
//!   identidad del objeto Rust que envuelve ([`HostKey`]), su *payload* opaco y valores de
//!   JavaScript que retiene (trazados por el GC).
//! - [`HostClass`]: la descripción de una interfaz WebIDL (WebIDL §3.7): el registro crea
//!   en cada realm el objeto interfaz y el objeto prototipo encadenados con los del padre
//!   (`Node` → `Element` → `HTMLElement` → …), las operaciones como métodos enumerables, los
//!   atributos como accessors del prototipo, los `[LegacyUnforgeable]` como propiedades
//!   propias de cada instancia, las constantes y `@@toStringTag`. Las funciones del host
//!   pasan por un trampolín que hace el *brand check* (TypeError "Illegal invocation").
//! - Identidad de wrappers: una tabla débil `(realm, HostKey) → objeto`: el wrapper vive
//!   mientras lo alcance JavaScript o el host lo fije con [`HostHandle`].
//! - [`hooks`]: la interfaz con el embebedor; [`event_loop`]: tareas, temporizadores,
//!   microtasks y animation frames; [`webidl`]: conversiones; [`runtime`]: la fachada.

pub mod event_loop;
pub mod exotic;
pub mod hooks;
pub mod runtime;
pub mod webidl;

use std::any::Any;
use std::collections::HashMap;

use crate::builtins::slots;
use crate::heap::{Gc, Trace, Tracer};
use crate::object::{Object, ObjectKind, Prop, PropValue};
use crate::value::{PropertyKey, Value};
use crate::vm::function::{CallArgs, NativeFn};
use crate::vm::{Agent, JsResult, RealmId};

pub use event_loop::{TimerHandler, TimerId};
pub use exotic::HostExotic;
pub use hooks::{ConsoleLevel, DefaultHooks, HostHooks, RejectionOperation, TaskSource};
pub use runtime::{FromJs, IntoJs, JsError, Runtime};

/// El identificador de una clase host registrada.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HostClassId(pub u32);

/// La identidad del objeto del host que envuelve un wrapper (p. ej. el id de un nodo).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HostKey(pub u64);

/// Un valor fijado por el host: es raíz del GC hasta [`Agent::host_unpin`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HostHandle(u64);

/// Un objeto de plataforma.
pub struct HostObject {
    pub class: HostClassId,
    pub key: Option<HostKey>,
    pub payload: Box<dyn Any>,
    /// Valores de JavaScript que el objeto retiene (p. ej. los listeners de eventos).
    pub slots: Vec<Value>,
}

impl std::fmt::Debug for HostObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostObject")
            .field("class", &self.class)
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl Trace for HostObject {
    fn trace(&self, t: &mut Tracer) {
        for v in &self.slots {
            t.value(v);
        }
    }
}

/// Un atributo WebIDL: nombre, getter y setter (sin setter es `readonly`).
pub type HostAttribute = (&'static str, Option<NativeFn>, Option<NativeFn>);

/// Una operación WebIDL: nombre, `length` (los argumentos obligatorios) y la función.
pub type HostOperation = (&'static str, u32, NativeFn);

/// La descripción de una interfaz WebIDL.
#[derive(Clone)]
pub struct HostClass {
    pub name: &'static str,
    pub parent: Option<HostClassId>,
    /// El constructor (`[[Construct]]` del objeto interfaz) con su `length`; sin él, el
    /// objeto interfaz lanza "Illegal constructor".
    pub constructor: Option<(NativeFn, u32)>,
    pub methods: Vec<HostOperation>,
    pub attributes: Vec<HostAttribute>,
    /// Atributos `[LegacyUnforgeable]`: propiedades propias no configurables de cada
    /// instancia.
    pub unforgeable: Vec<HostAttribute>,
    pub constants: Vec<(&'static str, f64)>,
    pub static_methods: Vec<HostOperation>,
    pub static_attributes: Vec<HostAttribute>,
    /// `[Symbol.hasInstance]` propio del objeto interfaz.
    pub has_instance: Option<NativeFn>,
    /// Propiedades indexadas y con nombre (objeto de plataforma *legacy*, WebIDL §3.9).
    pub exotic: Option<HostExotic>,
    /// ¿Se expone como propiedad del objeto global? (`[Exposed]`.)
    pub exposed: bool,
}

impl HostClass {
    pub fn new(name: &'static str) -> HostClass {
        HostClass {
            name,
            parent: None,
            constructor: None,
            methods: Vec::new(),
            attributes: Vec::new(),
            unforgeable: Vec::new(),
            constants: Vec::new(),
            static_methods: Vec::new(),
            static_attributes: Vec::new(),
            has_instance: None,
            exotic: None,
            exposed: true,
        }
    }

    pub fn parent(mut self, p: HostClassId) -> HostClass {
        self.parent = Some(p);
        self
    }

    pub fn constructor(mut self, f: NativeFn, length: u32) -> HostClass {
        self.constructor = Some((f, length));
        self
    }

    pub fn method(mut self, name: &'static str, length: u32, f: NativeFn) -> HostClass {
        self.methods.push((name, length, f));
        self
    }

    pub fn attribute(
        mut self,
        name: &'static str,
        get: Option<NativeFn>,
        set: Option<NativeFn>,
    ) -> HostClass {
        self.attributes.push((name, get, set));
        self
    }

    pub fn unforgeable(
        mut self,
        name: &'static str,
        get: Option<NativeFn>,
        set: Option<NativeFn>,
    ) -> HostClass {
        self.unforgeable.push((name, get, set));
        self
    }

    pub fn constant(mut self, name: &'static str, value: f64) -> HostClass {
        self.constants.push((name, value));
        self
    }

    pub fn static_method(mut self, name: &'static str, length: u32, f: NativeFn) -> HostClass {
        self.static_methods.push((name, length, f));
        self
    }

    pub fn static_attribute(
        mut self,
        name: &'static str,
        get: Option<NativeFn>,
        set: Option<NativeFn>,
    ) -> HostClass {
        self.static_attributes.push((name, get, set));
        self
    }

    pub fn has_instance(mut self, f: NativeFn) -> HostClass {
        self.has_instance = Some(f);
        self
    }

    /// Getters indexado y con nombre (WebIDL §3.9).
    pub fn exotic(mut self, exotic: HostExotic) -> HostClass {
        self.exotic = Some(exotic);
        self
    }

    pub fn exposed(mut self, exposed: bool) -> HostClass {
        self.exposed = exposed;
        self
    }
}

/// Los objetos de una clase en un realm.
#[derive(Debug, Clone, Copy)]
pub struct ClassObjects {
    pub prototype: Gc<Object>,
    pub interface: Gc<Object>,
}

/// El estado del host en el agente.
pub struct HostState {
    pub hooks: Box<dyn HostHooks>,
    pub(crate) classes: Vec<HostClass>,
    /// Por realm, los objetos de cada clase ya creada.
    pub(crate) realm_classes: Vec<Vec<Option<ClassObjects>>>,
    /// Las funciones del host detrás del trampolín.
    pub(crate) fns: Vec<NativeFn>,
    /// Identidad de wrappers (referencias débiles: se purgan tras cada recolección).
    wrappers: HashMap<(RealmId, HostKey), Gc<Object>>,
    /// Valores fijados por el host.
    pins: HashMap<u64, Value>,
    next_pin: u64,
    /// Valores que usa el código del host mientras corre (raíces).
    pub(crate) running: Vec<Value>,
    pub(crate) event_loop: event_loop::EventLoop,
    /// Datos del embebedor por tipo (para todo el agente).
    embedder: HashMap<std::any::TypeId, Box<dyn Any>>,
    /// Datos del embebedor por realm y tipo (p. ej. el `WebContext` de cada ventana).
    realm_data: HashMap<(RealmId, std::any::TypeId), Box<dyn Any>>,
}

impl Default for HostState {
    fn default() -> HostState {
        HostState {
            hooks: Box::new(DefaultHooks),
            classes: Vec::new(),
            realm_classes: Vec::new(),
            fns: Vec::new(),
            wrappers: HashMap::new(),
            pins: HashMap::new(),
            next_pin: 1,
            running: Vec::new(),
            event_loop: event_loop::EventLoop::default(),
            embedder: HashMap::new(),
            realm_data: HashMap::new(),
        }
    }
}

impl Trace for HostState {
    fn trace(&self, t: &mut Tracer) {
        for objs in self.realm_classes.iter().flatten().flatten() {
            t.gc(objs.prototype);
            t.gc(objs.interface);
        }
        for v in self.pins.values().chain(self.running.iter()) {
            t.value(v);
        }
        self.event_loop.trace(t);
    }
}

/// Tras cada recolección: se olvidan los wrappers que murieron.
pub(crate) fn after_gc(agent: &mut Agent) {
    let heap = &agent.heap;
    agent.host.wrappers.retain(|_, o| heap.is_alive(*o));
}

/// Al crear un realm: los objetos de todas las clases registradas.
pub(crate) fn on_realm_created(agent: &mut Agent, realm: RealmId) {
    for i in 0..agent.host.classes.len() {
        instantiate(agent, realm, HostClassId(i as u32));
    }
}

// --- Trampolines ---

/// Cómo se invoca una función del host.
const CALL_METHOD: f64 = 0.0;
const CALL_STATIC: f64 = 1.0;

/// Las operaciones, getters y setters: brand check y la función del host.
fn trampoline(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = slots(agent, a.callee);
    let idx = s[0].as_number().unwrap_or(0.0) as usize;
    let class = HostClassId(s[1].as_number().unwrap_or(0.0) as u32);
    if s[2].as_number() == Some(CALL_METHOD) && !agent.host_is_instance(&a.this, class) {
        return agent.type_error("Illegal invocation");
    }
    let f = agent.host.fns[idx];
    f(agent, a)
}

/// `[[Call]]`/`[[Construct]]` del objeto interfaz (WebIDL §3.7.1).
fn interface_trampoline(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = slots(agent, a.callee);
    let class = s[0].as_number().unwrap_or(0.0) as usize;
    let (name, ctor) = {
        let c = &agent.host.classes[class];
        (c.name, c.constructor)
    };
    if a.new_target.is_none() {
        return agent.type_error(&format!(
            "Failed to construct '{name}': Please use the 'new' operator"
        ));
    }
    match ctor {
        Some((f, _)) => f(agent, a),
        None => agent.type_error("Illegal constructor"),
    }
}

fn host_function(
    agent: &mut Agent,
    f: NativeFn,
    length: u32,
    name: &str,
    class: HostClassId,
    mode: f64,
) -> Gc<Object> {
    let idx = agent.host.fns.len();
    agent.host.fns.push(f);
    agent.create_builtin_with_slots(
        trampoline,
        length,
        name,
        vec![
            Value::Number(idx as f64),
            Value::Number(f64::from(class.0)),
            Value::Number(mode),
        ],
    )
}

/// Define un atributo como accessor `{enumerable, configurable}` (WebIDL §3.7.6).
fn define_attribute(
    agent: &mut Agent,
    target: Gc<Object>,
    attr: &HostAttribute,
    class: HostClassId,
    mode: f64,
    configurable: bool,
) {
    let (name, get, set) = *attr;
    let g = get.map(|f| host_function(agent, f, 0, &format!("get {name}"), class, mode));
    let s = set.map(|f| host_function(agent, f, 1, &format!("set {name}"), class, mode));
    agent.heap[target].props.insert(
        PropertyKey::from(name),
        Prop {
            value: PropValue::Accessor { get: g, set: s },
            enumerable: true,
            configurable,
        },
    );
}

/// Crea (si falta) la interfaz y el prototipo de `class` en `realm`.
fn instantiate(agent: &mut Agent, realm: RealmId, class: HostClassId) -> ClassObjects {
    while agent.host.realm_classes.len() <= realm {
        agent.host.realm_classes.push(Vec::new());
    }
    let ci = class.0 as usize;
    if let Some(Some(o)) = agent.host.realm_classes[realm].get(ci) {
        return *o;
    }
    let def = agent.host.classes[ci].clone();
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let (parent_proto, parent_iface) = match def.parent {
        Some(p) => {
            let po = instantiate(agent, realm, p);
            (po.prototype, po.interface)
        }
        None => {
            let i = agent.intrinsics();
            (i.object_prototype, i.function_prototype)
        }
    };
    // El objeto prototipo de la interfaz (WebIDL §3.7.3).
    let proto = agent.new_object(Some(parent_proto));
    // El objeto interfaz (WebIDL §3.7.1).
    let length = def.constructor.map_or(0, |(_, l)| l);
    let iface = agent.create_builtin_function(interface_trampoline, length, def.name, true);
    if let ObjectKind::Builtin(b) = &mut agent.heap[iface].kind {
        b.slots = vec![Value::Number(ci as f64)];
    }
    agent.heap[iface].proto = Some(parent_iface);
    let kp = agent.keys.prototype.clone();
    agent.put_data(iface, kp, Value::Object(proto), false, false, false);
    let kc = agent.keys.constructor.clone();
    agent.put_data(proto, kc, Value::Object(iface), true, false, true);
    let tag = PropertyKey::Symbol(agent.well_known.to_string_tag);
    agent.put_data(proto, tag, Value::str(def.name), false, false, true);
    for &(name, length, f) in &def.methods {
        let m = host_function(agent, f, length, name, class, CALL_METHOD);
        agent.put_data(
            proto,
            PropertyKey::from(name),
            Value::Object(m),
            true,
            true,
            true,
        );
    }
    for attr in &def.attributes {
        define_attribute(agent, proto, attr, class, CALL_METHOD, true);
    }
    for &(name, v) in &def.constants {
        agent.put_data(
            iface,
            PropertyKey::from(name),
            Value::Number(v),
            false,
            true,
            false,
        );
        agent.put_data(
            proto,
            PropertyKey::from(name),
            Value::Number(v),
            false,
            true,
            false,
        );
    }
    for &(name, length, f) in &def.static_methods {
        let m = host_function(agent, f, length, name, class, CALL_STATIC);
        agent.put_data(
            iface,
            PropertyKey::from(name),
            Value::Object(m),
            true,
            true,
            true,
        );
    }
    for attr in &def.static_attributes {
        define_attribute(agent, iface, attr, class, CALL_STATIC, true);
    }
    if let Some(f) = def.has_instance {
        let m = host_function(agent, f, 1, "[Symbol.hasInstance]", class, CALL_STATIC);
        let k = PropertyKey::Symbol(agent.well_known.has_instance);
        agent.put_data(iface, k, Value::Object(m), false, false, false);
    }
    if def.exposed {
        let g = agent.realms[realm].global_object;
        agent.put_data(
            g,
            PropertyKey::from(def.name),
            Value::Object(iface),
            true,
            false,
            true,
        );
    }
    agent.current_realm = saved;
    let objs = ClassObjects {
        prototype: proto,
        interface: iface,
    };
    let list = &mut agent.host.realm_classes[realm];
    while list.len() <= ci {
        list.push(None);
    }
    list[ci] = Some(objs);
    objs
}

impl Agent {
    /// Registra una interfaz y crea sus objetos en todos los realms existentes.
    pub fn register_host_class(&mut self, class: HostClass) -> HostClassId {
        if let Some(p) = class.parent {
            assert!(
                (p.0 as usize) < self.host.classes.len(),
                "la clase padre debe estar registrada antes"
            );
        }
        let id = HostClassId(self.host.classes.len() as u32);
        self.host.classes.push(class);
        for realm in 0..self.realms.len() {
            instantiate(self, realm, id);
        }
        id
    }

    /// Los objetos (prototipo e interfaz) de `class` en `realm`.
    pub fn host_class_objects(&mut self, realm: RealmId, class: HostClassId) -> ClassObjects {
        instantiate(self, realm, class)
    }

    /// El nombre de la clase.
    pub fn host_class_name(&self, class: HostClassId) -> &'static str {
        self.host.classes[class.0 as usize].name
    }

    /// ¿`class` es `ancestor` o hereda de ella?
    pub fn host_class_inherits(&self, mut class: HostClassId, ancestor: HostClassId) -> bool {
        loop {
            if class == ancestor {
                return true;
            }
            match self.host.classes[class.0 as usize].parent {
                Some(p) => class = p,
                None => return false,
            }
        }
    }

    /// ¿`v` es una instancia de la interfaz `class` (o de una que hereda de ella)?
    pub fn host_is_instance(&self, v: &Value, class: HostClassId) -> bool {
        match v {
            Value::Object(o) => match &self.heap[*o].kind {
                ObjectKind::Host(h) => self.host_class_inherits(h.class, class),
                _ => false,
            },
            _ => false,
        }
    }

    /// La clase de un objeto host.
    pub fn host_class_of(&self, v: &Value) -> Option<HostClassId> {
        match v {
            Value::Object(o) => match &self.heap[*o].kind {
                ObjectKind::Host(h) => Some(h.class),
                _ => None,
            },
            _ => None,
        }
    }

    /// La clave de identidad de un objeto host.
    pub fn host_key_of(&self, v: &Value) -> Option<HostKey> {
        match v {
            Value::Object(o) => match &self.heap[*o].kind {
                ObjectKind::Host(h) => h.key,
                _ => None,
            },
            _ => None,
        }
    }

    fn new_host_object(
        &mut self,
        proto: Gc<Object>,
        class: HostClassId,
        key: Option<HostKey>,
        payload: Box<dyn Any>,
    ) -> Gc<Object> {
        let o = self.alloc(Object::new(
            Some(proto),
            ObjectKind::Host(Box::new(HostObject {
                class,
                key,
                payload,
                slots: Vec::new(),
            })),
        ));
        self.define_unforgeables(o, class);
        o
    }

    /// Los atributos `[LegacyUnforgeable]` de la clase y sus ancestros como propiedades
    /// propias no configurables (WebIDL §3.7.7).
    fn define_unforgeables(&mut self, o: Gc<Object>, class: HostClassId) {
        let mut chain = Vec::new();
        let mut c = Some(class);
        while let Some(id) = c {
            chain.push(id);
            c = self.host.classes[id.0 as usize].parent;
        }
        for id in chain.into_iter().rev() {
            let attrs = self.host.classes[id.0 as usize].unforgeable.clone();
            for attr in &attrs {
                define_attribute(self, o, attr, id, CALL_METHOD, false);
            }
        }
    }

    /// El wrapper de `key` en `realm`, si existe todavía.
    pub fn host_lookup(&self, realm: RealmId, key: HostKey) -> Option<Gc<Object>> {
        self.host
            .wrappers
            .get(&(realm, key))
            .copied()
            .filter(|o| self.heap.is_alive(*o))
    }

    /// El wrapper de `key` en `realm`: el existente (y `payload` se descarta) o uno nuevo de
    /// la clase `class`.
    pub fn host_wrap(
        &mut self,
        realm: RealmId,
        key: HostKey,
        class: HostClassId,
        payload: Box<dyn Any>,
    ) -> Gc<Object> {
        if let Some(o) = self.host_lookup(realm, key) {
            return o;
        }
        let proto = instantiate(self, realm, class).prototype;
        let saved = self.current_realm;
        self.current_realm = realm;
        let o = self.new_host_object(proto, class, Some(key), payload);
        self.current_realm = saved;
        self.host.wrappers.insert((realm, key), o);
        o
    }

    /// Un objeto host sin identidad (p. ej. el resultado de un constructor) en el realm
    /// actual.
    pub fn host_create(&mut self, class: HostClassId, payload: Box<dyn Any>) -> Gc<Object> {
        let realm = self.current_realm;
        let proto = instantiate(self, realm, class).prototype;
        self.new_host_object(proto, class, None, payload)
    }

    /// Un objeto host para `[[Construct]]` con `new_target` (el prototipo sale de
    /// `newTarget.prototype`, con el de la clase en el realm de `new_target` por defecto,
    /// como en las subclases de clases host).
    pub fn host_create_from_new_target(
        &mut self,
        new_target: Gc<Object>,
        class: HostClassId,
        key: Option<HostKey>,
        payload: Box<dyn Any>,
    ) -> JsResult<Gc<Object>> {
        let k = self.keys.prototype.clone();
        let p = self.get(new_target, &k, Value::Object(new_target))?;
        let proto = match p {
            Value::Object(p) => p,
            _ => {
                let realm = self.get_function_realm(new_target)?;
                instantiate(self, realm, class).prototype
            }
        };
        let o = self.new_host_object(proto, class, key, payload);
        if let Some(key) = key {
            let realm = self.current_realm;
            self.host.wrappers.insert((realm, key), o);
        }
        Ok(o)
    }

    /// El payload de un objeto host, si es de tipo `T`.
    pub fn host_unwrap<T: 'static>(&self, v: &Value) -> Option<&T> {
        match v {
            Value::Object(o) => match &self.heap[*o].kind {
                ObjectKind::Host(h) => h.payload.downcast_ref::<T>(),
                _ => None,
            },
            _ => None,
        }
    }

    /// El payload mutable de un objeto host, si es de tipo `T`.
    pub fn host_unwrap_mut<T: 'static>(&mut self, v: &Value) -> Option<&mut T> {
        match v {
            Value::Object(o) => match &mut self.heap[*o].kind {
                ObjectKind::Host(h) => h.payload.downcast_mut::<T>(),
                _ => None,
            },
            _ => None,
        }
    }

    /// Los valores de JavaScript que retiene un objeto host (trazados).
    pub fn host_slots_mut(&mut self, o: Gc<Object>) -> Option<&mut Vec<Value>> {
        match &mut self.heap[o].kind {
            ObjectKind::Host(h) => Some(&mut h.slots),
            _ => None,
        }
    }

    /// Fija `v` como raíz del GC hasta [`Agent::host_unpin`].
    pub fn host_pin(&mut self, v: Value) -> HostHandle {
        let id = self.host.next_pin;
        self.host.next_pin += 1;
        self.host.pins.insert(id, v);
        HostHandle(id)
    }

    /// El valor fijado por `h`.
    pub fn host_pinned(&self, h: HostHandle) -> Option<Value> {
        self.host.pins.get(&h.0).cloned()
    }

    /// Libera un valor fijado.
    pub fn host_unpin(&mut self, h: HostHandle) -> Option<Value> {
        self.host.pins.remove(&h.0)
    }

    /// Guarda un dato del embebedor para todo el agente (uno por tipo).
    pub fn set_embedder_data<T: Any>(&mut self, data: T) {
        self.host
            .embedder
            .insert(std::any::TypeId::of::<T>(), Box::new(data));
    }

    /// El dato del embebedor de tipo `T`.
    pub fn embedder_data<T: Any>(&self) -> Option<&T> {
        self.host
            .embedder
            .get(&std::any::TypeId::of::<T>())
            .and_then(|b| b.downcast_ref::<T>())
    }

    pub fn embedder_data_mut<T: Any>(&mut self) -> Option<&mut T> {
        self.host
            .embedder
            .get_mut(&std::any::TypeId::of::<T>())
            .and_then(|b| b.downcast_mut::<T>())
    }

    /// Guarda un dato del embebedor para `realm` (uno por tipo y realm).
    pub fn set_realm_data<T: Any>(&mut self, realm: RealmId, data: T) {
        self.host
            .realm_data
            .insert((realm, std::any::TypeId::of::<T>()), Box::new(data));
    }

    /// El dato de tipo `T` de `realm`.
    pub fn realm_data<T: Any>(&self, realm: RealmId) -> Option<&T> {
        self.host
            .realm_data
            .get(&(realm, std::any::TypeId::of::<T>()))
            .and_then(|b| b.downcast_ref::<T>())
    }

    pub fn realm_data_mut<T: Any>(&mut self, realm: RealmId) -> Option<&mut T> {
        self.host
            .realm_data
            .get_mut(&(realm, std::any::TypeId::of::<T>()))
            .and_then(|b| b.downcast_mut::<T>())
    }

    /// Saca el dato de tipo `T` de `realm` (para devolverlo con `set_realm_data`).
    pub fn take_realm_data<T: Any>(&mut self, realm: RealmId) -> Option<T> {
        self.host
            .realm_data
            .remove(&(realm, std::any::TypeId::of::<T>()))
            .and_then(|b| b.downcast::<T>().ok())
            .map(|b| *b)
    }

    /// Convierte el objeto global de `realm` en un objeto host de `class` (p. ej. `Window`,
    /// HTML §7.3): conserva sus propiedades y su prototipo pasa a ser el de la clase.
    pub fn host_make_global(
        &mut self,
        realm: RealmId,
        class: HostClassId,
        payload: Box<dyn Any>,
    ) -> Gc<Object> {
        let proto = instantiate(self, realm, class).prototype;
        let g = self.realms[realm].global_object;
        self.heap[g].kind = ObjectKind::Host(Box::new(HostObject {
            class,
            key: None,
            payload,
            slots: Vec::new(),
        }));
        self.heap[g].proto = Some(proto);
        let saved = self.current_realm;
        self.current_realm = realm;
        self.define_unforgeables(g, class);
        self.current_realm = saved;
        g
    }

    /// Cambia los hooks del embebedor.
    pub fn set_host_hooks(&mut self, hooks: Box<dyn HostHooks>) {
        self.host.hooks = hooks;
    }
}
