//! Varios agentes (ECMA-262 §9.7, Agents y Agent Clusters; HTML §8.1.2): cada agente es un
//! [`Agent`] en su propio hilo (los agentes no comparten el heap ni los objetos) y lo único
//! que comparten es la memoria de los `SharedArrayBuffer` y las WaiterLists de `Atomics`.
//!
//! # La memoria compartida
//!
//! Un Shared Data Block (§6.2.9) es un [`SharedBlock`]: palabras `AtomicU64` en bloques de
//! 64 KB que se crean al crecer el buffer (un `SharedArrayBuffer` creciente reserva su
//! `maxByteLength` sin tocar la memoria). No hay `unsafe`: no se pueden dar `&[u8]` de una
//! memoria que otro hilo escribe a la vez, así que todo acceso es por palabra:
//!
//! - Un elemento alineado (los de los TypedArray y todos los de `Atomics`) cae entero en una
//!   palabra: leerlo es una carga atómica, escribirlo un `fetch_update` que solo cambia sus
//!   bits, y las operaciones de lectura-modificación-escritura de `Atomics` son
//!   `compare_exchange` sobre la palabra (*lock-free* para 1, 2, 4 y 8 bytes).
//! - Un acceso no alineado (`DataView`) va byte a byte: en el modelo de memoria (§29) los
//!   accesos `unordered` pueden partirse.
//!
//! Todas las operaciones son `SeqCst`: las `seq-cst` del modelo lo necesitan y para las
//! `unordered` es más de lo exigido.
//!
//! # Las WaiterLists (§25.4.3.9–13)
//!
//! Una tabla global (`(id del bloque, índice de byte)` → waiters en orden FIFO) protegida por
//! un `Mutex`, que es la *critical section* del spec: `Atomics.wait` compara el valor y se
//! encola dentro de ella, y `Atomics.notify` desencola dentro de ella. Cada waiter tiene su
//! `Condvar`; uno de `waitAsync` además avisa a la [`AgentSignal`] del agente dueño (que
//! puede estar durmiendo en su bucle) para que resuelva la promesa.
//!
//! # `$262.agent` (test262 INTERPRETING.md)
//!
//! [`install_test262_agent`] agrega a `$262.agent` `start`, `broadcast` y `getReport`; los
//! agentes que crea `start` corren en hilos con su propio `$262.agent` (`receiveBroadcast`,
//! `report`, `leaving`, `sleep`, `monotonicNow`).

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use crate::builtins::array_buffer::ArrayBufferData;
use crate::builtins::method;
use crate::heap::Gc;
use crate::object::{Object, ObjectKind};
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, AgentOptions, JsResult, Throw};

const SEQ: Ordering = Ordering::SeqCst;

/// Un `Mutex` que sigue sirviendo si otro hilo entró en pánico con él tomado.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// --- Shared Data Blocks ---

/// Palabras por bloque de memoria (64 KB).
const CHUNK_WORDS: usize = 8192;

/// Un Shared Data Block (§6.2.9): la memoria de un `SharedArrayBuffer`, compartida por todos
/// los agentes que tienen un `SharedArrayBuffer` sobre ella.
#[derive(Clone)]
pub struct SharedBlock(Arc<BlockInner>);

struct BlockInner {
    id: u64,
    /// El largo actual en bytes (solo crece).
    len: AtomicUsize,
    /// El largo máximo en bytes.
    capacity: usize,
    chunks: Box<[OnceLock<Box<[AtomicU64]>>]>,
}

impl fmt::Debug for SharedBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SharedBlock#{}({} bytes)", self.0.id, self.len())
    }
}

impl SharedBlock {
    /// Un bloque de `len` bytes en cero que puede crecer hasta `capacity`.
    pub fn new(len: usize, capacity: usize) -> SharedBlock {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let capacity = capacity.max(len);
        let words = capacity.div_ceil(8);
        let chunks = (0..words.div_ceil(CHUNK_WORDS))
            .map(|_| OnceLock::new())
            .collect();
        let b = SharedBlock(Arc::new(BlockInner {
            id: NEXT.fetch_add(1, SEQ),
            len: AtomicUsize::new(len),
            capacity,
            chunks,
        }));
        b.ensure(len);
        b
    }

