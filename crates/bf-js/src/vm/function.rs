//! Objetos función: ECMAScript (§10.2), built-in (§10.3) y bound (§10.4.1), con
//! `[[Call]]`/`[[Construct]]`, la creación de funciones (OrdinaryFunctionCreate,
//! MakeConstructor, SetFunctionName, SetFunctionLength, CreateBuiltinFunction) y los
//! elementos de clase (InitializeInstanceElements, DefineField, los elementos privados).

use std::rc::Rc;

use crate::compiler::bytecode::{CodeKind, FunctionCode};
use crate::heap::{Gc, Trace, Tracer};
use crate::object::{Object, ObjectKind, PrivateElement, Prop, PropValue, PropertyDescriptor};
use crate::string::JsString;
use crate::value::{PropertyKey, Symbol, Value};
use crate::vm::env::Env;
use crate::vm::frame::Frame;
use crate::vm::interp::{FrameExit, Resume};
use crate::vm::{Agent, JsResult, RealmId, Throw};

/// El nombre de un campo de clase: una clave o un Private Name.
#[derive(Debug, Clone)]
pub enum FieldName {
    Key(PropertyKey),
    Private(Gc<Symbol>),
}

/// ClassFieldDefinition (§6.2.11).
#[derive(Debug, Clone)]
pub struct ClassField {
    pub name: FieldName,
    pub initializer: Option<Gc<Object>>,
}

/// `[[Fields]]` y `[[PrivateMethods]]` de un constructor de clase.
#[derive(Debug, Clone, Default)]
pub struct ClassData {
    pub fields: Vec<ClassField>,
    pub private_methods: Vec<(Gc<Symbol>, PrivateElement)>,
}

/// Una función ECMAScript (§10.2).
#[derive(Debug)]
pub struct Closure {
    pub code: Rc<FunctionCode>,
    pub env: Gc<Env>,
    pub home_object: Option<Gc<Object>>,
    pub realm: RealmId,
    pub is_constructor: bool,
    pub class: Option<Box<ClassData>>,
    /// `[[ClassFieldInitializerName]]`.
    pub field_name: Option<FieldName>,
}

impl Trace for Closure {
    fn trace(&self, t: &mut Tracer) {
        t.gc(self.env);
        t.opt(self.home_object);
        if let Some(c) = &self.class {
            for f in &c.fields {
                f.name.trace(t);
                t.opt(f.initializer);
            }
            for (n, e) in &c.private_methods {
                t.gc(*n);
                e.trace(t);
            }
        }
        if let Some(n) = &self.field_name {
            n.trace(t);
        }
    }
}

impl Trace for FieldName {
    fn trace(&self, t: &mut Tracer) {
        match self {
            FieldName::Key(PropertyKey::Symbol(s)) | FieldName::Private(s) => t.gc(*s),
            FieldName::Key(_) => {}
        }
    }
}

/// Los argumentos de una función nativa.
pub struct CallArgs<'a> {
    pub this: Value,
    pub args: &'a [Value],
    /// `Some` si se llamó con `[[Construct]]`.
    pub new_target: Option<Gc<Object>>,
    pub callee: Gc<Object>,
}

impl CallArgs<'_> {
    pub fn arg(&self, i: usize) -> Value {
        self.args.get(i).cloned().unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.args.len()
    }

    pub fn is_empty(&self) -> bool {
        self.args.is_empty()
    }
}

pub type NativeFn = fn(&mut Agent, &CallArgs<'_>) -> JsResult<Value>;

/// Una función built-in (§10.3).
pub struct BuiltinFunction {
    pub func: NativeFn,
    pub realm: RealmId,
    pub constructor: bool,
    /// Datos propios (p. ej. de las funciones creadas por otras built-ins).
    pub slots: Vec<Value>,
}

impl std::fmt::Debug for BuiltinFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BuiltinFunction")
    }
}

impl Trace for BuiltinFunction {
    fn trace(&self, t: &mut Tracer) {
        for v in &self.slots {
            t.value(v);
        }
    }
}

