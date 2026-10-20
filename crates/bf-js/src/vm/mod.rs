//! La máquina virtual: el Agent (§9.7) con su heap, sus realms (§9.3), la pila de execution
//! contexts (§9.4, los [`Frame`]s del intérprete) y las operaciones de ejecución.
//!
//! Recolección de basura: el GC (ver [`crate::heap`]) solo corre en puntos seguros del
//! intérprete (saltos hacia atrás y entradas de función) y únicamente cuando ningún código
//! Rust con valores sin enraizar está en la pila: las llamadas de Rust a JavaScript que hacen
//! las operaciones abstractas y los built-ins ([`Agent::call`]) suben el contador
//! `gc_inhibit`; el intérprete, que mantiene en la pila de la VM todo lo que usa, llama sin
//! subirlo. Las raíces son explícitas: los realms (intrínsecos, objeto global, environment
//! global, templates), los frames (pila, locales, environments, handlers, `this`,
//! argumentos), el registro de símbolos y los [`crate::heap::HandleScope`] abiertos.

pub mod env;
pub mod env_ops;
pub mod eval;
pub mod frame;
pub mod function;
pub mod generator;
pub mod interp;
pub mod module; // E1
pub mod realm;

use std::collections::HashMap;
use std::time::Instant;

use crate::heap::{Gc, GcStats, Heap, Trace, Tracer};
use crate::object::{Object, ObjectKind, Prop};
use crate::string::JsString;
use crate::value::{PropertyKey, Symbol, Value};
use frame::Frame;
use realm::{Intrinsics, Realm};

pub type RealmId = usize;

/// Una completion abrupta que sube por Rust: una excepción de JavaScript o la terminación
/// del agente (límite de tiempo o de instrucciones), que ningún `catch` atrapa.
#[derive(Debug, Clone)]
pub enum Throw {
    Value(Value),
    Terminate(String),
}

pub type JsResult<T> = Result<T, Throw>;

/// Los constructores de error nativos (§20.5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorType {
    Error,
    EvalError,
    RangeError,
    ReferenceError,
    SyntaxError,
    TypeError,
    UriError,
}

/// Los símbolos bien conocidos (§6.1.5.1), compartidos por todos los realms.
#[derive(Debug, Clone, Copy)]
pub struct WellKnown {
    pub async_iterator: Gc<Symbol>,
    pub has_instance: Gc<Symbol>,
    pub is_concat_spreadable: Gc<Symbol>,
    pub iterator: Gc<Symbol>,
    pub match_: Gc<Symbol>,
    pub match_all: Gc<Symbol>,
    pub replace: Gc<Symbol>,
    pub search: Gc<Symbol>,
    pub species: Gc<Symbol>,
    pub split: Gc<Symbol>,
    pub to_primitive: Gc<Symbol>,
    pub to_string_tag: Gc<Symbol>,
    pub unscopables: Gc<Symbol>,
    // E1: explicit-resource-management.
    pub async_dispose: Gc<Symbol>,
    pub dispose: Gc<Symbol>,
}

