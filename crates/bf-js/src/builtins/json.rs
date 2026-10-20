//! JSON (§25.5): `parse` con la gramática exacta de ECMA-404 y `reviver` (con el contexto
//! `source` de la propuesta JSON.parse source text access para los valores primitivos),
//! `stringify` con `replacer`, `space`, `toJSON`, detección de ciclos y surrogates sueltos
//! escapados, y `rawJSON`/`isRawJSON`.

use crate::builtins::{FnEntry, methods, to_string_tag};
use crate::heap::Gc;
use crate::object::{IterKind, Object, ObjectKind};
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    let j = agent.new_object(Some(op));
    const FNS: &[FnEntry] = &[
        ("isRawJSON", 1, is_raw_json),
        ("parse", 2, parse),
        ("rawJSON", 1, raw_json),
        ("stringify", 3, stringify),
    ];
    methods(agent, j, FNS);
    to_string_tag(agent, j, "JSON");
    crate::builtins::register_global(agent, realm, "JSON", j);
}

// --- Parser ---

/// Un valor parseado con su texto fuente (para el contexto del reviver).
enum Node {
    Primitive { value: Value, source: JsString },
    Array(Vec<Node>),
    Object(Vec<(JsString, Node)>),
}

struct Parser<'a> {
    s: &'a [u16],
    pos: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while let Some(&c) = self.s.get(self.pos) {
            if c == 0x20 || c == 0x09 || c == 0x0A || c == 0x0D {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<u16> {
        self.s.get(self.pos).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(u16::from(c)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn literal(&mut self, word: &str) -> bool {
        let w: Vec<u16> = word.encode_utf16().collect();
        if self.s[self.pos..].starts_with(&w) {
            self.pos += w.len();
            true
        } else {
            false
        }
    }

    fn string(&mut self) -> Option<JsString> {
        if !self.eat(b'"') {
            return None;
        }
        let mut out = Vec::new();
        loop {
            let c = self.peek()?;
            self.pos += 1;
            match c {
                0x22 => return Some(JsString::from_units(out)),
                0x5C => {
                    let e = self.peek()?;
                    self.pos += 1;
                    let v = match e {
                        0x22 => 0x22,
                        0x5C => 0x5C,
                        0x2F => 0x2F,
                        0x62 => 0x08,
                        0x66 => 0x0C,
                        0x6E => 0x0A,
                        0x72 => 0x0D,
                        0x74 => 0x09,
                        0x75 => {
                            let mut v: u16 = 0;
                            for _ in 0..4 {
                                let d = char::from_u32(u32::from(self.peek()?))?.to_digit(16)?;
                                self.pos += 1;
                                v = v * 16 + d as u16;
                            }
                            v
                        }
                        _ => return None,
                    };
                    out.push(v);
                }
                c if c < 0x20 => return None,
                c => out.push(c),
            }
        }
    }

    fn number(&mut self) -> Option<f64> {
        let start = self.pos;
        let digit = |c: Option<u16>| c.is_some_and(|c| (0x30..=0x39).contains(&c));
        self.eat(b'-');
        if self.eat(b'0') {
        } else if digit(self.peek()) {
            while digit(self.peek()) {
                self.pos += 1;
            }
        } else {
            return None;
        }
        if self.eat(b'.') {
            if !digit(self.peek()) {
                return None;
            }
            while digit(self.peek()) {
                self.pos += 1;
            }
        }
        if self.peek() == Some(u16::from(b'e')) || self.peek() == Some(u16::from(b'E')) {
            self.pos += 1;
            if !self.eat(b'+') {
                self.eat(b'-');
            }
            if !digit(self.peek()) {
                return None;
            }
            while digit(self.peek()) {
                self.pos += 1;
            }
        }
        let text = String::from_utf16(&self.s[start..self.pos]).ok()?;
        text.parse().ok()
    }

    fn value(&mut self, agent: &mut Agent) -> JsResult<Option<Node>> {
        if agent.stack_exhausted() {
            return agent.range_error("Maximum call stack size exceeded");
        }
        self.ws();
        let start = self.pos;
        let prim = |p: &Parser<'_>, value: Value| Node::Primitive {
            value,
            source: JsString::from_units(p.s[start..p.pos].to_vec()),
        };
        let Some(c) = self.peek() else {
            return Ok(None);
        };
        let node = match c {
            0x7B => {
                self.pos += 1;
                let mut members = Vec::new();
                self.ws();
                if !self.eat(b'}') {
                    loop {
                        self.ws();
                        let Some(k) = self.string() else {
                            return Ok(None);
                        };
                        self.ws();
                        if !self.eat(b':') {
                            return Ok(None);
                        }
                        let Some(v) = self.value(agent)? else {
                            return Ok(None);
                        };
                        members.push((k, v));
                        self.ws();
                        if self.eat(b',') {
                            continue;
                        }
                        if self.eat(b'}') {
                            break;
                        }
                        return Ok(None);
                    }
                }
                Node::Object(members)
            }
            0x5B => {
                self.pos += 1;
                let mut elems = Vec::new();
                self.ws();
                if !self.eat(b']') {
                    loop {
                        let Some(v) = self.value(agent)? else {
                            return Ok(None);
                        };
                        elems.push(v);
                        self.ws();
                        if self.eat(b',') {
                            continue;
                        }
                        if self.eat(b']') {
                            break;
                        }
                        return Ok(None);
                    }
                }
                Node::Array(elems)
            }
            0x22 => match self.string() {
                Some(s) => prim(self, Value::String(s)),
                None => return Ok(None),
            },
            0x74 if self.literal("true") => prim(self, Value::Boolean(true)),
            0x66 if self.literal("false") => prim(self, Value::Boolean(false)),
            0x6E if self.literal("null") => prim(self, Value::Null),
            _ => match self.number() {
                Some(n) => prim(self, Value::Number(n)),
                None => return Ok(None),
            },
        };
        Ok(Some(node))
    }
}

/// Parsea el texto completo como JSON (con espacios alrededor).
fn parse_text(agent: &mut Agent, s: &[u16]) -> JsResult<Node> {
    let mut p = Parser { s, pos: 0 };
    let v = p.value(agent)?;
    p.ws();
    match v {
        Some(node) if p.pos == s.len() => Ok(node),
        _ => agent.syntax_error("Unexpected token in JSON"),
    }
}

/// Construye los valores ECMAScript del árbol.
fn materialize(agent: &mut Agent, n: &Node) -> JsResult<Value> {
    match n {
        Node::Primitive { value, .. } => Ok(value.clone()),
        Node::Array(elems) => {
            let a = agent.array_create(0, None)?;
            for (i, e) in elems.iter().enumerate() {
                let v = materialize(agent, e)?;
                agent.create_data_property_or_throw(a, PropertyKey::Index(i as u32), v)?;
            }
            Ok(Value::Object(a))
        }
        Node::Object(members) => {
            let o = agent.new_plain_object();
            for (k, e) in members {
                let v = materialize(agent, e)?;
                agent.create_data_property_or_throw(
                    o,
                    PropertyKey::from_js_string(k.clone()),
                    v,
                )?;
            }
            Ok(Value::Object(o))
        }
    }
}

/// JSON.parse (§25.5.1).
fn parse(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let text = agent.to_string(&a.arg(0))?;
    let node = parse_text(agent, text.units())?;
    let unfiltered = materialize(agent, &node)?;
    let reviver = a.arg(1);
    if !agent.is_callable(&reviver) {
        return Ok(unfiltered);
    }
    let root = agent.new_plain_object();
    agent.create_data_property_or_throw(root, PropertyKey::from(""), unfiltered)?;
    internalize(agent, root, PropertyKey::from(""), &reviver, Some(&node))
}

/// El nodo hijo de `node` para la clave (si la estructura sigue igual).
fn child<'n>(node: Option<&'n Node>, key: &PropertyKey) -> Option<&'n Node> {
    match node? {
        Node::Array(elems) => key.as_index().and_then(|i| elems.get(i as usize)),
        Node::Object(members) => {
            let k = key.to_js_string()?;
            members.iter().rev().find(|(n, _)| *n == k).map(|(_, v)| v)
        }
        Node::Primitive { .. } => None,
    }
}