/// Bound function exótica (§10.4.1).
#[derive(Debug)]
pub struct BoundFunction {
    pub target: Gc<Object>,
    pub this: Value,
    pub args: Vec<Value>,
    pub constructor: bool,
}

impl Trace for BoundFunction {
    fn trace(&self, t: &mut Tracer) {
        t.gc(self.target);
        t.value(&self.this);
        for a in &self.args {
            t.value(a);
        }
    }
}

impl Agent {
    // --- Call / Construct ---

    /// Call (§7.3.14), desde código Rust: inhibe el GC mientras corre.
    pub fn call(&mut self, f: &Value, this: Value, args: &[Value]) -> JsResult<Value> {
        self.gc_inhibit += 1;
        let r = self.call_internal(f, this, args);
        self.gc_inhibit -= 1;
        r
    }

    /// Construct (§7.3.15), desde código Rust.
    pub fn construct(
        &mut self,
        f: Gc<Object>,
        args: &[Value],
        new_target: Option<Gc<Object>>,
    ) -> JsResult<Value> {
        self.gc_inhibit += 1;
        let r = self.construct_internal(f, args, new_target.unwrap_or(f));
        self.gc_inhibit -= 1;
        r
    }

    /// `[[Call]]` sin inhibir el GC (todo lo que se usa está enraizado por el llamador).
    pub(crate) fn call_internal(
        &mut self,
        f: &Value,
        this: Value,
        args: &[Value],
    ) -> JsResult<Value> {
        let Value::Object(fo) = f else {
            return self.type_error(&format!("{} is not a function", self.describe(f)));
        };
        let fo = *fo;
        if self.stack_exhausted() {
            return self.range_error("Maximum call stack size exceeded");
        }
        match &self.heap[fo].kind {
            ObjectKind::Function(_) => self.call_closure(fo, this, args.to_vec(), None),
            ObjectKind::HtmlDda => Ok(Value::Null),
            // E1: Proxy exótico.
            ObjectKind::Proxy(p) if p.callable => {
                self.call_depth += 1;
                let r = self.proxy_call(fo, this, args);
                self.call_depth -= 1;
                r
            }
            ObjectKind::Builtin(b) => {
                let (func, realm) = (b.func, b.realm);
                let saved = self.current_realm;
                self.current_realm = realm;
                self.call_depth += 1;
                let r = func(
                    self,
                    &CallArgs {
                        this,
                        args,
                        new_target: None,
                        callee: fo,
                    },
                );
                self.call_depth -= 1;
                self.current_realm = saved;
                r
            }
            ObjectKind::Bound(b) => {
                let target = Value::Object(b.target);
                let bthis = b.this.clone();
                let mut all = b.args.clone();
                all.extend_from_slice(args);
                self.call_depth += 1;
                let r = self.call_internal(&target, bthis, &all);
                self.call_depth -= 1;
                r
            }
            _ => self.type_error(&format!("{} is not a function", self.describe(f))),
        }
    }

    /// `[[Construct]]` sin inhibir el GC.
    pub(crate) fn construct_internal(
        &mut self,
        f: Gc<Object>,
        args: &[Value],
        new_target: Gc<Object>,
    ) -> JsResult<Value> {
        if self.stack_exhausted() {
            return self.range_error("Maximum call stack size exceeded");
        }
        match &self.heap[f].kind {
            ObjectKind::Function(c) if c.is_constructor => {
                self.construct_closure(f, args.to_vec(), new_target)
            }
            ObjectKind::Builtin(b) if b.constructor => {
                let (func, realm) = (b.func, b.realm);
                let saved = self.current_realm;
                self.current_realm = realm;
                self.call_depth += 1;
                let r = func(
                    self,
                    &CallArgs {
                        this: Value::Undefined,
                        args,
                        new_target: Some(new_target),
                        callee: f,
                    },
                );
                self.call_depth -= 1;
                self.current_realm = saved;
                r
            }
            ObjectKind::Bound(b) if b.constructor => {
                // §10.4.1.2
                let target = b.target;
                let mut all = b.args.clone();
                all.extend_from_slice(args);
                let nt = if new_target == f { target } else { new_target };
                self.call_depth += 1;
                let r = self.construct_internal(target, &all, nt);
                self.call_depth -= 1;
                r
            }
            // E1: Proxy exótico.
            ObjectKind::Proxy(p) if p.constructor => {
                self.call_depth += 1;
                let r = self.proxy_construct(f, args, new_target);
                self.call_depth -= 1;
                r
            }
            _ => {
                let d = self.describe(&Value::Object(f));
                self.type_error(&format!("{d} is not a constructor"))
            }
        }
    }