impl WellKnown {
    pub fn all(&self) -> [(&'static str, Gc<Symbol>); 15] {
        [
            ("asyncIterator", self.async_iterator),
            ("hasInstance", self.has_instance),
            ("isConcatSpreadable", self.is_concat_spreadable),
            ("iterator", self.iterator),
            ("match", self.match_),
            ("matchAll", self.match_all),
            ("replace", self.replace),
            ("search", self.search),
            ("species", self.species),
            ("split", self.split),
            ("toPrimitive", self.to_primitive),
            ("toStringTag", self.to_string_tag),
            ("unscopables", self.unscopables),
            ("asyncDispose", self.async_dispose),
            ("dispose", self.dispose),
        ]
    }
}

/// Claves de uso frecuente (sin volver a crear los strings).
#[derive(Debug, Clone)]
pub struct Keys {
    pub length: PropertyKey,
    pub prototype: PropertyKey,
    pub constructor: PropertyKey,
    pub name: PropertyKey,
    pub message: PropertyKey,
    pub next: PropertyKey,
    pub done: PropertyKey,
    pub value: PropertyKey,
    pub return_: PropertyKey,
    pub throw: PropertyKey,
    pub get: PropertyKey,
    pub set: PropertyKey,
    pub writable: PropertyKey,
    pub enumerable: PropertyKey,
    pub configurable: PropertyKey,
    pub to_string: PropertyKey,
    pub value_of: PropertyKey,
    pub callee: PropertyKey,
    pub caller: PropertyKey,
    pub cause: PropertyKey,
    pub stack: PropertyKey,
    pub proto: PropertyKey,
}

impl Keys {
    fn new() -> Keys {
        Keys {
            length: PropertyKey::from("length"),
            prototype: PropertyKey::from("prototype"),
            constructor: PropertyKey::from("constructor"),
            name: PropertyKey::from("name"),
            message: PropertyKey::from("message"),
            next: PropertyKey::from("next"),
            done: PropertyKey::from("done"),
            value: PropertyKey::from("value"),
            return_: PropertyKey::from("return"),
            throw: PropertyKey::from("throw"),
            get: PropertyKey::from("get"),
            set: PropertyKey::from("set"),
            writable: PropertyKey::from("writable"),
            enumerable: PropertyKey::from("enumerable"),
            configurable: PropertyKey::from("configurable"),
            to_string: PropertyKey::from("toString"),
            value_of: PropertyKey::from("valueOf"),
            callee: PropertyKey::from("callee"),
            caller: PropertyKey::from("caller"),
            cause: PropertyKey::from("cause"),
            stack: PropertyKey::from("stack"),
            proto: PropertyKey::from("__proto__"),
        }
    }
}

/// Límites de ejecución.
#[derive(Debug, Clone)]
pub struct AgentOptions {
    /// Pila máxima (bytes) que puede usar la recursión de llamadas antes de lanzar
    /// `RangeError: Maximum call stack size exceeded`.
    pub max_stack: usize,
    /// Profundidad máxima de llamadas.
    pub max_frames: usize,
    /// Presupuesto de instrucciones (para el runner); `None` sin límite.
    pub instruction_budget: Option<u64>,
    /// Límite de tiempo.
    pub deadline: Option<Instant>,
}

impl Default for AgentOptions {
    fn default() -> AgentOptions {
        AgentOptions {
            max_stack: 4 * 1024 * 1024,
            max_frames: 10_000,
            instruction_budget: None,
            deadline: None,
        }
    }
}

pub struct Agent {
    pub heap: Heap,
    pub realms: Vec<Realm>,
    pub current_realm: RealmId,
    pub frames: Vec<Frame>,
    pub well_known: WellKnown,
    pub keys: Keys,
    /// GlobalSymbolRegistry (§20.4.2.2.1), en orden de creación.
    pub symbol_registry: Vec<(JsString, Gc<Symbol>)>,
    /// Llamadas desde Rust a JavaScript en curso (mientras sea > 0 no se recolecta).
    pub gc_inhibit: u32,
    pub options: AgentOptions,
    pub instructions: u64,
    /// Lo no implementado que se ejecutó (InternalError): el runner lo cuenta como fallo.
    pub internal_errors: Vec<String>,
    /// La salida de `print`.
    pub printer: Box<dyn FnMut(&str)>,
    /// Profundidad de llamadas nativas (para el límite de pila).
    pub(crate) call_depth: usize,
    stack_base: usize,
    /// Los Module Records y el loader del host (§16.2.1) (E1).
    pub modules: module::ModuleHost,
    // E3
    /// La cola de jobs (HostEnqueuePromiseJob, §9.5.5; HostEnqueueFinalizationRegistryCleanupJob,
    /// §9.5.6), en orden FIFO.
    pub jobs: std::collections::VecDeque<crate::builtins::promise::Job>,
    /// Los jobs que se están ejecutando (son raíces del GC mientras corren).
    pub(crate) running_jobs: Vec<crate::builtins::promise::Job>,
    /// HostPromiseRejectionTracker (§27.2.1.9): los rechazos sin manejar.
    pub rejections: crate::builtins::promise::RejectionTracker,
    /// `[[KeptAlive]]` del agente (§9.10, AddToKeptObjects / ClearKeptObjects).
    pub kept_alive: Vec<Value>,
    /// Las excepciones que ningún código atrapó dentro de un job (HostReportErrors).
    pub job_errors: Vec<Value>,
    /// AgentCanSuspend (§9.7.1): ¿puede bloquearse `Atomics.wait`? (El hilo principal de
    /// un navegador no puede; el runner de test262 lo activa con `CanBlockIsTrue`.)
    pub can_block: bool,
    /// Los waiters de `Atomics.waitAsync` y los timers del host, que RunJobs dispara cuando
    /// la cola queda vacía.
    pub(crate) timers: Vec<crate::builtins::atomics::Timer>,
}

impl Agent {
    pub fn new(options: AgentOptions) -> Agent {
        let mut heap = Heap::default();
        let mut sym = |d: &str| {
            heap.alloc(Symbol {
                description: Some(JsString::from(d)),
                private: false,
            })
        };
        let well_known = WellKnown {
            async_iterator: sym("Symbol.asyncIterator"),
            has_instance: sym("Symbol.hasInstance"),
            is_concat_spreadable: sym("Symbol.isConcatSpreadable"),
            iterator: sym("Symbol.iterator"),
            match_: sym("Symbol.match"),
            match_all: sym("Symbol.matchAll"),
            replace: sym("Symbol.replace"),
            search: sym("Symbol.search"),
            species: sym("Symbol.species"),
            split: sym("Symbol.split"),
            to_primitive: sym("Symbol.toPrimitive"),
            to_string_tag: sym("Symbol.toStringTag"),
            unscopables: sym("Symbol.unscopables"),
            async_dispose: sym("Symbol.asyncDispose"),
            dispose: sym("Symbol.dispose"),
        };
        let mut agent = Agent {
            heap,
            realms: Vec::new(),
            current_realm: 0,
            frames: Vec::new(),
            well_known,
            keys: Keys::new(),
            symbol_registry: Vec::new(),
            gc_inhibit: 0,
            options,
            instructions: 0,
            internal_errors: Vec::new(),
            printer: Box::new(|s| println!("{s}")),
            call_depth: 0,
            stack_base: crate::parser::stack_position(),
            modules: module::ModuleHost::default(), // E1
            jobs: std::collections::VecDeque::new(),
            running_jobs: Vec::new(),
            rejections: crate::builtins::promise::RejectionTracker::default(),
            kept_alive: Vec::new(),
            job_errors: Vec::new(),
            can_block: false,
            timers: Vec::new(),
        };
        let realm = agent.create_realm();
        agent.current_realm = realm;
        agent
    }