    /// Un identificador único del bloque (la clave de sus WaiterLists).
    pub fn id(&self) -> u64 {
        self.0.id
    }

    pub fn len(&self) -> usize {
        self.0.len.load(SEQ)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn capacity(&self) -> usize {
        self.0.capacity
    }

    /// ¿Es el mismo bloque?
    pub fn same(&self, other: &SharedBlock) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// Crea los bloques de memoria que cubren los primeros `len` bytes.
    fn ensure(&self, len: usize) {
        let words = len.div_ceil(8);
        for c in self.0.chunks.iter().take(words.div_ceil(CHUNK_WORDS)) {
            c.get_or_init(|| (0..CHUNK_WORDS).map(|_| AtomicU64::new(0)).collect());
        }
    }

    /// Crece hasta `new_len` (HostGrowSharedArrayBuffer, §25.2.2.3): `false` si es menor que
    /// el largo actual o mayor que la capacidad. El largo nunca baja aunque crezcan a la vez.
    pub fn grow(&self, new_len: usize) -> bool {
        if new_len > self.0.capacity {
            return false;
        }
        self.ensure(new_len);
        let mut cur = self.0.len.load(SEQ);
        loop {
            if new_len < cur {
                return false;
            }
            match self.0.len.compare_exchange(cur, new_len, SEQ, SEQ) {
                Ok(_) => return true,
                Err(c) => cur = c,
            }
        }
    }

    fn word(&self, w: usize) -> Option<&AtomicU64> {
        self.0
            .chunks
            .get(w / CHUNK_WORDS)?
            .get()
            .map(|c| &c[w % CHUNK_WORDS])
    }

    /// ¿Cabe el acceso entero en una palabra?
    fn in_word(index: usize, size: usize) -> bool {
        index % 8 + size <= 8
    }

    fn mask(size: usize) -> u64 {
        if size >= 8 {
            u64::MAX
        } else {
            (1u64 << (8 * size)) - 1
        }
    }

    pub fn load_byte(&self, index: usize) -> u8 {
        self.word(index / 8)
            .map_or(0, |w| (w.load(SEQ) >> (8 * (index % 8))) as u8)
    }

    pub fn store_byte(&self, index: usize, v: u8) {
        self.store(index, 1, u64::from(v));
    }

    /// Los `size` bytes en `index` como entero little-endian.
    pub fn load(&self, index: usize, size: usize) -> u64 {
        if Self::in_word(index, size) {
            let shift = 8 * (index % 8);
            return self
                .word(index / 8)
                .map_or(0, |w| (w.load(SEQ) >> shift) & Self::mask(size));
        }
        (0..size).fold(0u64, |acc, i| {
            acc | u64::from(self.load_byte(index + i)) << (8 * i)
        })
    }

    /// Escribe los `size` bytes de `v` (little-endian) en `index` sin tocar los demás bytes
    /// de la palabra.
    pub fn store(&self, index: usize, size: usize, v: u64) {
        if Self::in_word(index, size) {
            let shift = 8 * (index % 8);
            let m = Self::mask(size) << shift;
            let bits = (v << shift) & m;
            if let Some(w) = self.word(index / 8) {
                let _ = w.fetch_update(SEQ, SEQ, |x| Some((x & !m) | bits));
            }
            return;
        }
        for i in 0..size {
            self.store(index + i, 1, (v >> (8 * i)) & 0xFF);
        }
    }

    /// Lectura-modificación-escritura atómica de un elemento alineado: devuelve el valor
    /// anterior; `f` recibe el anterior y devuelve el nuevo (o `None` para no escribir, como
    /// `compareExchange` cuando no coincide).
    pub fn rmw(&self, index: usize, size: usize, f: impl Fn(u64) -> Option<u64>) -> u64 {
        debug_assert!(Self::in_word(index, size));
        let shift = 8 * (index % 8);
        let m = Self::mask(size);
        let Some(w) = self.word(index / 8) else {
            return 0;
        };
        let mut old = 0;
        let _ = w.fetch_update(SEQ, SEQ, |x| {
            old = (x >> shift) & m;
            f(old).map(|n| (x & !(m << shift)) | ((n & m) << shift))
        });
        old
    }

    /// Copia `out.len()` bytes desde `start`.
    pub fn read(&self, start: usize, out: &mut [u8]) {
        let mut i = 0;
        while i < out.len() {
            let at = start + i;
            if at.is_multiple_of(8) && out.len() - i >= 8 {
                let v = self.word(at / 8).map_or(0, |w| w.load(SEQ));
                out[i..i + 8].copy_from_slice(&v.to_le_bytes());
                i += 8;
            } else {
                out[i] = self.load_byte(at);
                i += 1;
            }
        }
    }

    /// Escribe `bytes` desde `start`.
    pub fn write(&self, start: usize, bytes: &[u8]) {
        let mut i = 0;
        while i < bytes.len() {
            let at = start + i;
            if at.is_multiple_of(8) && bytes.len() - i >= 8 {
                let v = u64::from_le_bytes(bytes[i..i + 8].try_into().expect("8 bytes"));
                if let Some(w) = self.word(at / 8) {
                    w.store(v, SEQ);
                }
                i += 8;
            } else {
                self.store_byte(at, bytes[i]);
                i += 1;
            }
        }
    }

    /// Una copia de los bytes `[start, start + len)`.
    pub fn to_vec(&self, start: usize, len: usize) -> Vec<u8> {
        let mut v = vec![0; len];
        self.read(start, &mut v);
        v
    }
}

// --- Señales entre agentes ---

/// Por dónde otro hilo despierta a un agente (que puede estar durmiendo en su bucle de
/// eventos o en RunJobs): un contador de avisos con su `Condvar`, y un *waker* opcional del
/// embebedor (el del bucle de la pestaña o del worker).
#[derive(Default)]
pub struct AgentSignal {
    count: Mutex<u64>,
    cv: Condvar,
    waker: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl fmt::Debug for AgentSignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AgentSignal({})", *lock(&self.count))
    }
}

