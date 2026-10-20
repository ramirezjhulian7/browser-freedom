//! Heap con recolección de basura mark-sweep sobre arenas indexadas.
//!
//! Cada tipo del heap (objetos, símbolos, BigInts, environments) vive en su [`Arena`]: un
//! vector de slots `{generation, data}` con free list. Un [`Gc<T>`] es un índice tipado más la
//! generación del slot, así que es `Copy` y no necesita `unsafe`; usar un `Gc` cuyo slot ya fue
//! liberado (generación distinta) es un bug del motor y se detecta.
//!
//! La recolección marca desde raíces explícitas (las que entrega el [`crate::vm::Agent`]:
//! realms, frames de la VM, environments y valores vivos; más los [`HandleScope`] de este
//! módulo para los valores temporales del código Rust) y libera todo lo no marcado. Se
//! dispara por umbral de asignaciones o bajo demanda.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::ops::{Index, IndexMut};

use crate::bigint::BigInt;
use crate::object::Object;
use crate::value::{Symbol, Value};
use crate::vm::env::Env;

/// Referencia a un valor del heap.
pub struct Gc<T> {
    index: u32,
    generation: u32,
    _type: PhantomData<fn() -> T>,
}

impl<T> Gc<T> {
    pub fn index(self) -> u32 {
        self.index
    }
}

impl<T> Clone for Gc<T> {
    fn clone(&self) -> Gc<T> {
        *self
    }
}

impl<T> Copy for Gc<T> {}

impl<T> PartialEq for Gc<T> {
    fn eq(&self, other: &Gc<T>) -> bool {
        self.index == other.index && self.generation == other.generation
    }
}

impl<T> Eq for Gc<T> {}

impl<T> Hash for Gc<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.index.hash(state);
        self.generation.hash(state);
    }
}

impl<T> fmt::Debug for Gc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}.{}", self.index, self.generation)
    }
}

struct Slot<T> {
    generation: u32,
    marked: bool,
    data: Option<T>,
}

/// Arena de un tipo del heap.
pub struct Arena<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    live: usize,
}

impl<T> Default for Arena<T> {
    fn default() -> Arena<T> {
        Arena {
            slots: Vec::new(),
            free: Vec::new(),
            live: 0,
        }
    }
}

impl<T> Arena<T> {
    fn alloc(&mut self, data: T) -> Gc<T> {
        self.live += 1;
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.data = Some(data);
            slot.marked = false;
            return Gc {
                index,
                generation: slot.generation,
                _type: PhantomData,
            };
        }
        let index = self.slots.len() as u32;
        self.slots.push(Slot {
            generation: 0,
            marked: false,
            data: Some(data),
        });
        Gc {
            index,
            generation: 0,
            _type: PhantomData,
        }
    }

    pub fn is_alive(&self, gc: Gc<T>) -> bool {
        self.slots
            .get(gc.index as usize)
            .is_some_and(|s| s.generation == gc.generation && s.data.is_some())
    }

    fn get(&self, gc: Gc<T>) -> &T {
        match self.slots.get(gc.index as usize) {
            Some(s) if s.generation == gc.generation => match &s.data {
                Some(d) => d,
                None => panic!("Gc liberado: {gc:?}"),
            },
            _ => panic!("Gc liberado: {gc:?}"),
        }
    }

    fn get_mut(&mut self, gc: Gc<T>) -> &mut T {
        match self.slots.get_mut(gc.index as usize) {
            Some(s) if s.generation == gc.generation => match &mut s.data {
                Some(d) => d,
                None => panic!("Gc liberado: {gc:?}"),
            },
            _ => panic!("Gc liberado: {gc:?}"),
        }
    }

    /// Marca el slot; `true` si no estaba marcado (hay que recorrer sus hijos).
    fn mark(&mut self, index: u32, generation: u32) -> bool {
        match self.slots.get_mut(index as usize) {
            Some(s) if s.generation == generation && s.data.is_some() && !s.marked => {
                s.marked = true;
                true
            }
            _ => false,
        }
    }

    fn sweep(&mut self) -> usize {
        let mut freed = 0;
        for (i, s) in self.slots.iter_mut().enumerate() {
            if s.data.is_some() && !s.marked {
                s.data = None;
                s.generation = s.generation.wrapping_add(1);
                self.free.push(i as u32);
                freed += 1;
            }
            s.marked = false;
        }
        self.live -= freed;
        freed
    }

    pub fn live(&self) -> usize {
        self.live
    }
}

/// Tipos que viven en el heap.
pub trait HeapData: Sized {
    fn arena(heap: &Heap) -> &Arena<Self>;
    fn arena_mut(heap: &mut Heap) -> &mut Arena<Self>;
    fn gc_ref(gc: Gc<Self>) -> GcRef;
}

macro_rules! heap_data {
    ($t:ty, $field:ident, $variant:ident) => {
        impl HeapData for $t {
            fn arena(heap: &Heap) -> &Arena<Self> {
                &heap.$field
            }
            fn arena_mut(heap: &mut Heap) -> &mut Arena<Self> {
                &mut heap.$field
            }
            fn gc_ref(gc: Gc<Self>) -> GcRef {
                GcRef::$variant(gc)
            }
        }
    };
}

heap_data!(Object, objects, Object);
heap_data!(Symbol, symbols, Symbol);
heap_data!(BigInt, bigints, BigInt);
heap_data!(Env, envs, Env);

