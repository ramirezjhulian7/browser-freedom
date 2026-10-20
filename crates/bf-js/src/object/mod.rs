//! Objetos (ECMA-262 §6.1.7, §10): prototipo, extensibilidad, propiedades propias ordenadas,
//! elementos privados y el [`ObjectKind`] que distingue los objetos exóticos y los internal
//! slots de cada clase de objeto. Los métodos internos (`[[GetPrototypeOf]]` …
//! `[[OwnPropertyKeys]]`) están en [`internal`] y despachan por `kind`.

pub mod internal;
mod property_map;
pub mod proxy;

use std::collections::{HashSet, VecDeque};

use crate::bigint::BigInt;
use crate::heap::{Gc, Trace, Tracer};
use crate::string::JsString;
use crate::value::{PropertyKey, Symbol, Value};
use crate::vm::env::Env;
use crate::vm::function::{BoundFunction, BuiltinFunction, Closure};
use crate::vm::generator::GeneratorData;

pub use property_map::{Prop, PropValue, PropertyDescriptor, PropertyMap};

/// PrivateElement (§6.2.10).
#[derive(Debug, Clone)]
pub enum PrivateElement {
    Field(Value),
    Method(Gc<Object>),
    Accessor {
        get: Option<Gc<Object>>,
        set: Option<Gc<Object>>,
    },
}

impl Trace for PrivateElement {
    fn trace(&self, t: &mut Tracer) {
        match self {
            PrivateElement::Field(v) => t.value(v),
            PrivateElement::Method(m) => t.gc(*m),
            PrivateElement::Accessor { get, set } => {
                t.opt(*get);
                t.opt(*set);
            }
        }
    }
}

/// Los elementos de un Array exótico (§10.4.2): un vector denso de propiedades de datos con
/// atributos `{writable, enumerable, configurable}` (los huecos son `None`). Los elementos con
/// otros atributos o demasiado lejos del final van al [`PropertyMap`] del objeto.
#[derive(Debug, Clone)]
pub struct ArrayData {
    pub dense: Vec<Option<Value>>,
    pub length: u32,
    pub length_writable: bool,
}