impl AgentSignal {
    /// Avisa al agente.
    pub fn signal(&self) {
        *lock(&self.count) += 1;
        self.cv.notify_all();
        let w = lock(&self.waker).clone();
        if let Some(w) = w {
            w();
        }
    }

    /// Cuántos avisos llegaron hasta ahora.
    pub fn seen(&self) -> u64 {
        *lock(&self.count)
    }

    /// Espera un aviso posterior a `seen` hasta `until` (`None`: sin límite). `true` si llegó.
    pub fn wait(&self, seen: u64, until: Option<Instant>) -> bool {
        let mut c = lock(&self.count);
        loop {
            if *c != seen {
                return true;
            }
            match until {
                None => c = self.cv.wait(c).unwrap_or_else(|e| e.into_inner()),
                Some(t) => {
                    let now = Instant::now();
                    if now >= t {
                        return false;
                    }
                    c = self
                        .cv
                        .wait_timeout(c, t - now)
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
                }
            }
        }
    }

    /// El *waker* del embebedor: se llama (desde cualquier hilo) en cada aviso.
    pub fn set_waker(&self, waker: Option<Arc<dyn Fn() + Send + Sync>>) {
        *lock(&self.waker) = waker;
    }
}

// --- WaiterLists (§25.4.3.9–13) ---

/// Un waiter de `Atomics.wait` o `Atomics.waitAsync`.
pub struct Waiter {
    notified: Mutex<bool>,
    cv: Condvar,
    /// El agente dueño de un waiter de `waitAsync`.
    owner: Option<Arc<AgentSignal>>,
    key: (u64, usize),
}

impl fmt::Debug for Waiter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Waiter{:?}", self.key)
    }
}

impl Waiter {
    pub fn is_notified(&self) -> bool {
        *lock(&self.notified)
    }

    /// ¿Es de este agente?
    pub fn owned_by(&self, signal: &Arc<AgentSignal>) -> bool {
        self.owner.as_ref().is_some_and(|o| Arc::ptr_eq(o, signal))
    }
}

type WaiterTable = HashMap<(u64, usize), VecDeque<Arc<Waiter>>>;

/// Las WaiterLists de todos los agentes; su `Mutex` es la critical section (§25.4.3.12).
fn waiters() -> &'static Mutex<WaiterTable> {
    static W: OnceLock<Mutex<WaiterTable>> = OnceLock::new();
    W.get_or_init(Mutex::default)
}

