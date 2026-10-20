//! Generadores (§27.5): el frame suspendido vive en el objeto generador.

use crate::heap::{Gc, Trace, Tracer};
use crate::object::{Object, ObjectKind};
use crate::value::Value;
use crate::vm::frame::Frame;
use crate::vm::function::resume_frame;
use crate::vm::interp::{FrameExit, Resume};
use crate::vm::{Agent, JsResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenState {
    SuspendedStart,
    SuspendedYield,
    Executing,
    Completed,
}

#[derive(Debug)]
pub struct GeneratorData {
    pub state: GenState,
    pub frame: Option<Box<Frame>>,
}

impl Trace for GeneratorData {
    fn trace(&self, t: &mut Tracer) {
        if let Some(f) = &self.frame {
            f.trace(t);
        }
    }
}

/// Guarda el frame recién suspendido en GeneratorStart.
pub(crate) fn store_start_frame(agent: &mut Agent, g: Gc<Object>, frame: Frame) {
    match &mut agent.heap[g].kind {
        ObjectKind::Generator(data) => {
            data.frame = Some(Box::new(frame));
            data.state = GenState::SuspendedStart;
        }
        // E3: AsyncGeneratorStart (§27.6.3.5).
        ObjectKind::AsyncGenerator(data) => {
            data.frame = Some(Box::new(frame));
            data.state = crate::builtins::r#async::AsyncGenState::SuspendedStart;
        }
        _ => {}
    }
}

/// Crea el objeto generador (OrdinaryCreateFromConstructor(functionObject,
/// "%GeneratorPrototype%"), §15.5.2 EvaluateGeneratorBody).
pub(crate) fn create_generator(agent: &mut Agent, f: Gc<Object>) -> JsResult<Gc<Object>> {
    // E3: los generadores async (§15.6.2 EvaluateAsyncGeneratorBody).
    if matches!(&agent.heap[f].kind, ObjectKind::Function(c) if c.code.is_async) {
        return crate::builtins::r#async::create_async_generator(agent, f);
    }
    agent.ordinary_create_from_constructor(
        f,
        |i| i.generator_prototype,
        ObjectKind::Generator(Box::new(GeneratorData {
            state: GenState::Executing,
            frame: None,
        })),
    )
}

/// GeneratorValidate (§27.5.3.2): el estado, o TypeError.
fn validate(agent: &mut Agent, g: &Value) -> JsResult<(Gc<Object>, GenState)> {
    if let Value::Object(o) = g
        && let ObjectKind::Generator(d) = &agent.heap[*o].kind
    {
        if d.state == GenState::Executing {
            return agent.type_error("Generator is already running");
        }
        return Ok((*o, d.state));
    }
    agent.type_error("not a generator object")
}

/// GeneratorResume / GeneratorResumeAbrupt (§27.5.3.3, §27.5.3.4).
pub fn generator_resume(agent: &mut Agent, g: &Value, mode: Resume) -> JsResult<Value> {
    let (o, state) = validate(agent, g)?;
    if state == GenState::SuspendedStart {
        match &mode {
            Resume::Return(_) | Resume::Throw(_) => set_state(agent, o, GenState::Completed),
            _ => {}
        }
    }
    let state = match &agent.heap[o].kind {
        ObjectKind::Generator(d) => d.state,
        _ => unreachable!("generador"),
    };
    if state == GenState::Completed {
        return match mode {
            Resume::Normal | Resume::Next(_) => agent.create_iter_result(Value::Undefined, true),
            Resume::Return(v) => agent.create_iter_result(v, true),
            Resume::Throw(e) => Err(crate::vm::Throw::Value(e)),
        };
    }
    let frame = match &mut agent.heap[o].kind {
        ObjectKind::Generator(d) => {
            d.state = GenState::Executing;
            d.frame.take()
        }
        _ => None,
    };
    let Some(frame) = frame else {
        return agent.type_error("Generator is already running");
    };
    let (r, frame) = resume_frame(agent, *frame, mode);
    match r {
        Ok(FrameExit::Yield(v, raw)) => {
            if let ObjectKind::Generator(d) = &mut agent.heap[o].kind {
                d.state = GenState::SuspendedYield;
                d.frame = Some(Box::new(frame));
            }
            if raw {
                Ok(v)
            } else {
                agent.create_iter_result(v, false)
            }
        }
        Ok(FrameExit::Return(v)) => {
            set_state(agent, o, GenState::Completed);
            agent.create_iter_result(v, true)
        }
        Ok(FrameExit::GeneratorStart(_)) => unreachable!("GeneratorStart al reanudar"),
        Ok(FrameExit::Await(_)) => unreachable!("await en un generador"),
        Err(e) => {
            set_state(agent, o, GenState::Completed);
            Err(e)
        }
    }
}

fn set_state(agent: &mut Agent, o: Gc<Object>, s: GenState) {
    if let ObjectKind::Generator(d) = &mut agent.heap[o].kind {
        d.state = s;
        if s == GenState::Completed {
            d.frame = None;
        }
    }
}