    /// Una descripción corta de un valor para los mensajes de error.
    pub fn describe(&self, v: &Value) -> String {
        match v {
            Value::String(s) => format!("\"{}\"", s.to_string_lossy()),
            Value::Object(o) => {
                if self.heap[*o].is_callable() {
                    "function".to_owned()
                } else {
                    "object".to_owned()
                }
            }
            Value::Number(n) => crate::abstract_ops::number::number_to_string(*n).to_string(),
            Value::Symbol(_) => "Symbol()".to_owned(),
            other => format!("{other:?}"),
        }
    }

    /// PrepareForOrdinaryCall + OrdinaryCallBindThis + OrdinaryCallEvaluateBody (§10.2.1).
    fn call_closure(
        &mut self,
        f: Gc<Object>,
        this: Value,
        args: Vec<Value>,
        new_target: Option<Gc<Object>>,
    ) -> JsResult<Value> {
        let (code, env, realm) = match &self.heap[f].kind {
            ObjectKind::Function(c) => (c.code.clone(), c.env, c.realm),
            _ => unreachable!("closure"),
        };
        if matches!(code.kind, CodeKind::ClassConstructor { .. }) && new_target.is_none() {
            // §10.2.1 paso 2: el error se crea en el realm de la función.
            let saved = self.current_realm;
            self.current_realm = realm;
            let e = self.type_error("Class constructor cannot be invoked without 'new'");
            self.current_realm = saved;
            return e;
        }
        // OrdinaryCallBindThis (§10.2.1.2).
        let this_value = if code.kind == CodeKind::Arrow {
            None
        } else if code.strict {
            Some(this)
        } else if this.is_nullish() {
            Some(Value::Object(self.realms[realm].global_object))
        } else {
            let saved = self.current_realm;
            self.current_realm = realm;
            let o = self.to_object(&this);
            self.current_realm = saved;
            Some(Value::Object(o?))
        };
        let mut frame = Frame::new(code, env, realm);
        frame.this = this_value;
        frame.callee = Some(f);
        frame.new_target = new_target;
        frame.args = args;
        // E3: las funciones async corren hasta el primer `await` y devuelven su promesa.
        if frame.code.is_async && !frame.code.is_generator {
            return crate::builtins::r#async::async_function_start(self, frame);
        }
        match self.run_frame(frame)? {
            (FrameExit::Return(v), _) => Ok(v),
            (FrameExit::GeneratorStart(g), frame) => {
                crate::vm::generator::store_start_frame(self, g, frame);
                Ok(Value::Object(g))
            }
            (FrameExit::Yield(..), _) => unreachable!("yield fuera de un generador"),
            (FrameExit::Await(_), _) => unreachable!("await fuera de una función async"),
        }
    }

    /// Ejecuta un frame nuevo hasta que termina (o se suspende el generador al empezar).
    pub(crate) fn run_frame(&mut self, frame: Frame) -> JsResult<(FrameExit, Frame)> {
        let saved = self.current_realm;
        self.current_realm = frame.realm;
        self.frames.push(frame);
        let r = self.run(Resume::Normal);
        let frame = self.frames.pop().expect("frame");
        self.current_realm = saved;
        r.map(|exit| (exit, frame))
    }