/// El resultado de esperar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitResult {
    NotEqual,
    Ok,
    TimedOut,
    /// Se agotó el tiempo del agente (o lo terminaron).
    Terminated,
}

/// Encola un waiter si el elemento `(index, size)` del bloque vale `expected` (todo dentro
/// de la critical section, DoWait pasos 19–27).
fn enqueue(
    block: &SharedBlock,
    index: usize,
    size: usize,
    expected: u64,
    owner: Option<Arc<AgentSignal>>,
) -> Option<Arc<Waiter>> {
    let mut table = lock(waiters());
    if block.load(index, size) != expected {
        return None;
    }
    let key = (block.id(), index);
    let w = Arc::new(Waiter {
        notified: Mutex::new(false),
        cv: Condvar::new(),
        owner,
        key,
    });
    table.entry(key).or_default().push_back(w.clone());
    Some(w)
}

/// Saca el waiter de su lista si sigue ahí (`true`); `false` si ya lo notificaron.
pub fn remove_waiter(w: &Arc<Waiter>) -> bool {
    let mut table = lock(waiters());
    let Some(list) = table.get_mut(&w.key) else {
        return false;
    };
    let before = list.len();
    list.retain(|x| !Arc::ptr_eq(x, w));
    let removed = list.len() != before;
    if list.is_empty() {
        table.remove(&w.key);
    }
    removed
}

/// `Atomics.wait` (DoWait síncrono): suspende el hilo hasta que lo notifiquen, venza
/// `timeout` o llegue `deadline` (el límite del agente).
pub fn wait(
    block: &SharedBlock,
    index: usize,
    size: usize,
    expected: u64,
    timeout: Option<Duration>,
    deadline: Option<Instant>,
) -> WaitResult {
    let Some(w) = enqueue(block, index, size, expected, None) else {
        return WaitResult::NotEqual;
    };
    let start = Instant::now();
    let timeout_at = timeout.and_then(|t| start.checked_add(t));
    let limit = match (timeout_at, deadline) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let mut n = lock(&w.notified);
    loop {
        if *n {
            return WaitResult::Ok;
        }
        let now = Instant::now();
        match limit {
            Some(l) if now >= l => break,
            Some(l) => {
                n =
                    w.cv.wait_timeout(n, l - now)
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
            }
            None => n = w.cv.wait(n).unwrap_or_else(|e| e.into_inner()),
        }
    }
    drop(n);
    // Venció: si nadie lo notificó mientras tanto, sale de la lista.
    if !remove_waiter(&w) {
        return WaitResult::Ok;
    }
    if timeout_at.is_some_and(|t| Instant::now() >= t) {
        WaitResult::TimedOut
    } else {
        WaitResult::Terminated
    }
}

/// `Atomics.waitAsync` (DoWait asíncrono): el waiter, o `None` si el valor no coincide.
pub fn wait_async(
    block: &SharedBlock,
    index: usize,
    size: usize,
    expected: u64,
    owner: Arc<AgentSignal>,
) -> Option<Arc<Waiter>> {
    enqueue(block, index, size, expected, Some(owner))
}

/// `Atomics.notify` (§25.4.12): despierta hasta `count` waiters del elemento, en orden; los
/// waiters despertados.
pub fn notify(block: &SharedBlock, index: usize, count: usize) -> Vec<Arc<Waiter>> {
    let mut table = lock(waiters());
    let key = (block.id(), index);
    let Some(list) = table.get_mut(&key) else {
        return Vec::new();
    };
    let n = count.min(list.len());
    let woken: Vec<Arc<Waiter>> = list.drain(..n).collect();
    if list.is_empty() {
        table.remove(&key);
    }
    drop(table);
    for w in &woken {
        *lock(&w.notified) = true;
        w.cv.notify_all();
        if let Some(o) = &w.owner {
            o.signal();
        }
    }
    woken
}

// --- `$262.agent` con varios agentes (test262 INTERPRETING.md) ---

/// El valor de un broadcast: un Int32 (Number) o un BigInt.
#[derive(Debug, Clone)]
enum BroadcastValue {
    Number(f64),
    BigInt(crate::bigint::BigInt),
}

#[derive(Default)]
struct MailState {
    message: Option<(SharedBlock, Option<usize>, BroadcastValue)>,
}