/// InternalizeJSONProperty (§25.5.1.1).
fn internalize(
    agent: &mut Agent,
    holder: Gc<Object>,
    name: PropertyKey,
    reviver: &Value,
    node: Option<&Node>,
) -> JsResult<Value> {
    if agent.stack_exhausted() {
        return agent.range_error("Maximum call stack size exceeded");
    }
    let val = agent.get(holder, &name, Value::Object(holder))?;
    let context = agent.new_plain_object();
    if let Value::Object(o) = &val {
        let o = *o;
        let is_arr = agent.heap[o].is_array();
        let node = match node {
            Some(Node::Array(_)) if is_arr => node,
            Some(Node::Object(_)) if !is_arr => node,
            _ => None,
        };
        if agent.is_array(&val)? {
            let len = agent.length_of_array_like(o)?;
            let mut i = 0.0;
            while i < len {
                let prop = PropertyKey::from_number(i);
                let sub = child(node, &prop);
                let new = internalize(agent, o, prop.clone(), reviver, sub)?;
                if new.is_undefined() {
                    agent.delete(o, &prop)?;
                } else {
                    agent.create_data_property(o, prop, new)?;
                }
                i += 1.0;
            }
        } else {
            let keys = agent.enumerable_own_properties(o, IterKind::Keys)?;
            for k in keys {
                let prop = agent.to_property_key(&k)?;
                let sub = child(node, &prop);
                let new = internalize(agent, o, prop.clone(), reviver, sub)?;
                if new.is_undefined() {
                    agent.delete(o, &prop)?;
                } else {
                    agent.create_data_property(o, prop, new)?;
                }
            }
        }
    } else if let Some(Node::Primitive { value, source }) = node
        && agent.same_value(value, &val)
    {
        agent.create_data_property_or_throw(
            context,
            PropertyKey::from("source"),
            Value::String(source.clone()),
        )?;
    }
    agent.call(
        reviver,
        Value::Object(holder),
        &[name.to_value(), val, Value::Object(context)],
    )
}

