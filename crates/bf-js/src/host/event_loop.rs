//! El bucle de eventos de HTML (§8.1.7.3 "event loop processing model", sin la
//! actualización del render): colas de tareas por *task source*, el mapa de
//! temporizadores activos (§8.6, "timer initialization steps"), el punto de control de
//! microtasks (§8.1.7.3 "perform a microtask checkpoint") y los callbacks de animation
//! frame (§8.11). El reloj es `HostHooks::now()`; el embebedor pregunta `next_wake()` para
//! saber cuándo volver a llamar a `run_one_task`.
//!
//! También instala los globales que delegan aquí: `setTimeout`, `setInterval`,
//! `clearTimeout`, `clearInterval`, `queueMicrotask`, `requestAnimationFrame`,
//! `cancelAnimationFrame` y `console`.

use std::collections::{BTreeMap, VecDeque};

use crate::builtins::promise::Job;
use crate::builtins::{method, slots};
use crate::heap::{Trace, Tracer};
use crate::host::hooks::{ConsoleLevel, TaskSource};
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId, Throw};

/// El identificador de un temporizador (`setTimeout` devuelve > 0).
pub type TimerId = u32;

/// El *handler* de un temporizador: una función o código fuente.
#[derive(Debug, Clone)]
pub enum TimerHandler {
    Callback(Value),
    Code(JsString),
}

#[derive(Debug)]
struct Timer {
    realm: RealmId,
    handler: TimerHandler,
    args: Vec<Value>,
    when: f64,
    timeout: f64,
    repeat: bool,
    seq: u64,
    /// El *timer nesting level* de su tarea.
    nesting: u32,
    /// ¿Ya hay una tarea en la cola para este disparo?
    queued: bool,
}

/// Una tarea.
pub enum TaskKind {
    /// Código del embebedor (que debe fijar con `host_pin` los valores que capture).
    Native(Box<dyn FnOnce(&mut Agent)>),
    /// Llamar a una función de JavaScript.
    Callback {
        callback: Value,
        this: Value,
        args: Vec<Value>,
    },
    /// Disparar un temporizador.
    Timer(TimerId),
}

struct QueuedTask {
    seq: u64,
    kind: TaskKind,
}

#[derive(Default)]
pub(crate) struct EventLoop {
    queues: [VecDeque<QueuedTask>; 6],
    timers: BTreeMap<TimerId, Timer>,
    next_timer: TimerId,
    seq: u64,
    /// El *timer nesting level* de la tarea de temporizador en curso (0 fuera de ellas).
    current_nesting: Option<u32>,
    /// Los callbacks de animation frame pendientes (§8.11.1).
    animation: Vec<(u32, Value)>,
    next_animation: u32,
}

impl Trace for EventLoop {
    fn trace(&self, t: &mut Tracer) {
        for q in &self.queues {
            for task in q {
                if let TaskKind::Callback {
                    callback,
                    this,
                    args,
                } = &task.kind
                {
                    t.value(callback);
                    t.value(this);
                    for a in args {
                        t.value(a);
                    }
                }
            }
        }
        for timer in self.timers.values() {
            if let TimerHandler::Callback(c) = &timer.handler {
                t.value(c);
            }
            for a in &timer.args {
                t.value(a);
            }
        }
        for (_, cb) in &self.animation {
            t.value(cb);
        }
    }
}

impl EventLoop {
    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }
}

impl Agent {
    // --- Tareas ---

    /// Encola una tarea del embebedor en `source`.
    pub fn queue_task(&mut self, source: TaskSource, task: Box<dyn FnOnce(&mut Agent)>) {
        self.push_task(source, TaskKind::Native(task));
    }

    /// Encola una tarea que llama a `callback` con `this` y `args`.
    pub fn queue_callback_task(
        &mut self,
        source: TaskSource,
        callback: Value,
        this: Value,
        args: Vec<Value>,
    ) {
        self.push_task(
            source,
            TaskKind::Callback {
                callback,
                this,
                args,
            },
        );
    }