    /// Fija la base para medir la pila (llamar desde el hilo que va a ejecutar).
    pub fn reset_stack_base(&mut self) {
        self.stack_base = crate::parser::stack_position();
    }

    /// ¿Se usó más pila de la permitida?
    pub(crate) fn stack_exhausted(&self) -> bool {
        self.stack_base
            .saturating_sub(crate::parser::stack_position())
            > self.options.max_stack
            || self.frames.len() + self.call_depth > self.options.max_frames
    }

    pub fn realm(&self) -> &Realm {
        &self.realms[self.current_realm]
    }

    pub fn intrinsics(&self) -> &Intrinsics {
        &self.realms[self.current_realm].intrinsics
    }

    pub fn intrinsics_of(&self, realm: RealmId) -> &Intrinsics {
        &self.realms[realm].intrinsics
    }

    // --- Asignación ---

    pub fn alloc(&mut self, o: Object) -> Gc<Object> {
        self.heap.alloc(o)
    }

    /// OrdinaryObjectCreate (§10.1.12) sin slots adicionales.
    pub fn new_object(&mut self, proto: Option<Gc<Object>>) -> Gc<Object> {
        self.heap.alloc(Object::ordinary(proto))
    }

    /// Un objeto ordinario con %Object.prototype%.
    pub fn new_plain_object(&mut self) -> Gc<Object> {
        let p = self.intrinsics().object_prototype;
        self.new_object(Some(p))
    }

    pub fn new_symbol(&mut self, description: Option<JsString>) -> Gc<Symbol> {
        self.heap.alloc(Symbol {
            description,
            private: false,
        })
    }

    /// Define directamente una propiedad de datos (objetos recién creados, sin exóticos).
    pub fn put_data(
        &mut self,
        o: Gc<Object>,
        key: PropertyKey,
        value: Value,
        writable: bool,
        enumerable: bool,
        configurable: bool,
    ) {
        self.heap[o]
            .props
            .insert(key, Prop::data(value, writable, enumerable, configurable));
    }

    // --- Errores ---

    /// Crea un objeto de error del realm actual con `message`.
    pub fn make_error(&mut self, kind: ErrorType, message: &str) -> Gc<Object> {
        let i = self.intrinsics();
        let proto = match kind {
            ErrorType::Error => i.error_prototype,
            ErrorType::EvalError => i.eval_error_prototype,
            ErrorType::RangeError => i.range_error_prototype,
            ErrorType::ReferenceError => i.reference_error_prototype,
            ErrorType::SyntaxError => i.syntax_error_prototype,
            ErrorType::TypeError => i.type_error_prototype,
            ErrorType::UriError => i.uri_error_prototype,
        };
        let o = self.heap.alloc(Object::new(Some(proto), ObjectKind::Error));
        let key = self.keys.message.clone();
        self.put_data(o, key, Value::str(message), true, false, true);
        self.attach_stack(o);
        o
    }

