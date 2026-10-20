//! La fachada de incrustación: [`Runtime`] envuelve un [`Agent`] con hooks del host,
//! instala los globales del bucle de eventos en cada realm y convierte las completions
//! abruptas en [`JsError`]. Incluye el formateo de valores para mostrar (`to_display`, sin
//! ejecutar código del usuario) y las conversiones entre valores y tipos de Rust.

use std::any::Any;
use std::fmt;

use crate::heap::Gc;
use crate::host::hooks::HostHooks;
use crate::host::{ClassObjects, HostClass, HostClassId, HostHandle, HostKey};
use crate::object::{Object, ObjectKind, PropValue};
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::{Agent, AgentOptions, JsResult, RealmId, Throw};

/// Una excepción de JavaScript (o la terminación del agente) vista desde Rust.
#[derive(Debug, Clone)]
pub struct JsError {
    /// `Name: message` para los errores; si no, el valor formateado.
    pub message: String,
    /// La propiedad `stack` del error (vacía si no hay).
    pub stack: String,
    /// El valor lanzado (`None` si el agente terminó). No está enraizado: fíjelo con
    /// `Runtime::pin` si se va a usar después de otra ejecución.
    pub value: Option<Value>,
}

impl fmt::Display for JsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for JsError {}

impl JsError {
    pub fn from_value(agent: &mut Agent, v: Value) -> JsError {
        let (message, stack) = match &v {
            Value::Object(o) if matches!(agent.heap[*o].kind, ObjectKind::Error) => {
                let name = data_prop(agent, *o, "name")
                    .map(|n| display_depth(agent, &n, 0))
                    .unwrap_or_else(|| "Error".to_owned());
                let msg = data_prop(agent, *o, "message")
                    .map(|m| display_depth(agent, &m, 0))
                    .unwrap_or_default();
                let stack = data_prop(agent, *o, "stack")
                    .and_then(|s| s.as_string().map(JsString::to_string_lossy))
                    .unwrap_or_default();
                let message = if msg.is_empty() {
                    name
                } else {
                    format!("{name}: {msg}")
                };
                (message, stack)
            }
            other => (display(agent, other), String::new()),
        };
        JsError {
            message,
            stack,
            value: Some(v),
        }
    }

    pub fn from_throw(agent: &mut Agent, t: Throw) -> JsError {
        match t {
            Throw::Value(v) => JsError::from_value(agent, v),
            Throw::Terminate(reason) => JsError {
                message: reason,
                stack: String::new(),
                value: None,
            },
        }
    }
}

/// Una propiedad de datos (propia o heredada) sin ejecutar código: los accessors y los
/// Proxy dan `None`.
fn data_prop(agent: &Agent, o: Gc<Object>, name: &str) -> Option<Value> {
    let key = PropertyKey::from(name);
    let mut cur = Some(o);
    let mut depth = 0;
    while let Some(c) = cur {
        let obj = &agent.heap[c];
        if matches!(obj.kind, ObjectKind::Proxy(_)) || depth > 64 {
            return None;
        }
        if let Some(p) = obj.props.get(&key) {
            return match &p.value {
                PropValue::Data { value, .. } => Some(value.clone()),
                PropValue::Accessor { .. } => None,
            };
        }
        cur = obj.proto;
        depth += 1;
    }
    None
}

/// Un valor formateado para mostrar (consola, errores) sin ejecutar código del usuario.
pub fn display(agent: &mut Agent, v: &Value) -> String {
    display_depth(agent, v, 0)
}

fn display_depth(agent: &mut Agent, v: &Value, depth: usize) -> String {
    match v {
        Value::Undefined => "undefined".into(),
        Value::Null => "null".into(),
        Value::Boolean(b) => b.to_string(),
        Value::Number(n) => crate::abstract_ops::number::number_to_std_string(*n),
        Value::String(s) => {
            if depth == 0 {
                s.to_string_lossy()
            } else {
                format!("{:?}", s.to_string_lossy())
            }
        }
        Value::BigInt(b) => format!("{}n", agent.heap[*b].to_string_radix(10)),
        Value::Symbol(s) => {
            let d = agent.heap[*s].description.clone();
            format!(
                "Symbol({})",
                d.map(|d| d.to_string_lossy()).unwrap_or_default()
            )
        }
        Value::Object(o) => display_object(agent, *o, depth),
    }
}