    fn push_task(&mut self, source: TaskSource, kind: TaskKind) {
        let seq = self.host.event_loop.next_seq();
        self.host.event_loop.queues[source.index()].push_back(QueuedTask { seq, kind });
        self.host.hooks.task_queued(source);
    }

    /// ¿Hay tareas encoladas en alguna cola?
    pub fn has_pending_tasks(&self) -> bool {
        self.host.event_loop.queues.iter().any(|q| !q.is_empty())
    }

    /// Cuándo hay que volver a llamar a `run_one_task`: ahora si hay tareas, el vencimiento
    /// del próximo temporizador, o `None` si no hay nada pendiente.
    pub fn next_wake(&mut self) -> Option<f64> {
        let now = self.host.hooks.now();
        if self.has_pending_tasks() {
            return Some(now);
        }
        self.host
            .event_loop
            .timers
            .values()
            .filter(|t| !t.queued)
            .map(|t| t.when)
            .reduce(f64::min)
    }

    /// Una iteración del bucle de eventos (§8.1.7.3 pasos 1–2 y 5): los temporizadores
    /// vencidos pasan a la cola de *timers*, se ejecuta la tarea más antigua de todas las
    /// colas y después el punto de control de microtasks. `false` si no había ninguna.
    pub fn run_one_task(&mut self) -> JsResult<bool> {
        let now = self.host.hooks.now();
        let el = &mut self.host.event_loop;
        let mut due: Vec<(f64, u64, TimerId)> = el
            .timers
            .iter()
            .filter(|(_, t)| !t.queued && t.when <= now)
            .map(|(id, t)| (t.when, t.seq, *id))
            .collect();
        due.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        for (_, _, id) in due {
            if let Some(t) = el.timers.get_mut(&id) {
                t.queued = true;
            }
            let seq = el.next_seq();
            el.queues[TaskSource::Timers.index()].push_back(QueuedTask {
                seq,
                kind: TaskKind::Timer(id),
            });
        }
        let pick = el
            .queues
            .iter()
            .enumerate()
            .filter_map(|(i, q)| q.front().map(|t| (t.seq, i)))
            .min();
        let Some((_, qi)) = pick else {
            return Ok(false);
        };
        let task = el.queues[qi].pop_front().expect("tarea");
        self.run_task(task.kind)?;
        self.perform_microtask_checkpoint()?;
        Ok(true)
    }

    /// Ejecuta tareas mientras haya alguna lista para correr ahora; devuelve cuántas.
    pub fn run_until_idle(&mut self) -> JsResult<usize> {
        let mut n = 0;
        while self.run_one_task()? {
            n += 1;
        }
        Ok(n)
    }

    fn run_task(&mut self, kind: TaskKind) -> JsResult<()> {
        match kind {
            TaskKind::Native(f) => {
                f(self);
                Ok(())
            }
            TaskKind::Callback {
                callback,
                this,
                args,
            } => {
                let r = self.call_rooted(&callback, this, &args);
                self.report_result(r)
            }
            TaskKind::Timer(id) => self.run_timer(id),
        }
    }

    /// Llama a `f` desde el bucle (sin inhibir el GC: los valores quedan enraizados).
    pub(crate) fn call_rooted(
        &mut self,
        f: &Value,
        this: Value,
        args: &[Value],
    ) -> JsResult<Value> {
        let base = self.host.running.len();
        self.host.running.push(f.clone());
        self.host.running.push(this.clone());
        self.host.running.extend_from_slice(args);
        let r = if self.gc_inhibit == 0 && self.frames.is_empty() {
            self.call_internal(f, this, args)
        } else {
            self.call(f, this, args)
        };
        self.host.running.truncate(base);
        r
    }

    /// Reporta una excepción y sigue (HTML "report an exception"); la terminación se propaga.
    fn report_result(&mut self, r: JsResult<Value>) -> JsResult<()> {
        match r {
            Ok(_) => Ok(()),
            Err(Throw::Value(e)) => {
                self.report_exception(&e);
                Ok(())
            }
            Err(t) => Err(t),
        }
    }

    /// HTML "report an exception" (§8.1.4.6) a través de los hooks.
    pub fn report_exception(&mut self, e: &Value) {
        let err = crate::host::runtime::JsError::from_value(self, e.clone());
        self.host.hooks.report_exception(&err.message, &err.stack);
    }