    /// La propiedad no estándar `stack` con la cadena de frames.
    pub fn attach_stack(&mut self, o: Gc<Object>) {
        let mut out = String::new();
        for f in self.frames.iter().rev().take(32) {
            let name = f.code.name.to_string_lossy();
            let name = if name.is_empty() {
                "<anonymous>".to_owned()
            } else {
                name
            };
            let pos = f
                .code
                .position(f.pc.saturating_sub(1))
                .unwrap_or(f.code.span.start);
            let (line, col) = f.code.source.line_col(pos);
            out.push_str(&format!(
                "    at {name} ({}:{line}:{col})\n",
                f.code.source.name
            ));
        }
        let key = self.keys.stack.clone();
        self.put_data(o, key, Value::str(&out), true, false, true);
    }

    pub fn error(&mut self, kind: ErrorType, message: &str) -> Throw {
        Throw::Value(Value::Object(self.make_error(kind, message)))
    }

    pub fn type_error<T>(&mut self, message: &str) -> JsResult<T> {
        Err(self.error(ErrorType::TypeError, message))
    }

    pub fn range_error<T>(&mut self, message: &str) -> JsResult<T> {
        Err(self.error(ErrorType::RangeError, message))
    }

    pub fn reference_error<T>(&mut self, message: &str) -> JsResult<T> {
        Err(self.error(ErrorType::ReferenceError, message))
    }

    pub fn syntax_error<T>(&mut self, message: &str) -> JsResult<T> {
        Err(self.error(ErrorType::SyntaxError, message))
    }

    /// Algo no implementado: lanza un error con nombre `InternalError` y lo registra (para
    /// que cuente como fallo aunque el código lo atrape).
    pub fn internal_error<T>(&mut self, what: &str) -> JsResult<T> {
        let msg = format!("no implementado: {what}");
        self.internal_errors.push(msg.clone());
        let o = self.make_error(ErrorType::Error, &msg);
        let key = self.keys.name.clone();
        self.put_data(o, key, Value::str("InternalError"), true, false, true);
        Err(Throw::Value(Value::Object(o)))
    }

    // --- GC ---

    /// Recolecta ahora (si no hay código Rust con valores sin enraizar en la pila).
    pub fn collect(&mut self) -> bool {
        if self.gc_inhibit > 0 {
            return false;
        }
        let mut t = Tracer::default();
        self.trace_roots(&mut t);
        self.heap.collect(t);
        // E3: los FinalizationRegistry con celdas vaciadas piden su job de limpieza.
        crate::builtins::weak::enqueue_cleanup_jobs(self);
        true
    }

    fn trace_roots(&self, t: &mut Tracer) {
        for r in &self.realms {
            r.trace(t);
        }
        // E3
        for j in self.jobs.iter().chain(self.running_jobs.iter()) {
            j.trace(t);
        }
        self.rejections.trace(t);
        for v in self.kept_alive.iter().chain(self.job_errors.iter()) {
            t.value(v);
        }
        for timer in &self.timers {
            timer.trace(t);
        }
        for f in &self.frames {
            f.trace(t);
        }
        for (_, s) in &self.symbol_registry {
            t.gc(*s);
        }
        for (_, s) in self.well_known.all() {
            t.gc(s);
        }
        self.modules.trace(t); // E1
    }

    pub fn gc_stats(&self) -> GcStats {
        self.heap.stats()
    }

    /// Punto seguro del intérprete: recolecta si se superó el umbral.
    pub(crate) fn safepoint(&mut self) -> JsResult<()> {
        if self.heap.should_collect() && self.gc_inhibit == 0 {
            self.collect();
        }
        if let Some(budget) = self.options.instruction_budget
            && self.instructions > budget
        {
            return Err(Throw::Terminate(
                "presupuesto de instrucciones agotado".into(),
            ));
        }
        if self.instructions & 0x3FF == 0
            && let Some(d) = self.options.deadline
            && Instant::now() > d
        {
            return Err(Throw::Terminate("tiempo agotado".into()));
        }
        Ok(())
    }

    /// `print` del host.
    pub fn print(&mut self, s: &str) {
        (self.printer)(s);
    }

    /// Registro de símbolos: Symbol.for.
    pub fn symbol_for(&mut self, key: JsString) -> Gc<Symbol> {
        if let Some((_, s)) = self.symbol_registry.iter().find(|(k, _)| *k == key) {
            return *s;
        }
        let s = self.new_symbol(Some(key.clone()));
        self.symbol_registry.push((key, s));
        s
    }

    pub fn symbol_key_for(&self, s: Gc<Symbol>) -> Option<JsString> {
        self.symbol_registry
            .iter()
            .find(|(_, x)| *x == s)
            .map(|(k, _)| k.clone())
    }
}

/// Valores de string comunes ya convertidos.
pub(crate) fn js(s: &str) -> JsString {
    JsString::from(s)
}

#[allow(dead_code)]
pub(crate) type Map<K, V> = HashMap<K, V>;