fn display_object(agent: &mut Agent, o: Gc<Object>, depth: usize) -> String {
    let obj = &agent.heap[o];
    if obj.is_callable() {
        let name = data_prop(agent, o, "name")
            .and_then(|n| n.as_string().map(JsString::to_string_lossy))
            .unwrap_or_default();
        return if name.is_empty() {
            "ƒ anonymous()".into()
        } else {
            format!("ƒ {name}()")
        };
    }
    match &obj.kind {
        ObjectKind::Error => return JsError::from_value(agent, Value::Object(o)).message,
        ObjectKind::Host(h) => {
            let name = agent.host_class_name(h.class);
            return format!("[object {name}]");
        }
        ObjectKind::Promise(p) => {
            let state = match p.state {
                crate::builtins::promise::PromiseState::Pending => "<pending>".to_owned(),
                crate::builtins::promise::PromiseState::Fulfilled => {
                    let r = p.result.clone();
                    display_depth(agent, &r, depth + 1)
                }
                crate::builtins::promise::PromiseState::Rejected => {
                    let r = p.result.clone();
                    format!("<rejected> {}", display_depth(agent, &r, depth + 1))
                }
            };
            return format!("Promise {{{state}}}");
        }
        _ => {}
    }
    if depth > 2 {
        return if obj.is_array() {
            "[Array]".into()
        } else {
            "[Object]".into()
        };
    }
    let is_array = obj.is_array();
    let keys: Vec<PropertyKey> = match &obj.kind {
        ObjectKind::Array(a) => {
            let len = a.length.min(100);
            (0..len).map(PropertyKey::Index).collect()
        }
        _ => obj.props.keys().into_iter().take(20).collect(),
    };
    let mut parts = Vec::new();
    for k in keys {
        if k.is_symbol() {
            continue;
        }
        let shown = match agent.own_prop(o, &k) {
            Some(d) if d.is_accessor() => "[Getter/Setter]".to_owned(),
            Some(d) if !is_array && d.enumerable != Some(true) => continue,
            Some(d) => {
                let v = d.value.unwrap_or_default();
                display_depth(agent, &v, depth + 1)
            }
            None => {
                if is_array {
                    "<empty>".to_owned()
                } else {
                    continue;
                }
            }
        };
        if is_array {
            parts.push(shown);
        } else {
            parts.push(format!("{k}: {shown}"));
        }
    }
    if is_array {
        format!("[{}]", parts.join(", "))
    } else if parts.is_empty() {
        "{}".into()
    } else {
        format!("{{{}}}", parts.join(", "))
    }
}

/// Conversión de un valor a un tipo de Rust (con las operaciones abstractas de JS).
pub trait FromJs: Sized {
    fn from_js(agent: &mut Agent, v: &Value) -> JsResult<Self>;
}

/// Conversión de un tipo de Rust a un valor.
pub trait IntoJs {
    fn into_js(self, agent: &mut Agent) -> Value;
}

impl FromJs for Value {
    fn from_js(_: &mut Agent, v: &Value) -> JsResult<Value> {
        Ok(v.clone())
    }
}

impl FromJs for f64 {
    fn from_js(agent: &mut Agent, v: &Value) -> JsResult<f64> {
        agent.to_number(v)
    }
}

impl FromJs for i32 {
    fn from_js(agent: &mut Agent, v: &Value) -> JsResult<i32> {
        agent.to_int32(v)
    }
}

impl FromJs for u32 {
    fn from_js(agent: &mut Agent, v: &Value) -> JsResult<u32> {
        agent.to_uint32(v)
    }
}

impl FromJs for bool {
    fn from_js(agent: &mut Agent, v: &Value) -> JsResult<bool> {
        Ok(crate::abstract_ops::conversion::to_boolean(agent, v))
    }
}

