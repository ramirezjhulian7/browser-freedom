//! `MutationObserver` y `MutationRecord` (DOM §4.3) sobre los registros de `bf-dom`. La
//! entrega es un microtask ("queue a mutation observer microtask", §4.3): después de cada
//! mutación hecha desde script, si `bf-dom` encoló records, se agenda un microtask que llama
//! a los callbacks.

use std::cell::RefCell;
use std::collections::HashMap;

use bf_dom::observer::{MutationRecord, ObserverId, ObserverInit};
use bf_js::builtins::promise::Job;
use bf_js::host::HostClass;
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::function::CallArgs;
use bf_js::vm::{Agent, JsResult, Throw};

use crate::dom::state::state;
use crate::dom::{arg_node, classes, from_opt_dom_string, throw_dom, to_dom_string, wrap_opt};
use crate::{DocIndex, WebContextHandle, with_doc_mut};

/// El payload de un `MutationObserver`: su id en cada documento observado.
#[derive(Debug, Default)]
pub struct ObserverData {
    pub ids: RefCell<HashMap<DocIndex, ObserverId>>,
}

/// El payload de un `MutationRecord`.
#[derive(Debug, Clone)]
pub struct RecordData {
    pub doc: DocIndex,
    pub record: MutationRecord,
}

pub fn mutation_observer_class() -> HostClass {
    HostClass::new("MutationObserver")
        .constructor(ctor, 1)
        .method("observe", 1, observe)
        .method("disconnect", 0, disconnect)
        .method("takeRecords", 0, take_records)
}

pub fn mutation_record_class() -> HostClass {
    HostClass::new("MutationRecord")
        .attribute("type", Some(rec_type), None)
        .attribute("target", Some(rec_target), None)
        .attribute("addedNodes", Some(rec_added), None)
        .attribute("removedNodes", Some(rec_removed), None)
        .attribute("previousSibling", Some(rec_previous), None)
        .attribute("nextSibling", Some(rec_next), None)
        .attribute("attributeName", Some(rec_attribute_name), None)
        .attribute("attributeNamespace", Some(rec_attribute_namespace), None)
        .attribute("oldValue", Some(rec_old_value), None)
}

/// `new MutationObserver(callback)`.
fn ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let cb = a.arg(0);
    if !agent.is_callable(&cb) {
        return agent.type_error(
            "Failed to construct 'MutationObserver': parameter 1 is not of type 'Function'.",
        );
    }
    let class = classes(agent).mutation_observer;
    let nt = a.new_target.expect("new.target");
    let o = agent.host_create_from_new_target(nt, class, None, Box::<ObserverData>::default())?;
    if let Some(s) = agent.host_slots_mut(o) {
        s.push(cb);
    }
    Ok(Value::Object(o))
}