/// Referencia sin tipo, para la lista de trabajo del marcado.
#[derive(Debug, Clone, Copy)]
pub enum GcRef {
    Object(Gc<Object>),
    Symbol(Gc<Symbol>),
    BigInt(Gc<BigInt>),
    Env(Gc<Env>),
}

/// Acumula lo alcanzable durante el marcado.
#[derive(Default)]
pub struct Tracer {
    work: Vec<GcRef>,
}

impl Tracer {
    pub fn value(&mut self, v: &Value) {
        match v {
            Value::Object(o) => self.work.push(GcRef::Object(*o)),
            Value::Symbol(s) => self.work.push(GcRef::Symbol(*s)),
            Value::BigInt(b) => self.work.push(GcRef::BigInt(*b)),
            _ => {}
        }
    }

    pub fn opt_value(&mut self, v: &Option<Value>) {
        if let Some(v) = v {
            self.value(v);
        }
    }

    pub fn gc<T: HeapData>(&mut self, gc: Gc<T>) {
        self.work.push(T::gc_ref(gc));
    }

    pub fn opt<T: HeapData>(&mut self, gc: Option<Gc<T>>) {
        if let Some(gc) = gc {
            self.gc(gc);
        }
    }
}

/// Lo que puede enumerar sus referencias al heap.
pub trait Trace {
    fn trace(&self, t: &mut Tracer);
}

/// Estadísticas del recolector.
#[derive(Debug, Clone, Copy, Default)]
pub struct GcStats {
    pub live: usize,
    pub collections: usize,
    pub freed_total: usize,
}

/// Marca de un [`HandleScope`] abierto: el tamaño de la pila de handles al abrirlo.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandleScope(usize);

pub struct Heap {
    objects: Arena<Object>,
    symbols: Arena<Symbol>,
    bigints: Arena<BigInt>,
    envs: Arena<Env>,
    /// Valores enraizados por el código Rust ([`HandleScope`]).
    handles: Vec<Value>,
    allocated_since_gc: usize,
    /// Asignaciones entre recolecciones (se ajusta según lo vivo).
    threshold: usize,
    collections: usize,
    freed_total: usize,
}

/// Umbral mínimo de asignaciones entre recolecciones.
const MIN_THRESHOLD: usize = 100_000;

impl Default for Heap {
    fn default() -> Heap {
        Heap {
            objects: Arena::default(),
            symbols: Arena::default(),
            bigints: Arena::default(),
            envs: Arena::default(),
            handles: Vec::new(),
            allocated_since_gc: 0,
            threshold: MIN_THRESHOLD,
            collections: 0,
            freed_total: 0,
        }
    }
}

impl Heap {
    pub fn alloc<T: HeapData>(&mut self, data: T) -> Gc<T> {
        self.allocated_since_gc += 1;
        T::arena_mut(self).alloc(data)
    }

    pub fn is_alive<T: HeapData>(&self, gc: Gc<T>) -> bool {
        T::arena(self).is_alive(gc)
    }

    /// ¿Se superó el umbral de asignaciones?
    pub fn should_collect(&self) -> bool {
        self.allocated_since_gc >= self.threshold
    }

    pub fn set_threshold(&mut self, threshold: usize) {
        self.threshold = threshold.max(1);
    }

    pub fn stats(&self) -> GcStats {
        GcStats {
            live: self.objects.live()
                + self.symbols.live()
                + self.bigints.live()
                + self.envs.live(),
            collections: self.collections,
            freed_total: self.freed_total,
        }
    }

    // --- HandleScope ---

    pub fn open_scope(&self) -> HandleScope {
        HandleScope(self.handles.len())
    }

    /// Enraiza `v` hasta que se cierre el scope abierto más interno.
    pub fn root(&mut self, v: Value) -> Value {
        self.handles.push(v.clone());
        v
    }

    pub fn close_scope(&mut self, scope: HandleScope) {
        self.handles.truncate(scope.0);
    }

    /// Recolecta: marca desde `roots` (más los handles) y libera lo no alcanzado.
    pub fn collect(&mut self, mut roots: Tracer) {
        for v in &self.handles {
            roots.value(v);
        }
        while let Some(r) = roots.work.pop() {
            match r {
                GcRef::Object(o) => {
                    if self.objects.mark(o.index, o.generation) {
                        self.objects.get(o).trace(&mut roots);
                    }
                }
                GcRef::Symbol(s) => {
                    self.symbols.mark(s.index, s.generation);
                }
                GcRef::BigInt(b) => {
                    self.bigints.mark(b.index, b.generation);
                }
                GcRef::Env(e) => {
                    if self.envs.mark(e.index, e.generation) {
                        self.envs.get(e).trace(&mut roots);
                    }
                }
            }
        }
        let freed =
            self.objects.sweep() + self.symbols.sweep() + self.bigints.sweep() + self.envs.sweep();
        self.freed_total += freed;
        self.collections += 1;
        self.allocated_since_gc = 0;
        let live = self.stats().live;
        self.threshold = MIN_THRESHOLD.max(live);
    }
}

impl<T: HeapData> Index<Gc<T>> for Heap {
    type Output = T;
    fn index(&self, gc: Gc<T>) -> &T {
        T::arena(self).get(gc)
    }
}

impl<T: HeapData> IndexMut<Gc<T>> for Heap {
    fn index_mut(&mut self, gc: Gc<T>) -> &mut T {
        T::arena_mut(self).get_mut(gc)
    }
}
