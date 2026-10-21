//! Inline caches (E3: rendimiento de la VM). Cada sitio de acceso por nombre (`GetProp`,
//! `SetProp`, `GetGlobal`…) recuerda hasta [`WAYS`] disposiciones de objeto vistas: el `id` del
//! [`Shape`] del receptor y dónde estaba la propiedad. Con un acierto, la VM lee o escribe el
//! valor sin buscar la clave ni recorrer los métodos internos genéricos; con un fallo usa el
//! camino de la especificación y llena la cache.
//!
//! Solo se cachean los objetos cuyo `[[Get]]`/`[[Set]]` para claves con nombre es el ordinario
//! ([`named_ordinary`]): las propiedades con nombre de esos objetos están todas en su
//! `PropertyMap`.

use std::cell::RefCell;
use std::rc::Rc;

use crate::heap::Gc;
use crate::object::{Object, ObjectKind, Shape};
use crate::value::PropertyKey;
use crate::vm::Agent;

/// `"length"` en code units.
const LENGTH: [u16; 6] = [108, 101, 110, 103, 116, 104];

/// Cuántas disposiciones recuerda cada sitio (caches polimórficas).
pub const WAYS: usize = 4;

/// Una disposición cacheada.
#[derive(Clone, Default)]
pub enum Ic {
    #[default]
    Empty,
    /// Una propiedad propia: el shape del receptor y su posición.
    Own { shape: u64, slot: u32 },
    /// Una propiedad del prototipo directo: el shape del receptor (que no la tiene), el
    /// prototipo, su shape y la posición.
    Proto {
        shape: u64,
        proto: Gc<Object>,
        proto_shape: u64,
        slot: u32,
    },
    /// Una propiedad ausente: el receptor (con ese shape) no la tiene, su prototipo directo
    /// (con ese shape) tampoco y ese prototipo no tiene prototipo. El valor es `undefined`.
    Missing {
        shape: u64,
        proto: Gc<Object>,
        proto_shape: u64,
    },
    /// Una asignación que agrega la propiedad: el shape de partida, el prototipo (la cadena
    /// no tenía setter ni propiedad de solo lectura con ese [`crate::object::setter_epoch`])
    /// y el shape resultante.
    Add {
        from: u64,
        proto: Option<Gc<Object>>,
        epoch: u64,
        to: Rc<Shape>,
    },
    /// La definición de una propiedad de un literal de objeto (`InitProp`): sin setters ni
    /// prototipo que mirar, solo la transición.
    Init { from: u64, to: Rc<Shape> },
    /// Una propiedad propia del objeto global (el registro léxico global tenía `decl_len`
    /// bindings y no tenía el nombre).
    Global { shape: u64, slot: u32, decl_len: u32 },
}

/// Las disposiciones de un sitio.
#[derive(Clone, Default)]
pub struct IcSite {
    pub ways: [Ic; WAYS],
    next: u8,
}

impl IcSite {
    /// Agrega una disposición (reemplaza en ronda cuando el sitio está lleno).
    pub fn push(&mut self, ic: Ic) {
        if let Some(e) = self.ways.iter_mut().find(|e| matches!(e, Ic::Empty)) {
            *e = ic;
            return;
        }
        let i = self.next as usize % WAYS;
        self.ways[i] = ic;
        self.next = self.next.wrapping_add(1);
    }
}

/// Las caches de un código: un sitio por constante de nombre (se dimensiona al primer uso).
#[derive(Default)]
pub struct IcTable {
    sites: RefCell<Vec<IcSite>>,
}

impl std::fmt::Debug for IcTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IcTable({})", self.sites.borrow().len())
    }
}

impl IcTable {
    /// Una copia de las disposiciones del sitio `i`.
    pub fn get(&self, i: u32) -> Option<IcSite> {
        self.sites.borrow().get(i as usize).cloned()
    }

    /// Visita las disposiciones del sitio `i` sin copiarlas.
    pub fn with<R>(&self, i: u32, f: impl FnOnce(&IcSite) -> R) -> Option<R> {
        self.sites.borrow().get(i as usize).map(f)
    }