fn this_observer(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<()> {
    if agent.host_unwrap::<ObserverData>(&a.this).is_none() {
        return agent.type_error("Illegal invocation");
    }
    Ok(())
}

/// El id del observador en `doc` (lo crea y registra el callback la primera vez).
fn observer_id(agent: &mut Agent, this: &Value, doc: DocIndex) -> ObserverId {
    if let Some(id) = agent
        .host_unwrap::<ObserverData>(this)
        .and_then(|d| d.ids.borrow().get(&doc).copied())
    {
        return id;
    }
    let id = with_doc_mut(agent, doc, |d| d.create_mutation_observer());
    if let Some(d) = agent.host_unwrap::<ObserverData>(this) {
        d.ids.borrow_mut().insert(doc, id);
    }
    let cb = match this {
        Value::Object(o) => agent
            .host_slots_mut(*o)
            .and_then(|s| s.first().cloned())
            .unwrap_or_default(),
        _ => Value::Undefined,
    };
    let cbh = agent.host_pin(cb);
    let oh = agent.host_pin(this.clone());
    state(agent).observers.insert((doc, id.0), (cbh, oh));
    id
}

/// `observe(target, options)` (§4.3.1).
fn observe(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_observer(agent, a)?;
    let target = arg_node(agent, &a.arg(0), "parameter 1")?;
    let opts = a.arg(1);
    let get = |agent: &mut Agent, name: &str| -> JsResult<Value> {
        match &opts {
            Value::Object(o) => agent.get(*o, &PropertyKey::from(name), opts.clone()),
            Value::Undefined | Value::Null => Ok(Value::Undefined),
            _ => agent.type_error("The provided value is not of type 'MutationObserverInit'"),
        }
    };
    let opt_bool = |agent: &mut Agent, v: Value| -> Option<bool> {
        (!v.is_undefined()).then(|| bf_js::host::webidl::to_boolean(agent, &v))
    };
    // Los miembros en orden lexicográfico (WebIDL §3.2.17).
    let attribute_filter = get(agent, "attributeFilter")?;
    let attribute_filter = if attribute_filter.is_undefined() {
        None
    } else {
        Some(bf_js::host::webidl::to_sequence(
            agent,
            &attribute_filter,
            to_dom_string,
        )?)
    };
    let v = get(agent, "attributeOldValue")?;
    let attribute_old_value = opt_bool(agent, v);
    let v = get(agent, "attributes")?;
    let attributes = opt_bool(agent, v);
    let v = get(agent, "characterData")?;
    let character_data = opt_bool(agent, v);
    let v = get(agent, "characterDataOldValue")?;
    let character_data_old_value = opt_bool(agent, v);
    let v = get(agent, "childList")?;
    let child_list = bf_js::host::webidl::to_boolean(agent, &v);
    let v = get(agent, "subtree")?;
    let subtree = bf_js::host::webidl::to_boolean(agent, &v);
    let init = ObserverInit {
        child_list,
        attributes,
        character_data,
        subtree,
        attribute_old_value,
        character_data_old_value,
        attribute_filter,
    };
    let id = observer_id(agent, &a.this, target.doc);
    let r = with_doc_mut(agent, target.doc, |d| d.observe(id, target.node, init));
    match r {
        Ok(()) => Ok(Value::Undefined),
        Err(e) => throw_dom(agent, e),
    }
}

/// `disconnect()`.
fn disconnect(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_observer(agent, a)?;
    let ids: Vec<(DocIndex, ObserverId)> = agent
        .host_unwrap::<ObserverData>(&a.this)
        .map(|d| d.ids.borrow().iter().map(|(k, v)| (*k, *v)).collect())
        .unwrap_or_default();
    for (doc, id) in ids {
        with_doc_mut(agent, doc, |d| d.disconnect_observer(id));
    }
    Ok(Value::Undefined)
}

/// `takeRecords()`.
fn take_records(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    this_observer(agent, a)?;
    let ids: Vec<(DocIndex, ObserverId)> = agent
        .host_unwrap::<ObserverData>(&a.this)
        .map(|d| d.ids.borrow().iter().map(|(k, v)| (*k, *v)).collect())
        .unwrap_or_default();
    let mut values = Vec::new();
    for (doc, id) in ids {
        let records = with_doc_mut(agent, doc, |d| d.take_records(id));
        for r in records {
            values.push(record_object(agent, doc, r));
        }
    }
    Ok(Value::Object(agent.create_array_from_list(&values)?))
}

fn record_object(agent: &mut Agent, doc: DocIndex, record: MutationRecord) -> Value {
    let class = classes(agent).mutation_record;
    Value::Object(agent.host_create(class, Box::new(RecordData { doc, record })))
}

/// Después de una mutación desde script: agenda la entrega si `bf-dom` encoló records.
pub fn schedule_delivery(agent: &mut Agent, doc: DocIndex) {
    let ctx = WebContextHandle::current(agent);
    let queued = ctx.get(agent).doc(doc).mutation_observer_microtask_queued();
    if !queued || state(agent).observer_microtask {
        return;
    }
    state(agent).observer_microtask = true;
    let f = agent.create_builtin_function(deliver, 0, "", false);
    let realm = ctx.realm;
    agent.enqueue_job(Job::Microtask {
        callback: Value::Object(f),
        realm,
    });
}

/// El microtask: "notify mutation observers" (§4.3) de todos los documentos.
fn deliver(agent: &mut Agent, _: &CallArgs<'_>) -> JsResult<Value> {
    state(agent).observer_microtask = false;
    let ctx = WebContextHandle::current(agent);
    let n = ctx.get(agent).documents.len();
    for doc in 0..n as DocIndex {
        let notes = with_doc_mut(agent, doc, |d| d.notify_mutation_observers());
        for (id, records) in notes {
            if records.is_empty() {
                continue;
            }
            let Some(&(cbh, oh)) = state(agent).observers.get(&(doc, id.0)) else {
                continue;
            };
            let cb = agent.host_pinned(cbh).unwrap_or_default();
            let obs = agent.host_pinned(oh).unwrap_or_default();
            let values: Vec<Value> = records
                .into_iter()
                .map(|r| record_object(agent, doc, r))
                .collect();
            let arr = Value::Object(agent.create_array_from_list(&values)?);
            match agent.call(&cb, obs.clone(), &[arr, obs]) {
                Ok(_) => {}
                Err(Throw::Value(e)) => crate::dom::events::report_exception(agent, &e),
                Err(t) => return Err(t),
            }
        }
    }
    Ok(Value::Undefined)
}

// --- MutationRecord ---

fn rec(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<RecordData> {
    match agent.host_unwrap::<RecordData>(&a.this) {
        Some(r) => Ok(r.clone()),
        None => agent.type_error("Illegal invocation"),
    }
}

fn rec_type(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::str(rec(agent, a)?.record.type_.as_str()))
}

fn rec_target(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = rec(agent, a)?;
    Ok(wrap_opt(agent, r.doc, Some(r.record.target)))
}

/// `addedNodes`/`removedNodes`: la misma `NodeList` en cada lectura (en los slots).
fn rec_list(agent: &mut Agent, a: &CallArgs<'_>, i: usize) -> JsResult<Value> {
    let r = rec(agent, a)?;
    let Value::Object(o) = a.this else {
        return agent.type_error("Illegal invocation");
    };
    if let Some(v) = agent.host_slots_mut(o).and_then(|s| s.get(i).cloned())
        && !v.is_undefined()
    {
        return Ok(v);
    }
    let nodes = if i == 0 {
        r.record.added_nodes
    } else {
        r.record.removed_nodes
    };
    let list = crate::dom::collections::static_node_list(agent, r.doc, nodes);
    if let Some(s) = agent.host_slots_mut(o) {
        while s.len() <= i {
            s.push(Value::Undefined);
        }
        s[i] = list.clone();
    }
    Ok(list)
}

fn rec_added(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    rec_list(agent, a, 0)
}

fn rec_removed(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    rec_list(agent, a, 1)
}

fn rec_previous(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = rec(agent, a)?;
    Ok(wrap_opt(agent, r.doc, r.record.previous_sibling))
}

fn rec_next(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = rec(agent, a)?;
    Ok(wrap_opt(agent, r.doc, r.record.next_sibling))
}

fn rec_attribute_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = rec(agent, a)?;
    Ok(from_opt_dom_string(r.record.attribute_name.as_ref()))
}

fn rec_attribute_namespace(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = rec(agent, a)?;
    Ok(match r.record.attribute_namespace {
        Some(ns) => Value::str(ns.uri()),
        None => Value::Null,
    })
}

fn rec_old_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = rec(agent, a)?;
    Ok(from_opt_dom_string(r.record.old_value.as_ref()))
}