// --- rawJSON ---

/// JSON.rawJSON (propuesta JSON.parse source text access).
fn raw_json(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = agent.to_string(&a.arg(0))?;
    let u = s.units();
    let is_ws = |c: u16| matches!(c, 0x09 | 0x0A | 0x0D | 0x20);
    if u.is_empty() || is_ws(u[0]) || is_ws(u[u.len() - 1]) {
        return agent.syntax_error("Invalid value for JSON.rawJSON");
    }
    let node = parse_text(agent, u)?;
    if !matches!(node, Node::Primitive { .. }) {
        return agent.syntax_error("JSON.rawJSON cannot create objects or arrays");
    }
    let o = agent.alloc(Object::new(None, ObjectKind::RawJson));
    agent.create_data_property_or_throw(o, PropertyKey::from("rawJSON"), Value::String(s))?;
    agent.set_integrity_level(o, true)?;
    Ok(Value::Object(o))
}

/// JSON.isRawJSON.
fn is_raw_json(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(Value::Boolean(matches!(
        a.arg(0),
        Value::Object(o) if matches!(agent.heap[o].kind, ObjectKind::RawJson)
    )))
}

// --- stringify ---

struct State {
    replacer: Option<Value>,
    property_list: Option<Vec<PropertyKey>>,
    stack: Vec<Gc<Object>>,
    indent: Vec<u16>,
    gap: Vec<u16>,
}