    pub fn push(&self, i: u32, len: usize, ic: Ic) {
        let mut sites = self.sites.borrow_mut();
        if sites.len() < len {
            sites.resize_with(len, IcSite::default);
        }
        if let Some(s) = sites.get_mut(i as usize) {
            s.push(ic);
        }
    }
}

/// ¿El `[[Get]]`/`[[Set]]` del objeto para la clave con nombre `key` es el ordinario (la
/// propiedad, si es propia, está en su `PropertyMap`)?
pub fn named_ordinary(agent: &Agent, o: Gc<Object>, key: &PropertyKey) -> bool {
    if matches!(key, PropertyKey::Index(_)) {
        return false;
    }
    match &agent.heap[o].kind {
        ObjectKind::Ordinary
        | ObjectKind::Function(_)
        | ObjectKind::Builtin(_)
        | ObjectKind::Bound(_)
        | ObjectKind::Error
        | ObjectKind::Boolean(_)
        | ObjectKind::Number(_)
        | ObjectKind::Promise(_) => true,
        // `length` de un Array no está en su `PropertyMap`.
        ObjectKind::Array(_) => !matches!(key, PropertyKey::String(s) if s.units() == LENGTH),
        ObjectKind::Host(_) => agent.host_exotic(o).is_none(),
        _ => false,
    }
}

use crate::compiler::bytecode::FunctionCode;
use crate::object::{Prop, PropValue, setter_epoch};
use crate::string::Atom;
use crate::value::Value;
use crate::vm::JsResult;

/// El valor de una propiedad leída por una cache (un getter se llama con `receiver`).
fn read(agent: &mut Agent, prop: Option<Prop>, receiver: Value) -> Option<JsResult<Value>> {
    match prop?.value {
        PropValue::Data { value, .. } => Some(Ok(value)),
        PropValue::Accessor { get: None, .. } => Some(Ok(Value::Undefined)),
        PropValue::Accessor { get: Some(g), .. } => {
            Some(agent.call_internal(&Value::Object(g), receiver, &[]))
        }
    }
}

/// Un acierto de escritura.
enum SetHit {
    Own(u32),
    Add(Option<Gc<Object>>, u64, Rc<Shape>),
}

/// El estado de un objeto antes de un `[[Set]]` por el camino general.
pub(crate) struct BeforeSet {
    /// El shape compartido (`None` en modo diccionario) y el `id` de la disposición.
    shape: Option<Rc<Shape>>,
    id: u64,
    proto: Option<Gc<Object>>,
    epoch: u64,
}

impl Agent {
    /// `[[Get]]` de la clave `n` de `code` en `o` por la cache; `None` si no acertó. Los
    /// getters reciben `receiver` (el objeto, o el primitivo cuyo prototipo es `o`).
    pub(crate) fn ic_get(
        &mut self,
        code: &Rc<FunctionCode>,
        n: u32,
        o: Gc<Object>,
        key: &PropertyKey,
        receiver: &Value,
    ) -> Option<JsResult<Value>> {
        let shape = self.heap[o].props.shape_id();
        let hit = code.ics.with(n, |site| {
            site.ways.iter().find_map(|w| match w {
                Ic::Own { shape: s, slot } if *s == shape => Some((None, *slot)),
                Ic::Proto {
                    shape: s,
                    proto,
                    proto_shape,
                    slot,
                } if *s == shape => Some((Some((*proto, *proto_shape)), *slot)),
                Ic::Missing {
                    shape: s,
                    proto,
                    proto_shape,
                } if *s == shape => Some((Some((*proto, *proto_shape)), u32::MAX)),
                _ => None,
            })
        })??;
        if !named_ordinary(self, o, key) {
            return None;
        }
        let prop = match hit {
            (None, slot) => self.heap[o].props.slot(slot as usize).cloned(),
            (Some((proto, proto_shape)), slot) => {
                if self.heap[o].proto != Some(proto)
                    || self.heap[proto].props.shape_id() != proto_shape
                {
                    return None;
                }
                if slot == u32::MAX {
                    // Ausente (Missing): el prototipo no tiene prototipo.
                    if self.heap[proto].proto.is_some() || !named_ordinary(self, proto, key) {
                        return None;
                    }
                    return Some(Ok(Value::Undefined));
                }
                self.heap[proto].props.slot(slot as usize).cloned()
            }
        };
        read(self, prop, receiver.clone())
    }