    /// "Perform a microtask checkpoint" (§8.1.7.3): vacía la cola de jobs, notifica los
    /// rechazos sin manejar y reporta las excepciones de los jobs.
    pub fn perform_microtask_checkpoint(&mut self) -> JsResult<()> {
        self.drain_jobs()?;
        self.notify_rejections();
        let errors = std::mem::take(&mut self.job_errors);
        for e in &errors {
            self.report_exception(e);
        }
        Ok(())
    }

    // --- Temporizadores (§8.6) ---

    /// Los "timer initialization steps": programa `handler` dentro de `timeout` ms.
    pub fn set_timer(
        &mut self,
        realm: RealmId,
        handler: TimerHandler,
        timeout: f64,
        args: Vec<Value>,
        repeat: bool,
    ) -> TimerId {
        let el = &mut self.host.event_loop;
        el.next_timer += 1;
        let id = el.next_timer;
        let nesting = el.current_nesting.unwrap_or(0);
        let mut timeout = if timeout.is_finite() {
            timeout.max(0.0)
        } else {
            0.0
        };
        if nesting > 5 && timeout < 4.0 {
            timeout = 4.0;
        }
        let now = self.host.hooks.now();
        let el = &mut self.host.event_loop;
        let seq = el.next_seq();
        el.timers.insert(
            id,
            Timer {
                realm,
                handler,
                args,
                when: now + timeout,
                timeout,
                repeat,
                seq,
                nesting: nesting + 1,
                queued: false,
            },
        );
        self.host.hooks.timer_scheduled(now + timeout);
        id
    }

    /// `clearTimeout`/`clearInterval`: borra el temporizador del mapa de activos.
    pub fn clear_timer(&mut self, id: TimerId) {
        self.host.event_loop.timers.remove(&id);
    }

    fn run_timer(&mut self, id: TimerId) -> JsResult<()> {
        let Some(t) = self.host.event_loop.timers.get(&id) else {
            return Ok(());
        };
        let (realm, handler, args, nesting) =
            (t.realm, t.handler.clone(), t.args.clone(), t.nesting);
        let saved_nesting = self.host.event_loop.current_nesting.replace(nesting);
        let saved_realm = self.current_realm;
        self.current_realm = realm;
        let r = match &handler {
            TimerHandler::Callback(f) => {
                let global = Value::Object(self.realms[realm].global_object);
                self.call_rooted(f, global, &args)
            }
            TimerHandler::Code(src) => {
                let text = src.to_string_lossy();
                match self.compile_script_text(&text, "<timer>") {
                    Ok(code) => self.script_evaluation(code),
                    Err(e) => Err(e),
                }
            }
        };
        self.current_realm = saved_realm;
        let result = self.report_result(r);
        // Repetir (setInterval) o retirar del mapa de temporizadores activos.
        let now = self.host.hooks.now();
        let mut reschedule = None;
        if let Some(t) = self.host.event_loop.timers.get_mut(&id) {
            if t.repeat {
                let mut timeout = t.timeout;
                if nesting > 5 && timeout < 4.0 {
                    timeout = 4.0;
                }
                t.when = now + timeout;
                t.nesting = nesting + 1;
                t.queued = false;
                reschedule = Some(t.when);
            } else {
                self.host.event_loop.timers.remove(&id);
            }
        }
        if let Some(seq_when) = reschedule {
            let seq = self.host.event_loop.next_seq();
            if let Some(t) = self.host.event_loop.timers.get_mut(&id) {
                t.seq = seq;
            }
            self.host.hooks.timer_scheduled(seq_when);
        }
        self.host.event_loop.current_nesting = saved_nesting;
        result
    }

    // --- Animation frames (§8.11) ---

    /// `requestAnimationFrame`: registra `callback` para el próximo frame.
    pub fn request_animation_frame(&mut self, callback: Value) -> u32 {
        let el = &mut self.host.event_loop;
        el.next_animation += 1;
        let id = el.next_animation;
        el.animation.push((id, callback));
        self.host.hooks.animation_frame_requested();
        id
    }