/// El ParameterMap de un objeto `arguments` mapeado (§10.4.4): el slot del environment de
/// cada parámetro todavía mapeado.
#[derive(Debug, Clone)]
pub struct MappedArguments {
    pub env: Gc<Env>,
    pub map: Vec<Option<u32>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IterKind {
    Keys,
    Values,
    Entries,
}

/// %ArrayIteratorPrototype% (§23.1.5): `object` es `None` cuando terminó.
#[derive(Debug, Clone)]
pub struct ArrayIteratorData {
    pub object: Option<Gc<Object>>,
    pub next_index: f64,
    pub kind: IterKind,
}

/// El iterador interno de `for-in` (§14.7.5.10, CreateForInIterator).
#[derive(Debug, Clone)]
pub struct ForInData {
    pub object: Option<Gc<Object>>,
    pub object_was_visited: bool,
    pub visited: HashSet<PropertyKey>,
    pub remaining: VecDeque<PropertyKey>,
}

#[derive(Debug)]
pub enum ObjectKind {
    Ordinary,
    Array(ArrayData),
    /// Función ECMAScript (§10.2).
    Function(Box<Closure>),
    /// Función built-in (§10.3).
    Builtin(Box<BuiltinFunction>),
    /// Bound function exótica (§10.4.1).
    Bound(Box<BoundFunction>),
    /// String exótico (§10.4.3).
    String(JsString),
    /// `arguments` (§10.4.4): mapeado si hay ParameterMap.
    Arguments(Option<Box<MappedArguments>>),
    /// Instancias de Error (`[[ErrorData]]`).
    Error,
    Boolean(bool),
    Number(f64),
    Symbol(Gc<Symbol>),
    BigInt(Gc<BigInt>),
    Generator(Box<GeneratorData>),
    ArrayIterator(Box<ArrayIteratorData>),
    StringIterator {
        string: Option<JsString>,
        position: usize,
    },
    ForIn(Box<ForInData>),
    /// Un objeto con `[[IsHTMLDDA]]` (B.3.6, `$262.IsHTMLDDA`): llamable, devuelve null.
    HtmlDda,
    // --- E3: Promise, async, colecciones, buffers ---
    /// Instancias de Promise (§27.2.6).
    Promise(Box<crate::builtins::promise::PromiseData>),
    /// El estado interno de una función async suspendida en un `await` (no es visible).
    AsyncFunction(Box<crate::builtins::r#async::AsyncFnData>),
    /// Instancias de AsyncGenerator (§27.6.2).
    AsyncGenerator(Box<crate::builtins::r#async::AsyncGenData>),
    /// %AsyncFromSyncIteratorPrototype% (§27.1.6): `[[SyncIteratorRecord]]`.
    AsyncFromSyncIterator(Box<crate::builtins::r#async::SyncIteratorRecord>),
    /// Map y Set (§24.1, §24.2): las entradas con tombstones.
    Map(Box<crate::builtins::map_set::MapData>),
    Set(Box<crate::builtins::map_set::MapData>),
    MapIterator(Box<crate::builtins::map_set::CollectionIterator>),
    SetIterator(Box<crate::builtins::map_set::CollectionIterator>),
    /// WeakMap, WeakSet, WeakRef y FinalizationRegistry (§24.3, §24.4, §26): referencias
    /// débiles (el GC las limpia).
    WeakMap(Box<crate::builtins::weak::WeakMapData>),
    WeakSet(Box<crate::builtins::weak::WeakMapData>),
    WeakRef(Box<crate::builtins::weak::WeakRefData>),
    FinalizationRegistry(Box<crate::builtins::weak::FinalizationRegistryData>),
    /// ArrayBuffer y SharedArrayBuffer (§25.1, §25.2).
    ArrayBuffer(Box<crate::builtins::array_buffer::ArrayBufferData>),
    /// TypedArray exóticos (§10.4.5).
    TypedArray(Box<crate::builtins::typed_array::TypedArrayData>),
    /// DataView (§25.3).
    DataView(Box<crate::builtins::dataview::DataViewData>),
    /// Un objeto de `JSON.rawJSON` (`[[IsRawJSON]]`).
    RawJson,
    /// Proxy exótico (§10.5).
    Proxy(Box<proxy::ProxyData>),
    /// %IteratorHelperPrototype% (§27.1.2.1).
    IteratorHelper(Box<crate::builtins::iterator_helpers::HelperData>),
    /// %WrapForValidIteratorPrototype% (`[[Iterated]]`).
    WrappedIterator(Box<crate::builtins::iterator_helpers::IterRec>),
    /// Module Namespace exótico (§10.4.6).
    ModuleNamespace(Box<crate::builtins::module_namespace::NamespaceData>),
    // E2: RegExp (§22.2), RegExp String Iterator (§22.2.9) y Date (`[[DateValue]]`, §21.4).
    RegExp(Box<crate::builtins::regexp::RegExpData>),
    RegExpStringIterator(Box<crate::builtins::regexp::RegExpStringIteratorData>),
    Date(f64),
    // E3: objetos de plataforma (`crate::host`).
    Host(Box<crate::host::HostObject>),
}

#[derive(Debug)]
pub struct Object {
    pub proto: Option<Gc<Object>>,
    pub extensible: bool,
    pub props: PropertyMap,
    pub kind: ObjectKind,
    /// `[[PrivateElements]]` (§6.2.10).
    pub private: Vec<(Gc<Symbol>, PrivateElement)>,
}

impl Object {
    pub fn new(proto: Option<Gc<Object>>, kind: ObjectKind) -> Object {
        Object {
            proto,
            extensible: true,
            props: PropertyMap::new(),
            kind,
            private: Vec::new(),
        }
    }

    pub fn ordinary(proto: Option<Gc<Object>>) -> Object {
        Object::new(proto, ObjectKind::Ordinary)
    }

    /// ¿Tiene `[[Call]]`?
    pub fn is_callable(&self) -> bool {
        matches!(
            self.kind,
            ObjectKind::Function(_)
                | ObjectKind::Builtin(_)
                | ObjectKind::Bound(_)
                | ObjectKind::HtmlDda
        ) || matches!(&self.kind, ObjectKind::Proxy(p) if p.callable)
    }

    /// ¿Tiene `[[Construct]]`?
    pub fn is_constructor(&self) -> bool {
        match &self.kind {
            ObjectKind::Function(c) => c.is_constructor,
            ObjectKind::Builtin(b) => b.constructor,
            ObjectKind::Bound(b) => b.constructor,
            ObjectKind::Proxy(p) => p.constructor,
            _ => false,
        }
    }

    pub fn is_array(&self) -> bool {
        matches!(self.kind, ObjectKind::Array(_))
    }

    /// Busca el elemento privado (PrivateElementFind, §7.3.26).
    pub fn private_find(&self, name: Gc<Symbol>) -> Option<&PrivateElement> {
        self.private
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, e)| e)
    }
}

impl Trace for Object {
    fn trace(&self, t: &mut Tracer) {
        t.opt(self.proto);
        self.props.trace(t);
        for (n, e) in &self.private {
            t.gc(*n);
            e.trace(t);
        }
        match &self.kind {
            ObjectKind::Ordinary | ObjectKind::Error | ObjectKind::Boolean(_) => {}
            ObjectKind::Number(_) | ObjectKind::String(_) => {}
            ObjectKind::Array(a) => {
                for v in a.dense.iter().flatten() {
                    t.value(v);
                }
            }
            ObjectKind::Function(c) => c.trace(t),
            ObjectKind::Builtin(b) => b.trace(t),
            ObjectKind::Bound(b) => b.trace(t),
            ObjectKind::Arguments(m) => {
                if let Some(m) = m {
                    t.gc(m.env);
                }
            }
            ObjectKind::Symbol(s) => t.gc(*s),
            ObjectKind::BigInt(b) => t.gc(*b),
            ObjectKind::Generator(g) => g.trace(t),
            ObjectKind::ArrayIterator(it) => t.opt(it.object),
            ObjectKind::StringIterator { .. }
            | ObjectKind::HtmlDda
            | ObjectKind::RawJson
            | ObjectKind::ModuleNamespace(_) => {}
            ObjectKind::Proxy(p) => {
                t.opt(p.target);
                t.opt(p.handler);
            }
            ObjectKind::IteratorHelper(h) => h.trace(t),
            ObjectKind::WrappedIterator(r) => r.trace(t),
            ObjectKind::ForIn(f) => {
                t.opt(f.object);
                for k in f.visited.iter().chain(f.remaining.iter()) {
                    if let PropertyKey::Symbol(s) = k {
                        t.gc(*s);
                    }
                }
            }
            // E2
            ObjectKind::RegExp(_) | ObjectKind::Date(_) => {}
            ObjectKind::RegExpStringIterator(d) => d.trace(t),
            // E3
            ObjectKind::Promise(p) => p.trace(t),
            ObjectKind::AsyncFunction(a) => a.trace(t),
            ObjectKind::AsyncGenerator(g) => g.trace(t),
            ObjectKind::AsyncFromSyncIterator(r) => r.trace(t),
            ObjectKind::Map(m) | ObjectKind::Set(m) => m.trace(t),
            ObjectKind::MapIterator(i) | ObjectKind::SetIterator(i) => i.trace(t),
            ObjectKind::WeakMap(w) | ObjectKind::WeakSet(w) => w.trace(t),
            ObjectKind::WeakRef(w) => w.trace(t),
            ObjectKind::FinalizationRegistry(f) => f.trace(t),
            ObjectKind::ArrayBuffer(_) => {}
            ObjectKind::TypedArray(ta) => t.gc(ta.buffer),
            ObjectKind::DataView(dv) => t.gc(dv.buffer),
            ObjectKind::Host(h) => h.trace(t),
        }
    }
}