impl FromJs for String {
    fn from_js(agent: &mut Agent, v: &Value) -> JsResult<String> {
        Ok(agent.to_string(v)?.to_string_lossy())
    }
}

impl<T: FromJs> FromJs for Option<T> {
    fn from_js(agent: &mut Agent, v: &Value) -> JsResult<Option<T>> {
        if v.is_nullish() {
            return Ok(None);
        }
        T::from_js(agent, v).map(Some)
    }
}

impl IntoJs for Value {
    fn into_js(self, _: &mut Agent) -> Value {
        self
    }
}

impl IntoJs for f64 {
    fn into_js(self, _: &mut Agent) -> Value {
        Value::Number(self)
    }
}

impl IntoJs for i32 {
    fn into_js(self, _: &mut Agent) -> Value {
        Value::Number(f64::from(self))
    }
}

impl IntoJs for u32 {
    fn into_js(self, _: &mut Agent) -> Value {
        Value::Number(f64::from(self))
    }
}

impl IntoJs for bool {
    fn into_js(self, _: &mut Agent) -> Value {
        Value::Boolean(self)
    }
}

impl IntoJs for &str {
    fn into_js(self, _: &mut Agent) -> Value {
        Value::str(self)
    }
}

impl IntoJs for String {
    fn into_js(self, _: &mut Agent) -> Value {
        Value::str(&self)
    }
}

impl IntoJs for () {
    fn into_js(self, _: &mut Agent) -> Value {
        Value::Undefined
    }
}

impl<T: IntoJs> IntoJs for Option<T> {
    fn into_js(self, agent: &mut Agent) -> Value {
        match self {
            Some(x) => x.into_js(agent),
            None => Value::Null,
        }
    }
}

impl<T: IntoJs> IntoJs for Vec<T> {
    fn into_js(self, agent: &mut Agent) -> Value {
        let values: Vec<Value> = self.into_iter().map(|x| x.into_js(agent)).collect();
        match agent.create_array_from_list(&values) {
            Ok(a) => Value::Object(a),
            Err(_) => Value::Undefined,
        }
    }
}

/// Un motor listo para incrustar: un agente con los hooks del host y los globales del
/// bucle de eventos en cada realm.
pub struct Runtime {
    pub agent: Agent,
}

impl Runtime {
    pub fn new(hooks: Box<dyn HostHooks>) -> Runtime {
        Runtime::with_options(hooks, AgentOptions::default())
    }

    pub fn with_options(hooks: Box<dyn HostHooks>, options: AgentOptions) -> Runtime {
        let mut agent = Agent::new(options);
        agent.reset_stack_base();
        agent.set_host_hooks(hooks);
        let realm = agent.current_realm;
        crate::host::event_loop::install_globals(&mut agent, realm);
        Runtime { agent }
    }

    /// El realm creado con el agente.
    pub fn main_realm(&self) -> RealmId {
        self.agent.current_realm
    }

    /// Un realm nuevo con sus intrínsecos, las clases host y los globales del bucle.
    pub fn create_realm(&mut self) -> RealmId {
        let r = self.agent.create_realm();
        crate::host::event_loop::install_globals(&mut self.agent, r);
        r
    }

    /// El objeto global de `realm`.
    pub fn global(&self, realm: RealmId) -> Gc<Object> {
        self.agent.realms[realm].global_object
    }

    fn finish(&mut self, r: JsResult<Value>) -> Result<Value, JsError> {
        // "Clean up after running script" (HTML §8.1.4.4): con la pila vacía, punto de
        // control de microtasks.
        let r = if self.agent.frames.is_empty() {
            match r {
                Ok(v) => {
                    let scope = self.agent.heap.open_scope();
                    self.agent.heap.root(v.clone());
                    let c = self.agent.perform_microtask_checkpoint();
                    self.agent.heap.close_scope(scope);
                    c.map(|_| v)
                }
                Err(e) => {
                    let c = self.agent.perform_microtask_checkpoint();
                    match c {
                        Ok(()) => Err(e),
                        Err(t) => Err(t),
                    }
                }
            }
        } else {
            r
        };
        r.map_err(|t| JsError::from_throw(&mut self.agent, t))
    }