/// El buzón de un agente hijo.
#[derive(Default)]
struct Mailbox {
    state: Mutex<MailState>,
    cv: Condvar,
}

/// Lo que comparten el agente principal de un test y los que creó.
#[derive(Default)]
struct GroupInner {
    reports: Mutex<VecDeque<String>>,
    children: Mutex<Vec<Arc<Mailbox>>>,
    /// El test terminó: los agentes que esperan un broadcast se van.
    closed: AtomicBool,
}

/// Los agentes de `$262.agent.start` de un agente principal; al soltarlo (el test terminó),
/// los hijos que esperan un broadcast terminan.
#[derive(Default)]
pub struct AgentGroup(Arc<GroupInner>);

impl fmt::Debug for AgentGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AgentGroup")
    }
}

impl Drop for AgentGroup {
    fn drop(&mut self) {
        self.0.closed.store(true, SEQ);
        for m in lock(&self.0.children).iter() {
            let _g = lock(&m.state);
            m.cv.notify_all();
        }
    }
}

/// Agrega `start`, `broadcast` y `getReport` al `$262.agent` del agente principal.
pub fn install_test262_agent(agent: &mut Agent, o: Gc<Object>) {
    method(agent, o, "start", 1, agent_start);
    method(agent, o, "broadcast", 2, agent_broadcast);
    method(agent, o, "getReport", 0, agent_get_report);
}

fn group(agent: &mut Agent) -> Arc<GroupInner> {
    agent
        .agent_group
        .get_or_insert_with(AgentGroup::default)
        .0
        .clone()
}

/// `$262.agent.start(source)`: corre `source` en un agente nuevo, en su hilo; vuelve cuando
/// el agente está corriendo.
fn agent_start(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let src = agent.to_string(&a.arg(0))?.to_string_lossy();
    let g = group(agent);
    let mailbox = Arc::new(Mailbox::default());
    lock(&g.children).push(mailbox.clone());
    let options = AgentOptions {
        max_stack: agent.options.max_stack,
        max_frames: agent.options.max_frames,
        instruction_budget: None,
        deadline: agent.options.deadline,
    };
    let started = Arc::new((Mutex::new(false), Condvar::new()));
    let s2 = started.clone();
    let spawned = std::thread::Builder::new()
        .name("test262-agent".into())
        .stack_size(options.max_stack + (16 << 20))
        .spawn(move || run_child(src, options, g, mailbox, s2));
    if spawned.is_err() {
        return agent.internal_error("no se pudo crear el hilo del agente");
    }
    let (m, cv) = &*started;
    let mut s = lock(m);
    while !*s {
        s = cv.wait(s).unwrap_or_else(|e| e.into_inner());
    }
    Ok(Value::Undefined)
}

/// Lo que guarda el agente hijo (dato del embebedor).
struct ChildState {
    group: Arc<GroupInner>,
    mailbox: Arc<Mailbox>,
    callback: Option<crate::host::HostHandle>,
    leaving: bool,
}