/// JSON.stringify (§25.5.2).
fn stringify(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let mut state = State {
        replacer: None,
        property_list: None,
        stack: Vec::new(),
        indent: Vec::new(),
        gap: Vec::new(),
    };
    let replacer = a.arg(1);
    if let Value::Object(r) = &replacer {
        if agent.is_callable(&replacer) {
            state.replacer = Some(replacer.clone());
        } else if agent.is_array(&replacer)? {
            let r = *r;
            let mut list: Vec<PropertyKey> = Vec::new();
            let len = agent.length_of_array_like(r)?;
            let mut k = 0.0;
            while k < len {
                let v = agent.get(r, &PropertyKey::from_number(k), replacer.clone())?;
                let item = match &v {
                    Value::String(s) => Some(s.clone()),
                    Value::Number(_) => Some(agent.to_string(&v)?),
                    Value::Object(o)
                        if matches!(
                            agent.heap[*o].kind,
                            ObjectKind::String(_) | ObjectKind::Number(_)
                        ) =>
                    {
                        Some(agent.to_string(&v)?)
                    }
                    _ => None,
                };
                if let Some(item) = item {
                    let key = PropertyKey::from_js_string(item);
                    if !list.contains(&key) {
                        list.push(key);
                    }
                }
                k += 1.0;
            }
            state.property_list = Some(list);
        }
    }
    let mut space = a.arg(2);
    if let Value::Object(o) = &space {
        match agent.heap[*o].kind {
            ObjectKind::Number(_) => space = Value::Number(agent.to_number(&space)?),
            ObjectKind::String(_) => space = Value::String(agent.to_string(&space)?),
            _ => {}
        }
    }
    match &space {
        Value::Number(_) => {
            let n = agent.to_integer_or_infinity(&space)?.min(10.0);
            if n >= 1.0 {
                state.gap = vec![0x20; n as usize];
            }
        }
        Value::String(s) => state.gap = s.units().iter().take(10).copied().collect(),
        _ => {}
    }
    let wrapper = agent.new_plain_object();
    agent.create_data_property_or_throw(wrapper, PropertyKey::from(""), a.arg(0))?;
    let r = serialize_property(agent, &mut state, PropertyKey::from(""), wrapper)?;
    Ok(r.map_or(Value::Undefined, |u| Value::String(JsString::from_units(u))))
}

/// SerializeJSONProperty (§25.5.2.2).
fn serialize_property(
    agent: &mut Agent,
    state: &mut State,
    key: PropertyKey,
    holder: Gc<Object>,
) -> JsResult<Option<Vec<u16>>> {
    let mut value = agent.get(holder, &key, Value::Object(holder))?;
    if matches!(value, Value::Object(_) | Value::BigInt(_)) {
        let to_json = agent.get_value_of(&value, &PropertyKey::from("toJSON"))?;
        if agent.is_callable(&to_json) {
            value = agent.call(&to_json, value, &[key.to_value()])?;
        }
    }
    if let Some(r) = state.replacer.clone() {
        value = agent.call(&r, Value::Object(holder), &[key.to_value(), value])?;
    }
    if let Value::Object(o) = &value {
        let o = *o;
        match agent.heap[o].kind {
            ObjectKind::RawJson => {
                let raw = agent.get_str(o, "rawJSON")?;
                let s = agent.to_string(&raw)?;
                return Ok(Some(s.units().to_vec()));
            }
            ObjectKind::Number(_) => value = Value::Number(agent.to_number(&value)?),
            ObjectKind::String(_) => value = Value::String(agent.to_string(&value)?),
            ObjectKind::Boolean(b) => value = Value::Boolean(b),
            ObjectKind::BigInt(b) => value = Value::BigInt(b),
            _ => {}
        }
    }
    match &value {
        Value::Null => Ok(Some("null".encode_utf16().collect())),
        Value::Boolean(true) => Ok(Some("true".encode_utf16().collect())),
        Value::Boolean(false) => Ok(Some("false".encode_utf16().collect())),
        Value::String(s) => Ok(Some(quote(s.units()))),
        Value::Number(n) => Ok(Some(if n.is_finite() {
            crate::abstract_ops::number::number_to_string(*n)
                .units()
                .to_vec()
        } else {
            "null".encode_utf16().collect()
        })),
        Value::BigInt(_) => agent.type_error("Do not know how to serialize a BigInt"),
        Value::Object(o) if !agent.heap[*o].is_callable() => {
            if agent.is_array(&value)? {
                serialize_array(agent, state, *o).map(Some)
            } else {
                serialize_object(agent, state, *o).map(Some)
            }
        }
        _ => Ok(None),
    }
}