    /// `cancelAnimationFrame`.
    pub fn cancel_animation_frame(&mut self, id: u32) {
        self.host.event_loop.animation.retain(|(i, _)| *i != id);
    }

    /// ¿Hay callbacks de animation frame pendientes?
    pub fn has_animation_frame_callbacks(&self) -> bool {
        !self.host.event_loop.animation.is_empty()
    }

    /// "Run the animation frame callbacks" (§8.11.1) con `timestamp`: los registrados hasta
    /// ahora, cada uno seguido de un punto de control de microtasks.
    pub fn run_animation_frames(&mut self, timestamp: f64) -> JsResult<()> {
        let callbacks = std::mem::take(&mut self.host.event_loop.animation);
        for (_, cb) in callbacks {
            let r = self.call_rooted(&cb, Value::Undefined, &[Value::Number(timestamp)]);
            self.report_result(r)?;
            self.perform_microtask_checkpoint()?;
        }
        Ok(())
    }
}

// --- Globales ---

/// Instala en el objeto global de `realm` los temporizadores, `queueMicrotask`,
/// `requestAnimationFrame` y `console`.
pub fn install_globals(agent: &mut Agent, realm: RealmId) {
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let g = agent.realms[realm].global_object;
    method(agent, g, "setTimeout", 1, set_timeout);
    method(agent, g, "setInterval", 1, set_interval);
    method(agent, g, "clearTimeout", 0, clear_timeout);
    method(agent, g, "clearInterval", 0, clear_timeout);
    method(agent, g, "queueMicrotask", 1, queue_microtask);
    method(
        agent,
        g,
        "requestAnimationFrame",
        1,
        request_animation_frame,
    );
    method(agent, g, "cancelAnimationFrame", 1, cancel_animation_frame);
    let console = agent.new_plain_object();
    let levels: [(&str, f64); 8] = [
        ("log", 0.0),
        ("info", 1.0),
        ("warn", 2.0),
        ("error", 3.0),
        ("debug", 4.0),
        ("trace", 4.0),
        ("dir", 0.0),
        ("table", 0.0),
    ];
    for (name, level) in levels {
        let f = agent.create_builtin_with_slots(console_log, 0, name, vec![Value::Number(level)]);
        agent.put_data(
            console,
            PropertyKey::from(name),
            Value::Object(f),
            true,
            true,
            true,
        );
    }
    method(agent, console, "assert", 0, console_assert);
    let tag = PropertyKey::Symbol(agent.well_known.to_string_tag);
    agent.put_data(console, tag, Value::str("console"), false, false, true);
    agent.put_data(
        g,
        PropertyKey::from("console"),
        Value::Object(console),
        true,
        false,
        true,
    );
    agent.current_realm = saved;
}

/// `setTimeout`/`setInterval` (§8.6): handler (función o string), `long timeout = 0`, args.
fn timer(agent: &mut Agent, a: &CallArgs<'_>, repeat: bool) -> JsResult<Value> {
    let h = a.arg(0);
    let handler = if agent.is_callable(&h) {
        TimerHandler::Callback(h)
    } else {
        TimerHandler::Code(agent.to_string(&h)?)
    };
    let timeout = f64::from(crate::host::webidl::to_long(agent, &a.arg(1))?);
    let args = a.args.get(2..).unwrap_or(&[]).to_vec();
    let realm = agent.current_realm;
    let id = agent.set_timer(realm, handler, timeout, args, repeat);
    Ok(Value::Number(f64::from(id)))
}

fn set_timeout(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    timer(agent, a, false)
}

fn set_interval(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    timer(agent, a, true)
}

/// `clearTimeout`/`clearInterval` (`long id = 0`).
fn clear_timeout(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = crate::host::webidl::to_long(agent, &a.arg(0))?;
    if id > 0 {
        agent.clear_timer(id as TimerId);
    }
    Ok(Value::Undefined)
}