    /// Evalúa un script clásico en `realm` (ScriptEvaluation, §16.1.6) y luego los
    /// microtasks.
    pub fn eval_script(&mut self, realm: RealmId, src: &str, url: &str) -> Result<Value, JsError> {
        let saved = self.agent.current_realm;
        self.agent.current_realm = realm;
        let r = self
            .agent
            .compile_script_text(src, url)
            .and_then(|code| self.agent.script_evaluation(code));
        self.agent.current_realm = saved;
        self.finish(r)
    }

    /// Llama a `f` (y luego los microtasks si la pila quedó vacía).
    pub fn call(&mut self, f: &Value, this: Value, args: &[Value]) -> Result<Value, JsError> {
        let r = self.agent.call_rooted(f, this, args);
        self.finish(r)
    }

    /// `Get(o, name)`.
    pub fn get(&mut self, o: Gc<Object>, name: &str) -> Result<Value, JsError> {
        let r = self
            .agent
            .get(o, &PropertyKey::from(name), Value::Object(o));
        r.map_err(|t| JsError::from_throw(&mut self.agent, t))
    }

    /// `Set(o, name, v, true)`.
    pub fn set(&mut self, o: Gc<Object>, name: &str, v: Value) -> Result<(), JsError> {
        let r = self.agent.set_prop(o, PropertyKey::from(name), v);
        r.map_err(|t| JsError::from_throw(&mut self.agent, t))
    }

    /// Un valor formateado para mostrar.
    pub fn to_display(&mut self, v: &Value) -> String {
        display(&mut self.agent, v)
    }

    pub fn convert<T: FromJs>(&mut self, v: &Value) -> Result<T, JsError> {
        T::from_js(&mut self.agent, v).map_err(|t| JsError::from_throw(&mut self.agent, t))
    }

    pub fn value<T: IntoJs>(&mut self, x: T) -> Value {
        x.into_js(&mut self.agent)
    }

    pub fn run_one_task(&mut self) -> Result<bool, JsError> {
        self.agent
            .run_one_task()
            .map_err(|t| JsError::from_throw(&mut self.agent, t))
    }

    pub fn run_until_idle(&mut self) -> Result<usize, JsError> {
        self.agent
            .run_until_idle()
            .map_err(|t| JsError::from_throw(&mut self.agent, t))
    }

    pub fn next_wake(&mut self) -> Option<f64> {
        self.agent.next_wake()
    }

    pub fn run_animation_frames(&mut self, timestamp: f64) -> Result<(), JsError> {
        self.agent
            .run_animation_frames(timestamp)
            .map_err(|t| JsError::from_throw(&mut self.agent, t))
    }

    /// Recolecta ahora.
    pub fn gc(&mut self) {
        self.agent.collect();
    }

    pub fn register_class(&mut self, class: HostClass) -> HostClassId {
        self.agent.register_host_class(class)
    }

    pub fn class_objects(&mut self, realm: RealmId, class: HostClassId) -> ClassObjects {
        self.agent.host_class_objects(realm, class)
    }

    pub fn wrap(
        &mut self,
        realm: RealmId,
        key: HostKey,
        class: HostClassId,
        payload: Box<dyn Any>,
    ) -> Gc<Object> {
        self.agent.host_wrap(realm, key, class, payload)
    }

    pub fn lookup(&self, realm: RealmId, key: HostKey) -> Option<Gc<Object>> {
        self.agent.host_lookup(realm, key)
    }

    pub fn unwrap<T: 'static>(&self, v: &Value) -> Option<&T> {
        self.agent.host_unwrap::<T>(v)
    }

    pub fn unwrap_mut<T: 'static>(&mut self, v: &Value) -> Option<&mut T> {
        self.agent.host_unwrap_mut::<T>(v)
    }

    pub fn pin(&mut self, v: Value) -> HostHandle {
        self.agent.host_pin(v)
    }

    pub fn unpin(&mut self, h: HostHandle) -> Option<Value> {
        self.agent.host_unpin(h)
    }
}