    /// `[[Construct]]` de una función ECMAScript (§10.2.2).
    fn construct_closure(
        &mut self,
        f: Gc<Object>,
        args: Vec<Value>,
        new_target: Gc<Object>,
    ) -> JsResult<Value> {
        let (code, env, realm) = match &self.heap[f].kind {
            ObjectKind::Function(c) => (c.code.clone(), c.env, c.realm),
            _ => unreachable!("closure"),
        };
        let derived = matches!(code.kind, CodeKind::ClassConstructor { derived: true });
        let this_arg = if derived {
            None
        } else {
            let fallback = self.intrinsics().object_prototype;
            let proto = self.get_prototype_from_constructor(new_target, |i| i.object_prototype)?;
            let _ = fallback;
            let o = self.new_object(Some(proto));
            Some(o)
        };
        if let Some(t) = this_arg
            && matches!(code.kind, CodeKind::ClassConstructor { .. })
        {
            // Los campos se inicializan antes del cuerpo (paso 6.b).
            let saved = self.current_realm;
            self.current_realm = realm;
            let r = self.initialize_instance_elements(t, f);
            self.current_realm = saved;
            r?;
        }
        let mut frame = Frame::new(code, env, realm);
        frame.this = this_arg.map(Value::Object);
        frame.callee = Some(f);
        frame.new_target = Some(new_target);
        frame.args = args;
        let (exit, frame) = self.run_frame(frame)?;
        let FrameExit::Return(result) = exit else {
            unreachable!("constructor generador")
        };
        if result.is_object() {
            return Ok(result);
        }
        if let Some(t) = this_arg {
            return Ok(Value::Object(t));
        }
        // E1: el error se crea en el realm del llamador (§10.2.2 paso 9 ya restauró su
        // contexto).
        if !result.is_undefined() {
            return self.type_error("Derived constructors may only return object or undefined");
        }
        // GetThisBinding del environment del constructor.
        let this = match frame.function_env {
            Some(env) => self.heap[env]
                .decl()
                .and_then(|d| d.function.as_ref())
                .and_then(|f| f.this.clone()),
            None => frame.this.clone(),
        };
        match this {
            Some(v) => Ok(v),
            None => self.reference_error(
                "Must call super constructor in derived class before accessing 'this' or returning from derived constructor",
            ),
        }
    }

    // --- Creación de funciones ---

    /// OrdinaryFunctionCreate (§10.2.3) + las propiedades `length`, `name` y `prototype`
    /// según la clase de código.
    pub fn make_closure(
        &mut self,
        code: Rc<FunctionCode>,
        env: Gc<Env>,
        home: Option<Gc<Object>>,
        name: Option<Value>,
    ) -> Gc<Object> {
        let i = self.intrinsics();
        let proto = match (code.is_generator, code.is_async) {
            (true, false) => i.generator_function_prototype,
            // E3: %AsyncGeneratorFunction.prototype% y %AsyncFunction.prototype%.
            (true, true) => i.async_generator_function_prototype,
            (false, true) => i.async_function_prototype,
            (false, false) => i.function_prototype,
        };
        let is_async = code.is_async;
        let is_sloppy_plain = matches!(code.kind, CodeKind::Normal)
            && !code.strict
            && !code.is_generator
            && !code.is_async;
        let is_constructor =
            matches!(code.kind, CodeKind::Normal) && !code.is_generator && !code.is_async;
        let realm = self.current_realm;
        let length = f64::from(code.length);
        let code_name = code.name.clone();
        let is_gen = code.is_generator;
        let has_prototype = code.has_prototype;
        let f = self.heap.alloc(Object::new(
            Some(proto),
            ObjectKind::Function(Box::new(Closure {
                code,
                env,
                home_object: home,
                realm,
                is_constructor,
                class: None,
                field_name: None,
            })),
        ));
        let k = self.keys.length.clone();
        self.put_data(f, k, Value::Number(length), false, false, true);
        let name = name.unwrap_or(Value::String(code_name));
        self.set_function_name_value(f, name);
        // E1: `caller` propio (null) en las funciones no estrictas comunes (§17.1 permite la
        // extensión salvo en las estrictas).
        if is_sloppy_plain {
            self.put_data(
                f,
                PropertyKey::from("caller"),
                Value::Null,
                false,
                false,
                false,
            );
        }
        if has_prototype {
            if is_gen {
                // §15.5.3: el prototype de un generador hereda de %GeneratorPrototype%
                // (§15.6.3: %AsyncGeneratorPrototype% si es async).
                let gp = if is_async {
                    self.intrinsics().async_generator_prototype
                } else {
                    self.intrinsics().generator_prototype
                };
                let p = self.new_object(Some(gp));
                let k = self.keys.prototype.clone();
                self.put_data(f, k, Value::Object(p), true, false, false);
            } else {
                self.make_constructor_prototype(f, true);
            }
        }
        f
    }