/// `queueMicrotask(callback)` (§8.7): el callback como microtask; sus excepciones se
/// reportan.
fn queue_microtask(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let cb = a.arg(0);
    if !agent.is_callable(&cb) {
        return agent.type_error(
            "Failed to execute 'queueMicrotask': parameter 1 is not of type 'Function'",
        );
    }
    let realm = agent.current_realm;
    agent.enqueue_job(Job::Microtask {
        callback: cb,
        realm,
    });
    Ok(Value::Undefined)
}

fn request_animation_frame(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let cb = a.arg(0);
    if !agent.is_callable(&cb) {
        return agent.type_error(
            "Failed to execute 'requestAnimationFrame': parameter 1 is not of type 'Function'",
        );
    }
    Ok(Value::Number(f64::from(agent.request_animation_frame(cb))))
}

fn cancel_animation_frame(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let id = crate::host::webidl::to_unsigned_long(agent, &a.arg(0))?;
    agent.cancel_animation_frame(id);
    Ok(Value::Undefined)
}

/// Formatter (Console Standard §2.2.1): `%s %d %i %f %o %O %c %%` en el primer argumento.
fn format_args(agent: &mut Agent, args: &[Value]) -> JsResult<String> {
    let mut out = String::new();
    let mut rest = args;
    if let Some(Value::String(fmt)) = args.first() {
        rest = &args[1..];
        let s = fmt.to_string_lossy();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            match chars.peek().copied() {
                Some('%') => {
                    chars.next();
                    out.push('%');
                }
                Some(spec @ ('s' | 'd' | 'i' | 'f' | 'o' | 'O' | 'c')) if !rest.is_empty() => {
                    chars.next();
                    let v = rest[0].clone();
                    rest = &rest[1..];
                    match spec {
                        's' => out.push_str(&match &v {
                            Value::String(s) => s.to_string_lossy(),
                            Value::Symbol(_) | Value::Object(_) => {
                                crate::host::runtime::display(agent, &v)
                            }
                            other => agent.to_string(other)?.to_string_lossy(),
                        }),
                        'd' | 'i' => {
                            let n = match &v {
                                Value::Symbol(_) => f64::NAN,
                                other => agent.to_number(other).unwrap_or(f64::NAN),
                            };
                            let n = if n.is_finite() { n.trunc() } else { n };
                            out.push_str(&crate::abstract_ops::number::number_to_std_string(n));
                        }
                        'f' => {
                            let n = match &v {
                                Value::Symbol(_) => f64::NAN,
                                other => agent.to_number(other).unwrap_or(f64::NAN),
                            };
                            out.push_str(&crate::abstract_ops::number::number_to_std_string(n));
                        }
                        'c' => {}
                        _ => out.push_str(&crate::host::runtime::display(agent, &v)),
                    }
                }
                _ => out.push('%'),
            }
        }
    }
    for v in rest {
        if !out.is_empty() {
            out.push(' ');
        }
        match v {
            Value::String(s) => out.push_str(&s.to_string_lossy()),
            other => out.push_str(&crate::host::runtime::display(agent, other)),
        }
    }
    Ok(out)
}

fn console_level(n: f64) -> ConsoleLevel {
    match n as u32 {
        1 => ConsoleLevel::Info,
        2 => ConsoleLevel::Warn,
        3 => ConsoleLevel::Error,
        4 => ConsoleLevel::Debug,
        _ => ConsoleLevel::Log,
    }
}

/// `console.log`, `info`, `warn`, `error`, `debug`… (Console Standard §1.1).
fn console_log(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let level = console_level(slots(agent, a.callee)[0].as_number().unwrap_or(0.0));
    let msg = format_args(agent, a.args)?;
    agent.host.hooks.console(level, &msg);
    Ok(Value::Undefined)
}

/// `console.assert(condition, ...data)` (Console Standard §1.1.1).
fn console_assert(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    if crate::abstract_ops::conversion::to_boolean(agent, &a.arg(0)) {
        return Ok(Value::Undefined);
    }
    let data = a.args.get(1..).unwrap_or(&[]);
    let mut msg = "Assertion failed".to_owned();
    if !data.is_empty() {
        msg.push_str(": ");
        msg.push_str(&format_args(agent, data)?);
    }
    agent.host.hooks.console(ConsoleLevel::Error, &msg);
    Ok(Value::Undefined)
}