/// QuoteJSONString (§25.5.2.3).
fn quote(s: &[u16]) -> Vec<u16> {
    let mut out = vec![u16::from(b'"')];
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        let esc = match c {
            0x08 => Some("\\b"),
            0x09 => Some("\\t"),
            0x0A => Some("\\n"),
            0x0C => Some("\\f"),
            0x0D => Some("\\r"),
            0x22 => Some("\\\""),
            0x5C => Some("\\\\"),
            _ => None,
        };
        if let Some(e) = esc {
            out.extend(e.encode_utf16());
        } else if c < 0x20 {
            out.extend(format!("\\u{c:04x}").encode_utf16());
        } else if (0xD800..0xDC00).contains(&c)
            && s.get(i + 1).is_some_and(|d| (0xDC00..0xE000).contains(d))
        {
            out.push(c);
            out.push(s[i + 1]);
            i += 1;
        } else if (0xD800..0xE000).contains(&c) {
            out.extend(format!("\\u{c:04x}").encode_utf16());
        } else {
            out.push(c);
        }
        i += 1;
    }
    out.push(u16::from(b'"'));
    out
}

fn enter(agent: &mut Agent, state: &mut State, o: Gc<Object>) -> JsResult<Vec<u16>> {
    if state.stack.contains(&o) {
        return agent.type_error("Converting circular structure to JSON");
    }
    if agent.stack_exhausted() {
        return agent.range_error("Maximum call stack size exceeded");
    }
    state.stack.push(o);
    let stepback = state.indent.clone();
    state.indent.extend_from_slice(&state.gap.clone());
    Ok(stepback)
}

fn join(state: &State, partial: Vec<Vec<u16>>, open: u8, close: u8, stepback: &[u16]) -> Vec<u16> {
    let mut out = vec![u16::from(open)];
    if partial.is_empty() {
        out.push(u16::from(close));
        return out;
    }
    if state.gap.is_empty() {
        for (i, p) in partial.into_iter().enumerate() {
            if i > 0 {
                out.push(u16::from(b','));
            }
            out.extend(p);
        }
    } else {
        for (i, p) in partial.into_iter().enumerate() {
            if i > 0 {
                out.push(u16::from(b','));
            }
            out.push(0x0A);
            out.extend_from_slice(&state.indent);
            out.extend(p);
        }
        out.push(0x0A);
        out.extend_from_slice(stepback);
    }
    out.push(u16::from(close));
    out
}

/// SerializeJSONObject (§25.5.2.5).
fn serialize_object(agent: &mut Agent, state: &mut State, o: Gc<Object>) -> JsResult<Vec<u16>> {
    let stepback = enter(agent, state, o)?;
    let keys: Vec<PropertyKey> = match &state.property_list {
        Some(list) => list.clone(),
        None => {
            let ks = agent.enumerable_own_properties(o, IterKind::Keys)?;
            let mut out = Vec::with_capacity(ks.len());
            for k in ks {
                out.push(agent.to_property_key(&k)?);
            }
            out
        }
    };
    let mut partial = Vec::new();
    for p in keys {
        if let Some(v) = serialize_property(agent, state, p.clone(), o)? {
            let mut member = quote(p.to_js_string().unwrap_or_default().units());
            member.push(u16::from(b':'));
            if !state.gap.is_empty() {
                member.push(0x20);
            }
            member.extend(v);
            partial.push(member);
        }
    }
    let out = join(state, partial, b'{', b'}', &stepback);
    state.stack.pop();
    state.indent = stepback;
    Ok(out)
}

/// SerializeJSONArray (§25.5.2.6).
fn serialize_array(agent: &mut Agent, state: &mut State, o: Gc<Object>) -> JsResult<Vec<u16>> {
    let stepback = enter(agent, state, o)?;
    let len = agent.length_of_array_like(o)?;
    let mut partial = Vec::new();
    let mut i = 0.0;
    while i < len {
        let v = serialize_property(agent, state, PropertyKey::from_number(i), o)?;
        partial.push(v.unwrap_or_else(|| "null".encode_utf16().collect()));
        i += 1.0;
    }
    let out = join(state, partial, b'[', b']', &stepback);
    state.stack.pop();
    state.indent = stepback;
    Ok(out)
}