    /// MakeConstructor (§10.2.5) con un prototype nuevo.
    pub fn make_constructor_prototype(&mut self, f: Gc<Object>, writable: bool) {
        let p = self.new_plain_object();
        let k = self.keys.constructor.clone();
        self.put_data(p, k, Value::Object(f), writable, false, true);
        let k = self.keys.prototype.clone();
        self.put_data(f, k, Value::Object(p), writable, false, false);
    }

    /// SetFunctionName (§10.2.9) con un nombre ya calculado (String) o una clave
    /// (Symbol → `[desc]`, Private Name → su descripción).
    pub fn set_function_name_value(&mut self, f: Gc<Object>, name: Value) {
        let s = match name {
            Value::Symbol(sym) => {
                let d = self.heap[sym].description.clone();
                if self.heap[sym].private {
                    d.unwrap_or_default()
                } else {
                    match d {
                        Some(d) => {
                            let mut u: Vec<u16> = vec![u16::from(b'[')];
                            u.extend_from_slice(d.units());
                            u.push(u16::from(b']'));
                            JsString::from_units(u)
                        }
                        None => JsString::default(),
                    }
                }
            }
            Value::String(s) => s,
            other => crate::vm::js(&format!("{other:?}")),
        };
        let k = self.keys.name.clone();
        self.put_data(f, k, Value::String(s), false, false, true);
    }

    /// SetFunctionName con prefijo (`get`/`set`).
    pub fn set_function_name(&mut self, f: Gc<Object>, key: &PropertyKey, prefix: Option<&str>) {
        let base = match key {
            PropertyKey::Symbol(s) => {
                let v = Value::Symbol(*s);
                self.set_function_name_value(f, v);
                let k = self.keys.name.clone();
                match self.heap[f].props.get(&k).map(|p| p.value.clone()) {
                    Some(PropValue::Data {
                        value: Value::String(s),
                        ..
                    }) => s,
                    _ => JsString::default(),
                }
            }
            other => other.to_js_string().unwrap_or_default(),
        };
        let full = match prefix {
            Some(p) => {
                let mut u: Vec<u16> = p.encode_utf16().collect();
                u.push(u16::from(b' '));
                u.extend_from_slice(base.units());
                JsString::from_units(u)
            }
            None => base,
        };
        let k = self.keys.name.clone();
        self.put_data(f, k, Value::String(full), false, false, true);
    }

    /// CreateBuiltinFunction (§10.3.4).
    pub fn create_builtin_function(
        &mut self,
        func: crate::vm::function::NativeFn,
        length: u32,
        name: &str,
        constructor: bool,
    ) -> Gc<Object> {
        let proto = self.intrinsics().function_prototype;
        let realm = self.current_realm;
        let f = self.heap.alloc(Object::new(
            Some(proto),
            ObjectKind::Builtin(Box::new(BuiltinFunction {
                func,
                realm,
                constructor,
                slots: Vec::new(),
            })),
        ));
        let k = self.keys.length.clone();
        self.put_data(f, k, Value::Number(f64::from(length)), false, false, true);
        let k = self.keys.name.clone();
        self.put_data(f, k, Value::str(name), false, false, true);
        f
    }