    /// Después de un `[[Get]]` por el camino general: recuerda dónde estaba la propiedad.
    pub(crate) fn ic_fill_get(
        &mut self,
        code: &Rc<FunctionCode>,
        n: u32,
        o: Gc<Object>,
        key: &PropertyKey,
    ) {
        if !named_ordinary(self, o, key) {
            return;
        }
        let len = code.consts.len();
        let props = &self.heap[o].props;
        if let Some(slot) = props.find_slot(key) {
            let ic = Ic::Own {
                shape: props.shape_id(),
                slot: slot as u32,
            };
            code.ics.push(n, len, ic);
            return;
        }
        let shape = props.shape_id();
        let Some(proto) = self.heap[o].proto else {
            return;
        };
        if !named_ordinary(self, proto, key) {
            return;
        }
        let pp = &self.heap[proto].props;
        if let Some(slot) = pp.find_slot(key) {
            let ic = Ic::Proto {
                shape,
                proto,
                proto_shape: pp.shape_id(),
                slot: slot as u32,
            };
            code.ics.push(n, len, ic);
        } else if self.heap[proto].proto.is_none() {
            let ic = Ic::Missing {
                shape,
                proto,
                proto_shape: pp.shape_id(),
            };
            code.ics.push(n, len, ic);
        }
    }

    /// `[[Set]]` de la clave `n` en `o` por la cache; `false` si no acertó (no hizo nada).
    pub(crate) fn ic_set(
        &mut self,
        code: &Rc<FunctionCode>,
        n: u32,
        o: Gc<Object>,
        key: &PropertyKey,
        v: &Value,
    ) -> bool {
        let shape = self.heap[o].props.shape_id();
        let hit = code.ics.with(n, |site| {
            site.ways.iter().find_map(|w| match w {
                Ic::Own { shape: s, slot } if *s == shape => Some(SetHit::Own(*slot)),
                Ic::Add {
                    from,
                    proto,
                    epoch,
                    to,
                } if *from == shape => Some(SetHit::Add(*proto, *epoch, to.clone())),
                _ => None,
            })
        });
        let Some(Some(hit)) = hit else {
            return false;
        };
        if !named_ordinary(self, o, key) {
            return false;
        }
        match hit {
            SetHit::Own(slot) => match self.heap[o].props.slot_mut(slot as usize) {
                Some(Prop {
                    value:
                        PropValue::Data {
                            value,
                            writable: true,
                        },
                    ..
                }) => {
                    *value = v.clone();
                    true
                }
                _ => false,
            },
            SetHit::Add(proto, epoch, to) => {
                let obj = &mut self.heap[o];
                if obj.proto != proto || !obj.extensible || epoch != setter_epoch() {
                    return false;
                }
                obj.props
                    .push_with_shape(to, Prop::data(v.clone(), true, true, true));
                true
            }
        }
    }

    /// El estado de `o` antes de un `[[Set]]` por el camino general.
    pub(crate) fn ic_before_set(&self, o: Gc<Object>) -> BeforeSet {
        let obj = &self.heap[o];
        BeforeSet {
            shape: obj.props.shape().cloned(),
            id: obj.props.shape_id(),
            proto: obj.proto,
            epoch: setter_epoch(),
        }
    }

