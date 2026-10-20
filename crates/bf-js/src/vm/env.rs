//! Environment Records (§9.1) en el heap.
//!
//! Los bindings que el compilador resuelve estáticamente y que ninguna clausura captura viven
//! en los slots del frame; los capturados (o visibles desde un `eval` directo o un `with`)
//! viven en un [`Env`] del heap, en el slot que les asignó el compilador ([`ScopeLayout`]).
//! Los nombres del layout permiten además la búsqueda dinámica por nombre de §9.1.2.1
//! (GetIdentifierReference) que usan `with`, `eval` y el código global.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::heap::{Gc, Trace, Tracer};
use crate::object::Object;
use crate::string::Atom;
use crate::value::Value;

/// Qué clase de binding es (para SetMutableBinding y la búsqueda dinámica).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    /// `var`, parámetros, funciones, `let`, `catch`: mutables.
    Mutable,
    /// `const`, `class` interno, `using`: inmutable estricto (asignar lanza TypeError).
    Const,
    /// El nombre de una expresión de función con nombre: inmutable no estricto (asignar se
    /// ignora salvo en código estricto).
    FunctionName,
    /// Creado por `eval` (CreateMutableBinding con D = true): se puede borrar.
    Deletable,
}

/// Los nombres y clases de los slots de un scope (lo produce el compilador).
#[derive(Debug, Clone, Default)]
pub struct ScopeLayout {
    pub names: Vec<Atom>,
    pub kinds: Vec<BindingKind>,
    /// Bindings que empiezan inicializados en `undefined` (los demás, sin inicializar).
    pub initialized: Vec<bool>,
    /// ¿Es el environment del parámetro de un `catch`? (B.3.4 permite `var` con el mismo
    /// nombre en un `eval` dentro del catch.)
    pub simple_catch: bool,
}

impl ScopeLayout {
    pub fn position(&self, name: &str) -> Option<usize> {
        self.names.iter().rposition(|n| n.as_str() == name)
    }
}

/// Lo que guarda el Function Environment Record (§9.1.1.3) además de sus bindings.
#[derive(Debug, Clone)]
pub struct FunctionEnvData {
    /// `None`: sin inicializar (constructor derivado antes de `super()`). Las flechas no
    /// tienen Function Environment con `this`, así que este registro implica `this` propio.
    pub this: Option<Value>,
    pub function: Gc<Object>,
    pub new_target: Option<Gc<Object>>,
}

/// Declarative Environment Record (§9.1.1.1), opcionalmente también Function Environment
/// Record (§9.1.1.3).
#[derive(Debug, Clone)]
pub struct DeclEnv {
    pub layout: Rc<ScopeLayout>,
    /// `None`: sin inicializar (TDZ).
    pub slots: Vec<Option<Value>>,
    /// Bindings que agregó un `eval` (siempre `Deletable`).
    pub extra: Vec<(Atom, Option<Value>)>,
    pub function: Option<Box<FunctionEnvData>>,
}

/// Global Environment Record (§9.1.1.4).
#[derive(Debug, Clone)]
pub struct GlobalEnv {
    pub object: Gc<Object>,
    pub this: Gc<Object>,
    /// `[[DeclarativeRecord]]`: nombre → (valor, ¿mutable?).
    pub decl: HashMap<Atom, (Option<Value>, bool)>,
    /// `[[VarNames]]`.
    pub var_names: HashSet<Atom>,
}

#[derive(Debug, Clone)]
pub enum EnvKind {
    Declarative(DeclEnv),
    /// Object Environment Record (§9.1.1.2).
    Object {
        object: Gc<Object>,
        with: bool,
    },
    Global(Box<GlobalEnv>),
}

#[derive(Debug, Clone)]
pub struct Env {
    pub outer: Option<Gc<Env>>,
    pub kind: EnvKind,
}

impl Env {
    pub fn declarative(outer: Option<Gc<Env>>, layout: Rc<ScopeLayout>) -> Env {
        let slots = layout
            .initialized
            .iter()
            .map(|&init| if init { Some(Value::Undefined) } else { None })
            .collect();
        Env {
            outer,
            kind: EnvKind::Declarative(DeclEnv {
                layout,
                slots,
                extra: Vec::new(),
                function: None,
            }),
        }
    }

    pub fn decl(&self) -> Option<&DeclEnv> {
        match &self.kind {
            EnvKind::Declarative(d) => Some(d),
            _ => None,
        }
    }

    pub fn decl_mut(&mut self) -> Option<&mut DeclEnv> {
        match &mut self.kind {
            EnvKind::Declarative(d) => Some(d),
            _ => None,
        }
    }

    pub fn global(&self) -> Option<&GlobalEnv> {
        match &self.kind {
            EnvKind::Global(g) => Some(g),
            _ => None,
        }
    }

    pub fn global_mut(&mut self) -> Option<&mut GlobalEnv> {
        match &mut self.kind {
            EnvKind::Global(g) => Some(g),
            _ => None,
        }
    }
}

impl Trace for Env {
    fn trace(&self, t: &mut Tracer) {
        t.opt(self.outer);
        match &self.kind {
            EnvKind::Declarative(d) => {
                for v in &d.slots {
                    t.opt_value(v);
                }
                for (_, v) in &d.extra {
                    t.opt_value(v);
                }
                if let Some(f) = &d.function {
                    t.opt_value(&f.this);
                    t.gc(f.function);
                    t.opt(f.new_target);
                }
            }
            EnvKind::Object { object, .. } => t.gc(*object),
            EnvKind::Global(g) => {
                t.gc(g.object);
                t.gc(g.this);
                for (v, _) in g.decl.values() {
                    t.opt_value(v);
                }
            }
        }
    }
}

/// Dónde encontró un nombre la búsqueda dinámica.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclSlot {
    Slot(usize),
    Extra(usize),
}

impl DeclEnv {
    /// HasBinding de un Declarative Environment Record.
    pub fn find(&self, name: &str) -> Option<DeclSlot> {
        if let Some(i) = self.extra.iter().position(|(n, _)| n.as_str() == name) {
            return Some(DeclSlot::Extra(i));
        }
        self.layout.position(name).map(DeclSlot::Slot)
    }

    pub fn kind_of(&self, slot: DeclSlot) -> BindingKind {
        match slot {
            DeclSlot::Slot(i) => self.layout.kinds[i],
            DeclSlot::Extra(_) => BindingKind::Deletable,
        }
    }

    pub fn get(&self, slot: DeclSlot) -> &Option<Value> {
        match slot {
            DeclSlot::Slot(i) => &self.slots[i],
            DeclSlot::Extra(i) => &self.extra[i].1,
        }
    }

    pub fn get_mut(&mut self, slot: DeclSlot) -> &mut Option<Value> {
        match slot {
            DeclSlot::Slot(i) => &mut self.slots[i],
            DeclSlot::Extra(i) => &mut self.extra[i].1,
        }
    }
}
