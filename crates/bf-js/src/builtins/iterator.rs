//! %IteratorPrototype% (§27.1.2), %ArrayIteratorPrototype% (§23.1.5),
//! %StringIteratorPrototype% (§22.1.5) y %ForInIteratorPrototype% (§14.7.5.10).

use crate::builtins::{method, method_sym, to_string_tag};
use crate::heap::Gc;
use crate::object::{ArrayIteratorData, IterKind, Object, ObjectKind};
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let ip = agent.new_object(Some(op));
    let it = agent.well_known.iterator;
    method_sym(agent, ip, it, "[Symbol.iterator]", 0, iterator_self);
    let ap = agent.new_object(Some(ip));
    method(agent, ap, "next", 0, array_iterator_next);
    to_string_tag(agent, ap, "Array Iterator");
    let sp = agent.new_object(Some(ip));
    method(agent, sp, "next", 0, string_iterator_next);
    to_string_tag(agent, sp, "String Iterator");
    let fp = agent.new_object(Some(ip));
    method(agent, fp, "next", 0, for_in_iterator_next);
    let i = &mut agent.realms[realm].intrinsics;
    i.iterator_prototype = ip;
    i.array_iterator_prototype = ap;
    i.string_iterator_prototype = sp;
    i.for_in_iterator_prototype = fp;
}

/// %IteratorPrototype% [ @@iterator ] () (§27.1.2.1).
fn iterator_self(_: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(a.this.clone())
}

/// CreateArrayIterator (§23.1.5.1).
pub fn create_array_iterator(agent: &mut Agent, o: Gc<Object>, kind: IterKind) -> Value {
    let proto = agent.intrinsics().array_iterator_prototype;
    Value::Object(agent.alloc(Object::new(
        Some(proto),
        ObjectKind::ArrayIterator(Box::new(ArrayIteratorData {
            object: Some(o),
            next_index: 0.0,
            kind,
        })),
    )))
}

/// %ArrayIteratorPrototype%.next (§23.1.5.2.1).
fn array_iterator_next(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Value::Object(it) = a.this else {
        return agent.type_error("next method called on incompatible receiver");
    };
    let (object, index, kind) = match &agent.heap[it].kind {
        ObjectKind::ArrayIterator(d) => (d.object, d.next_index, d.kind),
        _ => return agent.type_error("next method called on incompatible receiver"),
    };
    let Some(o) = object else {
        return agent.create_iter_result(Value::Undefined, true);
    };
    let len = match crate::builtins::typed_array::iteration_length(agent, o) {
        Some(len) => len?, // E3: TypedArray (§23.1.5.1 paso 2.b)
        None => agent.length_of_array_like(o)?,
    };
    if index >= len {
        if let ObjectKind::ArrayIterator(d) = &mut agent.heap[it].kind {
            d.object = None;
        }
        return agent.create_iter_result(Value::Undefined, true);
    }
    if let ObjectKind::ArrayIterator(d) = &mut agent.heap[it].kind {
        d.next_index = index + 1.0;
    }
    let key = PropertyKey::from_number(index);
    let result = match kind {
        IterKind::Keys => Value::Number(index),
        IterKind::Values => agent.get(o, &key, Value::Object(o))?,
        IterKind::Entries => {
            let v = agent.get(o, &key, Value::Object(o))?;
            Value::Object(agent.create_array_from_list(&[Value::Number(index), v])?)
        }
    };
    agent.create_iter_result(result, false)
}

/// CreateStringIterator (§22.1.3.36 paso 3).
pub fn create_string_iterator(agent: &mut Agent, s: JsString) -> Value {
    let proto = agent.intrinsics().string_iterator_prototype;
    Value::Object(agent.alloc(Object::new(
        Some(proto),
        ObjectKind::StringIterator {
            string: Some(s),
            position: 0,
        },
    )))
}

/// %StringIteratorPrototype%.next (§22.1.5.1.1): code point a code point.
fn string_iterator_next(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Value::Object(it) = a.this else {
        return agent.type_error("next method called on incompatible receiver");
    };
    let (s, pos) = match &agent.heap[it].kind {
        ObjectKind::StringIterator { string, position } => (string.clone(), *position),
        _ => return agent.type_error("next method called on incompatible receiver"),
    };
    let Some(s) = s else {
        return agent.create_iter_result(Value::Undefined, true);
    };
    let u = s.units();
    if pos >= u.len() {
        if let ObjectKind::StringIterator { string, .. } = &mut agent.heap[it].kind {
            *string = None;
        }
        return agent.create_iter_result(Value::Undefined, true);
    }
    let n = if (0xD800..0xDC00).contains(&u[pos])
        && pos + 1 < u.len()
        && (0xDC00..0xE000).contains(&u[pos + 1])
    {
        2
    } else {
        1
    };
    let cp = JsString::from_units(u[pos..pos + n].to_vec());
    if let ObjectKind::StringIterator { position, .. } = &mut agent.heap[it].kind {
        *position = pos + n;
    }
    agent.create_iter_result(Value::String(cp), false)
}

fn for_in_iterator_next(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Value::Object(it) = a.this else {
        return agent.type_error("next method called on incompatible receiver");
    };
    match agent.for_in_next(it)? {
        Some(k) => agent.create_iter_result(k, false),
        None => agent.create_iter_result(Value::Undefined, true),
    }
}