    /// Una built-in anónima con datos propios (funciones creadas en tiempo de ejecución).
    pub fn create_builtin_with_slots(
        &mut self,
        func: NativeFn,
        length: u32,
        name: &str,
        slots: Vec<Value>,
    ) -> Gc<Object> {
        let f = self.create_builtin_function(func, length, name, false);
        if let ObjectKind::Builtin(b) = &mut self.heap[f].kind {
            b.slots = slots;
        }
        f
    }

    /// GetFunctionRealm (§7.3.24).
    pub fn get_function_realm(&mut self, f: Gc<Object>) -> JsResult<RealmId> {
        match &self.heap[f].kind {
            ObjectKind::Function(c) => Ok(c.realm),
            ObjectKind::Builtin(b) => Ok(b.realm),
            ObjectKind::Bound(b) => {
                let t = b.target;
                self.get_function_realm(t)
            }
            // E1: Proxy exótico (§7.3.24 paso 4).
            ObjectKind::Proxy(p) => match p.target {
                Some(t) => self.get_function_realm(t),
                None => self.type_error("Cannot perform operation on a revoked proxy"),
            },
            _ => Ok(self.current_realm),
        }
    }

    /// GetPrototypeFromConstructor (§10.1.14).
    pub fn get_prototype_from_constructor(
        &mut self,
        ctor: Gc<Object>,
        default: fn(&crate::vm::realm::Intrinsics) -> Gc<Object>,
    ) -> JsResult<Gc<Object>> {
        let k = self.keys.prototype.clone();
        let proto = self.get(ctor, &k, Value::Object(ctor))?;
        if let Value::Object(p) = proto {
            return Ok(p);
        }
        let realm = self.get_function_realm(ctor)?;
        Ok(default(self.intrinsics_of(realm)))
    }

    /// OrdinaryCreateFromConstructor (§10.1.13).
    pub fn ordinary_create_from_constructor(
        &mut self,
        ctor: Gc<Object>,
        default: fn(&crate::vm::realm::Intrinsics) -> Gc<Object>,
        kind: ObjectKind,
    ) -> JsResult<Gc<Object>> {
        let proto = self.get_prototype_from_constructor(ctor, default)?;
        Ok(self.heap.alloc(Object::new(Some(proto), kind)))
    }

    // --- Elementos de clase ---

    /// InitializeInstanceElements (§7.3.34).
    pub fn initialize_instance_elements(
        &mut self,
        o: Gc<Object>,
        ctor: Gc<Object>,
    ) -> JsResult<()> {
        let class = match &self.heap[ctor].kind {
            ObjectKind::Function(c) => c.class.as_ref().map(|c| (**c).clone()),
            _ => None,
        };
        let Some(class) = class else { return Ok(()) };
        for (name, m) in class.private_methods {
            self.private_method_or_accessor_add(o, name, m)?;
        }
        for field in class.fields {
            self.define_field(o, &field)?;
        }
        Ok(())
    }

    /// DefineField (§7.3.33).
    pub fn define_field(&mut self, receiver: Gc<Object>, field: &ClassField) -> JsResult<()> {
        let value = match field.initializer {
            Some(init) => self.call(&Value::Object(init), Value::Object(receiver), &[])?,
            None => Value::Undefined,
        };
        match &field.name {
            FieldName::Private(n) => self.private_field_add(receiver, *n, value),
            FieldName::Key(k) => self.create_data_property_or_throw(receiver, k.clone(), value),
        }
    }

    /// PrivateFieldAdd (§7.3.31).
    pub fn private_field_add(
        &mut self,
        o: Gc<Object>,
        name: Gc<Symbol>,
        value: Value,
    ) -> JsResult<()> {
        if self.heap[o].private_find(name).is_some() {
            return self.type_error("Cannot initialize private field twice on the same object");
        }
        self.heap[o]
            .private
            .push((name, PrivateElement::Field(value)));
        Ok(())
    }