    /// Después de un `[[Set]]` por el camino general: recuerda la propiedad escrita o la
    /// transición que la agregó.
    pub(crate) fn ic_fill_set(
        &mut self,
        code: &Rc<FunctionCode>,
        n: u32,
        o: Gc<Object>,
        key: &PropertyKey,
        before: BeforeSet,
    ) {
        if !named_ordinary(self, o, key) {
            return;
        }
        let len = code.consts.len();
        let obj = &self.heap[o];
        if obj.props.shape_id() == before.id {
            if let Some(slot) = obj.props.find_slot(key)
                && matches!(
                    obj.props.slot(slot).map(|p| &p.value),
                    Some(PropValue::Data { writable: true, .. })
                )
            {
                let ic = Ic::Own {
                    shape: before.id,
                    slot: slot as u32,
                };
                code.ics.push(n, len, ic);
            }
            return;
        }
        let (Some(from), Some(after)) = (before.shape, obj.props.shape()) else {
            return;
        };
        // Una transición: el shape de partida compartido, una clave más (la escrita) y nada
        // que cambiara el epoch o el prototipo mientras tanto.
        if after.len() == from.len() + 1
            && after.last_key() == Some(key)
            && obj.proto == before.proto
            && setter_epoch() == before.epoch
        {
            let ic = Ic::Add {
                from: from.id(),
                proto: before.proto,
                epoch: before.epoch,
                to: after.clone(),
            };
            code.ics.push(n, len, ic);
        }
    }

    /// Una propiedad propia del objeto global por la cache (`GetGlobal`); `None` si no acertó.
    pub(crate) fn ic_get_global(
        &mut self,
        code: &Rc<FunctionCode>,
        n: u32,
    ) -> Option<JsResult<Value>> {
        let env = self.global_env();
        let g = self.heap[env].global()?;
        let (object, decl_len) = (g.object, g.decl.len() as u32);
        let shape = self.heap[object].props.shape_id();
        let slot = code.ics.with(n, |site| {
            site.ways.iter().find_map(|w| match w {
                Ic::Global {
                    shape: s,
                    slot,
                    decl_len: d,
                } if *s == shape && *d == decl_len => Some(*slot),
                _ => None,
            })
        })??;
        let prop = self.heap[object].props.slot(slot as usize).cloned();
        read(self, prop, Value::Object(object))
    }

    /// Después de resolver un nombre global por el camino general.
    pub(crate) fn ic_fill_global(
        &mut self,
        code: &Rc<FunctionCode>,
        n: u32,
        name: &Atom,
        key: &PropertyKey,
    ) {
        let env = self.global_env();
        let Some(g) = self.heap[env].global() else {
            return;
        };
        if g.decl.contains_key(name) {
            return;
        }
        let (object, decl_len) = (g.object, g.decl.len() as u32);
        if !named_ordinary(self, object, key) {
            return;
        }
        let props = &self.heap[object].props;
        if let Some(slot) = props.find_slot(key) {
            let ic = Ic::Global {
                shape: props.shape_id(),
                slot: slot as u32,
                decl_len,
            };
            code.ics.push(n, code.consts.len(), ic);
        }
    }
}

impl Agent {
    /// `InitProp` (CreateDataPropertyOrThrow en un literal de objeto) por la cache.
    pub(crate) fn ic_init(&mut self, code: &Rc<FunctionCode>, n: u32, o: Gc<Object>, v: &Value) -> bool {
        let shape = self.heap[o].props.shape_id();
        let to = code.ics.with(n, |site| {
            site.ways.iter().find_map(|w| match w {
                Ic::Init { from, to } if *from == shape => Some(to.clone()),
                _ => None,
            })
        });
        let Some(Some(to)) = to else {
            return false;
        };
        if !matches!(self.heap[o].kind, ObjectKind::Ordinary) || !self.heap[o].extensible {
            return false;
        }
        self.heap[o]
            .props
            .push_with_shape(to, Prop::data(v.clone(), true, true, true));
        true
    }

    /// Después de un `InitProp` por el camino general: la transición que agregó la clave.
    pub(crate) fn ic_fill_init(
        &mut self,
        code: &Rc<FunctionCode>,
        n: u32,
        o: Gc<Object>,
        key: &PropertyKey,
        before: Option<Rc<Shape>>,
    ) {
        if !matches!(self.heap[o].kind, ObjectKind::Ordinary) {
            return;
        }
        let (Some(from), Some(after)) = (before, self.heap[o].props.shape()) else {
            return;
        };
        if after.len() == from.len() + 1 && after.last_key() == Some(key) {
            let ic = Ic::Init {
                from: from.id(),
                to: after.clone(),
            };
            code.ics.push(n, code.consts.len(), ic);
        }
    }
}