fn run_child(
    src: String,
    options: AgentOptions,
    group: Arc<GroupInner>,
    mailbox: Arc<Mailbox>,
    started: Arc<(Mutex<bool>, Condvar)>,
) {
    let mut agent = Agent::new(options);
    agent.reset_stack_base();
    agent.can_block = true;
    let realm = agent.current_realm;
    crate::builtins::host::install_print(&mut agent, realm);
    let g = agent.realms[realm].global_object;
    let o262 = agent.new_plain_object();
    let ao = agent.new_plain_object();
    method(
        &mut agent,
        ao,
        "receiveBroadcast",
        1,
        child_receive_broadcast,
    );
    method(&mut agent, ao, "report", 1, child_report);
    method(&mut agent, ao, "leaving", 0, child_leaving);
    crate::builtins::atomics::install_child_hooks(&mut agent, ao);
    agent.put_data(
        o262,
        PropertyKey::from("agent"),
        Value::Object(ao),
        true,
        false,
        true,
    );
    agent.put_data(
        g,
        PropertyKey::from("$262"),
        Value::Object(o262),
        true,
        false,
        true,
    );
    agent.set_embedder_data(ChildState {
        group: group.clone(),
        mailbox: mailbox.clone(),
        callback: None,
        leaving: false,
    });
    {
        let (m, cv) = &*started;
        *lock(m) = true;
        cv.notify_all();
    }
    match agent.eval_script(&src, "agent.js") {
        Ok(_) => {}
        Err(Throw::Value(e)) => agent.job_errors.push(e),
        Err(_) => return,
    }
    if agent.run_jobs().is_err() {
        return;
    }
    // Espera broadcasts mientras haya a quién entregárselos.
    loop {
        let (has_cb, leaving) = agent
            .embedder_data::<ChildState>()
            .map_or((false, true), |c| (c.callback.is_some(), c.leaving));
        if !has_cb || leaving {
            return;
        }
        let msg = {
            let mut st = lock(&mailbox.state);
            loop {
                if let Some(m) = st.message.take() {
                    mailbox.cv.notify_all();
                    break Some(m);
                }
                if group.closed.load(SEQ) {
                    break None;
                }
                let now = Instant::now();
                let limit = agent.options.deadline;
                if limit.is_some_and(|l| now >= l) {
                    break None;
                }
                let wait = limit.map_or(Duration::from_millis(100), |l| {
                    (l - now).min(Duration::from_millis(100))
                });
                st = mailbox
                    .cv
                    .wait_timeout(st, wait)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        };
        let Some((block, max, value)) = msg else {
            return;
        };
        let sab = shared_array_buffer(&mut agent, block, max);
        let v = match value {
            BroadcastValue::Number(n) => Value::Number(n),
            BroadcastValue::BigInt(b) => Value::BigInt(agent.heap.alloc(b)),
        };
        let cb = agent
            .embedder_data::<ChildState>()
            .and_then(|c| c.callback)
            .and_then(|h| agent.host_pinned(h));
        if let Some(cb) = cb {
            match agent.call(&cb, Value::Undefined, &[Value::Object(sab), v]) {
                Ok(_) => {}
                Err(Throw::Value(e)) => agent.job_errors.push(e),
                Err(_) => return,
            }
        }
        if agent.run_jobs().is_err() {
            return;
        }
    }
}

/// Un `SharedArrayBuffer` nuevo de este agente sobre `block`.
pub fn shared_array_buffer(
    agent: &mut Agent,
    block: SharedBlock,
    max: Option<usize>,
) -> Gc<Object> {
    let proto = agent.intrinsics().shared_array_buffer_prototype;
    agent.alloc(Object::new(
        Some(proto),
        ObjectKind::ArrayBuffer(Box::new(ArrayBufferData {
            data: None,
            max_byte_length: max,
            shared: true,
            block: Some(block),
        })),
    ))
}

fn child_receive_broadcast(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let h = agent.host_pin(a.arg(0));
    if let Some(c) = agent.embedder_data_mut::<ChildState>() {
        c.callback = Some(h);
    }
    Ok(Value::Undefined)
}

fn child_report(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = agent.to_string(&a.arg(0))?.to_string_lossy();
    if let Some(c) = agent.embedder_data::<ChildState>() {
        lock(&c.group.reports).push_back(s);
    }
    Ok(Value::Undefined)
}

fn child_leaving(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    if let Some(c) = agent.embedder_data_mut::<ChildState>() {
        c.leaving = true;
        let _ = &c.mailbox;
    }
    Ok(Value::Undefined)
}

/// `$262.agent.broadcast(sab, value)`: a todos los agentes; vuelve cuando todos lo tomaron.
fn agent_broadcast(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (block, max) = match &a.arg(0) {
        Value::Object(o) => match &agent.heap[*o].kind {
            ObjectKind::ArrayBuffer(d) if d.shared => match &d.block {
                Some(b) => (b.clone(), d.max_byte_length),
                None => return agent.type_error("broadcast: not a SharedArrayBuffer"),
            },
            _ => return agent.type_error("broadcast: not a SharedArrayBuffer"),
        },
        _ => return agent.type_error("broadcast: not a SharedArrayBuffer"),
    };
    let value = match a.arg(1) {
        Value::BigInt(b) => BroadcastValue::BigInt(agent.heap[b].clone()),
        Value::Undefined => BroadcastValue::Number(0.0),
        v => BroadcastValue::Number(agent.to_number(&v)?),
    };
    let g = group(agent);
    let children: Vec<Arc<Mailbox>> = lock(&g.children).clone();
    for m in &children {
        lock(&m.state).message = Some((block.clone(), max, value.clone()));
        m.cv.notify_all();
    }
    let deadline = agent.options.deadline;
    for m in &children {
        let mut st = lock(&m.state);
        while st.message.is_some() {
            let now = Instant::now();
            if deadline.is_some_and(|d| now >= d) {
                return Err(Throw::Terminate("tiempo agotado".into()));
            }
            st =
                m.cv.wait_timeout(st, Duration::from_millis(50))
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
        }
    }
    Ok(Value::Undefined)
}

/// `$262.agent.getReport()`: el informe más antiguo de cualquier agente, o `null`.
fn agent_get_report(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    let g = group(agent);
    let r = lock(&g.reports).pop_front();
    Ok(r.map_or(Value::Null, |s| Value::str(&s)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_words_and_bytes() {
        let b = SharedBlock::new(20, 20);
        b.store(0, 4, 0xdead_beef);
        b.store(4, 2, 0x1234);
        b.store(7, 2, 0xabcd); // cruza la palabra
        assert_eq!(b.load(0, 4), 0xdead_beef);
        assert_eq!(b.load(4, 2), 0x1234);
        assert_eq!(b.load(7, 2), 0xabcd);
        assert_eq!(b.load_byte(8), 0xab);
        let old = b.rmw(0, 4, |x| Some(x.wrapping_add(1)));
        assert_eq!(old, 0xdead_beef);
        assert_eq!(b.load(0, 4), 0xdead_bef0);
        let mut out = [0u8; 20];
        b.read(0, &mut out);
        assert_eq!(&out[..4], &0xdead_bef0u32.to_le_bytes());
        b.write(13, &[1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(b.to_vec(13, 7), vec![1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn grow_is_lazy_and_monotonic() {
        let b = SharedBlock::new(8, 1 << 30);
        assert!(b.grow(1 << 20));
        assert!(!b.grow(16), "no se achica");
        b.store((1 << 20) - 4, 4, 7);
        assert_eq!(b.load((1 << 20) - 4, 4), 7);
        assert!(!b.grow((1 << 30) + 1));
    }

    #[test]
    fn concurrent_adds_are_atomic() {
        let b = SharedBlock::new(16, 16);
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let b = b.clone();
                std::thread::spawn(move || {
                    for _ in 0..10_000 {
                        b.rmw(4, 4, |x| Some(x + 1));
                        b.rmw(1, 1, |x| Some((x + 1) & 0xFF));
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().expect("hilo");
        }
        assert_eq!(b.load(4, 4), 80_000);
        assert_eq!(b.load(1, 1), 80_000 & 0xFF);
        assert_eq!(b.load(0, 1), 0, "los bytes vecinos no cambian");
    }

    #[test]
    fn wait_and_notify_between_threads() {
        let b = SharedBlock::new(8, 8);
        assert_eq!(
            wait(&b, 0, 4, 1, Some(Duration::from_millis(1)), None),
            WaitResult::NotEqual
        );
        assert_eq!(
            wait(&b, 0, 4, 0, Some(Duration::from_millis(5)), None),
            WaitResult::TimedOut
        );
        let b2 = b.clone();
        let t = std::thread::spawn(move || wait(&b2, 0, 4, 0, None, None));
        // Hasta que el otro hilo esté en la lista.
        let mut woken = Vec::new();
        for _ in 0..1000 {
            woken = notify(&b, 0, 1);
            if !woken.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(woken.len(), 1);
        assert_eq!(t.join().expect("hilo"), WaitResult::Ok);
    }

    #[test]
    fn async_waiters_signal_their_agent() {
        let b = SharedBlock::new(8, 8);
        let signal = Arc::new(AgentSignal::default());
        let seen = signal.seen();
        let w = wait_async(&b, 0, 4, 0, signal.clone()).expect("waiter");
        assert!(!w.is_notified());
        let b2 = b.clone();
        std::thread::spawn(move || notify(&b2, 0, usize::MAX))
            .join()
            .expect("hilo");
        assert!(signal.wait(seen, Some(Instant::now() + Duration::from_secs(5))));
        assert!(w.is_notified() && w.owned_by(&signal));
        assert!(!remove_waiter(&w), "ya no está en la lista");
    }
}