    /// PrivateMethodOrAccessorAdd (§7.3.32).
    pub fn private_method_or_accessor_add(
        &mut self,
        o: Gc<Object>,
        name: Gc<Symbol>,
        method: PrivateElement,
    ) -> JsResult<()> {
        if self.heap[o].private_find(name).is_some() {
            return self.type_error("Cannot initialize private methods twice on the same object");
        }
        self.heap[o].private.push((name, method));
        Ok(())
    }

    /// PrivateGet (§7.3.29).
    pub fn private_get(&mut self, o: &Value, name: Gc<Symbol>) -> JsResult<Value> {
        let Value::Object(obj) = o else {
            return self.type_error("Cannot read private member from a non-object");
        };
        let desc = self.heap[name].description.clone().unwrap_or_default();
        match self.heap[*obj].private_find(name).cloned() {
            None => self.type_error(&format!(
                "Cannot read private member {desc} from an object whose class did not declare it"
            )),
            Some(PrivateElement::Field(v)) => Ok(v),
            Some(PrivateElement::Method(m)) => Ok(Value::Object(m)),
            Some(PrivateElement::Accessor { get, .. }) => match get {
                Some(g) => self.call(&Value::Object(g), o.clone(), &[]),
                None => self.type_error(&format!("'{desc}' was defined without a getter")),
            },
        }
    }

    /// PrivateSet (§7.3.30).
    pub fn private_set(&mut self, o: &Value, name: Gc<Symbol>, value: Value) -> JsResult<()> {
        let Value::Object(obj) = o else {
            return self.type_error("Cannot write private member to a non-object");
        };
        let desc = self.heap[name].description.clone().unwrap_or_default();
        let idx = self.heap[*obj].private.iter().position(|(n, _)| *n == name);
        let Some(idx) = idx else {
            return self.type_error(&format!(
                "Cannot write private member {desc} to an object whose class did not declare it"
            ));
        };
        let elem = self.heap[*obj].private[idx].1.clone();
        match elem {
            PrivateElement::Field(_) => {
                self.heap[*obj].private[idx].1 = PrivateElement::Field(value);
                Ok(())
            }
            PrivateElement::Method(_) => {
                self.type_error(&format!("Private method {desc} is not writable"))
            }
            PrivateElement::Accessor { set, .. } => match set {
                Some(s) => {
                    self.call(&Value::Object(s), o.clone(), &[value])?;
                    Ok(())
                }
                None => self.type_error(&format!("'{desc}' was defined without a setter")),
            },
        }
    }

    /// Una propiedad de método (CreateMethodProperty / DefineMethodProperty).
    pub fn define_method_property(
        &mut self,
        o: Gc<Object>,
        key: PropertyKey,
        f: Gc<Object>,
        enumerable: bool,
    ) -> JsResult<()> {
        self.define_property_or_throw(
            o,
            key,
            PropertyDescriptor::data(Value::Object(f), true, enumerable, true),
        )
    }

    /// Lanza `t` en el realm `realm` (para errores que el spec crea en otro realm).
    pub fn in_realm<T>(&mut self, realm: RealmId, f: impl FnOnce(&mut Agent) -> T) -> T {
        let saved = self.current_realm;
        self.current_realm = realm;
        let r = f(self);
        self.current_realm = saved;
        r
    }
}

/// Reanuda un frame (generadores).
pub(crate) fn resume_frame(
    agent: &mut Agent,
    frame: Frame,
    mode: Resume,
) -> (JsResult<FrameExit>, Frame) {
    let saved = agent.current_realm;
    agent.current_realm = frame.realm;
    agent.frames.push(frame);
    let r = agent.run(mode);
    let frame = agent.frames.pop().expect("frame");
    agent.current_realm = saved;
    (r, frame)
}

#[allow(dead_code)]
fn _unused(_: Prop, _: Throw) {}
