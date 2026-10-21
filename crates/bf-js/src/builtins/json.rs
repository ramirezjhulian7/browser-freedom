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
    /// `source` solo se guarda cuando hay `reviver` (E3).
    Primitive {
        value: Value,
        source: Option<JsString>,
    },
    Array(Vec<Node>),
    Object(Vec<(JsString, Node)>),
}

struct Parser<'a> {
    s: &'a [u16],
    pos: usize,
    /// ¿Guardar el texto de los primitivos (para el contexto del reviver)?
    keep_source: bool,
    /// Las claves ya vistas en este texto (E3): los objetos de un mismo JSON comparten sus
    /// strings de clave (y sus shapes se comparan por puntero).
    keys: std::collections::HashMap<Vec<u16>, JsString>,
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

    /// El tramo sin escapes de un string que empieza en la posición actual (después de la
    /// comilla), si termina en una comilla: se copia de una vez (E3).
    fn plain_span(&self) -> Option<(usize, usize)> {
        let start = self.pos;
        let len = self.s[start..]
            .iter()
            .position(|&c| c == 0x22 || c == 0x5C || c < 0x20)?;
        (self.s[start + len] == 0x22).then_some((start, start + len))
    }

    fn string(&mut self) -> Option<JsString> {
        if !self.eat(b'"') {
            return None;
        }
        if let Some((a, b)) = self.plain_span() {
            self.pos = b + 1;
            return Some(JsString::from_slice(&self.s[a..b]));
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

    /// Una clave de objeto (internada: la misma clave da el mismo string).
    fn key(&mut self) -> Option<JsString> {
        if self.peek() == Some(0x22) {
            self.pos += 1;
            let span = self.plain_span();
            self.pos -= 1;
            if let Some((a, b)) = span
                && let Some(e) = self.keys.get(&self.s[a..b])
            {
                self.pos = b + 1;
                return Some(e.clone());
            }
        }
        let k = self.string()?;
        if let Some(e) = self.keys.get(k.units()) {
            return Some(e.clone());
        }
        if self.keys.len() < 1024 {
            self.keys.insert(k.units().to_vec(), k.clone());
        }
        Some(k)
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
        // Un entero corto sin signo menos ni exponente se convierte directo (E3).
        let span = &self.s[start..self.pos];
        if span.len() <= 15 && span.iter().all(|c| (0x30..=0x39).contains(c)) {
            let n = span.iter().fold(0u64, |n, c| n * 10 + u64::from(c - 0x30));
            return Some(n as f64);
        }
        let text = String::from_utf16(span).ok()?;
        text.parse().ok()
    }

    /// Un valor construido directamente, sin el árbol (sin reviver; E3). Crear objetos no
    /// recolecta (la recolección es en los safepoints), así que los valores a medio armar no
    /// necesitan enraizarse.
    fn value_direct(&mut self, agent: &mut Agent) -> JsResult<Option<Value>> {
        if agent.stack_exhausted() {
            return agent.range_error("Maximum call stack size exceeded");
        }
        self.ws();
        let Some(c) = self.peek() else {
            return Ok(None);
        };
        let v = match c {
            0x7B => {
                self.pos += 1;
                let o = agent.new_plain_object();
                self.ws();
                if !self.eat(b'}') {
                    loop {
                        self.ws();
                        let Some(k) = self.key() else {
                            return Ok(None);
                        };
                        self.ws();
                        if !self.eat(b':') {
                            return Ok(None);
                        }
                        let Some(v) = self.value_direct(agent)? else {
                            return Ok(None);
                        };
                        // Un objeto ordinario nuevo: definir es agregar (o reemplazar una
                        // clave repetida).
                        agent.heap[o].props.insert(
                            PropertyKey::from_js_string(k),
                            crate::object::Prop::data(v, true, true, true),
                        );
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
                Value::Object(o)
            }
            0x5B => {
                self.pos += 1;
                let mut elems = Vec::new();
                self.ws();
                if !self.eat(b']') {
                    loop {
                        let Some(v) = self.value_direct(agent)? else {
                            return Ok(None);
                        };
                        elems.push(Some(v));
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
                let a = agent.array_create(0, None)?;
                if let ObjectKind::Array(d) = &mut agent.heap[a].kind {
                    d.length = elems.len() as u32;
                    d.dense = elems;
                }
                Value::Object(a)
            }
            0x22 => match self.string() {
                Some(s) => Value::String(s),
                None => return Ok(None),
            },
            0x74 if self.literal("true") => Value::Boolean(true),
            0x66 if self.literal("false") => Value::Boolean(false),
            0x6E if self.literal("null") => Value::Null,
            _ => match self.number() {
                Some(n) => Value::Number(n),
                None => return Ok(None),
            },
        };
        Ok(Some(v))
    }

    fn value(&mut self, agent: &mut Agent) -> JsResult<Option<Node>> {
        if agent.stack_exhausted() {
            return agent.range_error("Maximum call stack size exceeded");
        }
        self.ws();
        let start = self.pos;
        let prim = |p: &Parser<'_>, value: Value| Node::Primitive {
            value,
            source: p
                .keep_source
                .then(|| JsString::from_units(p.s[start..p.pos].to_vec())),
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
                        let Some(k) = self.key() else {
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
fn parse_text(agent: &mut Agent, s: &[u16], keep_source: bool) -> JsResult<Node> {
    let mut p = Parser {
        s,
        pos: 0,
        keep_source,
        keys: std::collections::HashMap::new(),
    };
    let v = p.value(agent)?;
    p.ws();
    match v {
        Some(node) if p.pos == s.len() => Ok(node),
        _ => agent.syntax_error("Unexpected token in JSON"),
    }
}

/// Parsea y construye los valores sin el árbol (sin reviver; E3).
fn parse_direct(agent: &mut Agent, s: &[u16]) -> JsResult<Value> {
    let mut p = Parser {
        s,
        pos: 0,
        keep_source: false,
        keys: std::collections::HashMap::new(),
    };
    let v = p.value_direct(agent)?;
    p.ws();
    match v {
        Some(v) if p.pos == s.len() => Ok(v),
        _ => agent.syntax_error("Unexpected token in JSON"),
    }
}

/// Construye los valores ECMAScript del árbol.
fn materialize(agent: &mut Agent, n: &Node) -> JsResult<Value> {
    match n {
        Node::Primitive { value, .. } => Ok(value.clone()),
        // Un Array y un objeto ordinario nuevos (extensibles, sin setters propios): definir
        // las propiedades es agregarlas (E3).
        Node::Array(elems) => {
            let a = agent.array_create(0, None)?;
            for e in elems {
                let v = materialize(agent, e)?;
                if let ObjectKind::Array(d) = &mut agent.heap[a].kind {
                    d.dense.push(Some(v));
                    d.length += 1;
                }
            }
            Ok(Value::Object(a))
        }
        Node::Object(members) => {
            let o = agent.new_plain_object();
            for (k, e) in members {
                let v = materialize(agent, e)?;
                agent.heap[o].props.insert(
                    PropertyKey::from_js_string(k.clone()),
                    crate::object::Prop::data(v, true, true, true),
                );
            }
            Ok(Value::Object(o))
        }
    }
}

/// ParseJSON (§25.5.1 pasos 2–10 sin reviver): para los módulos JSON (ParseJSONModule).
pub fn parse_json(agent: &mut Agent, s: &[u16]) -> JsResult<Value> {
    parse_direct(agent, s)
}

/// JSON.parse (§25.5.1).
fn parse(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let text = agent.to_string(&a.arg(0))?;
    if !agent.is_callable(&a.arg(1)) {
        return parse_direct(agent, text.units());
    }
    let node = parse_text(agent, text.units(), true)?;
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
    } else if let Some(Node::Primitive {
        value,
        source: Some(source),
    }) = node
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
    let node = parse_text(agent, u, false)?;
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
    /// La clave `toJSON` (creada una vez).
    to_json: PropertyKey,
}

/// JSON.stringify (§25.5.2).
fn stringify(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let mut state = State {
        replacer: None,
        property_list: None,
        stack: Vec::new(),
        indent: Vec::new(),
        gap: Vec::new(),
        to_json: PropertyKey::from("toJSON"),
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
    let mut out = Vec::new();
    let written = serialize_property(agent, &mut state, &PropertyKey::from(""), wrapper, &mut out)?;
    Ok(if written {
        Value::String(JsString::from_units(out))
    } else {
        Value::Undefined
    })
}

fn push_str(out: &mut Vec<u16>, s: &str) {
    out.extend(s.bytes().map(u16::from));
}

/// SerializeJSONProperty (§25.5.2.2): escribe el valor en `out`; `false` si es `undefined`
/// (no escribe nada).
fn serialize_property(
    agent: &mut Agent,
    state: &mut State,
    key: &PropertyKey,
    holder: Gc<Object>,
    out: &mut Vec<u16>,
) -> JsResult<bool> {
    let mut value = match own_data(agent, holder, key) {
        Some(v) => v,
        None => agent.get(holder, key, Value::Object(holder))?,
    };
    if matches!(value, Value::Object(_) | Value::BigInt(_)) {
        let to_json = agent.get_value_of(&value, &state.to_json)?;
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
                out.extend_from_slice(s.units());
                return Ok(true);
            }
            ObjectKind::Number(_) => value = Value::Number(agent.to_number(&value)?),
            ObjectKind::String(_) => value = Value::String(agent.to_string(&value)?),
            ObjectKind::Boolean(b) => value = Value::Boolean(b),
            ObjectKind::BigInt(b) => value = Value::BigInt(b),
            _ => {}
        }
    }
    match &value {
        Value::Null => push_str(out, "null"),
        Value::Boolean(true) => push_str(out, "true"),
        Value::Boolean(false) => push_str(out, "false"),
        Value::String(s) => quote(s.units(), out),
        Value::Number(n) => {
            if n.is_finite() {
                out.extend_from_slice(crate::abstract_ops::number::number_to_string(*n).units());
            } else {
                push_str(out, "null");
            }
        }
        Value::BigInt(_) => return agent.type_error("Do not know how to serialize a BigInt"),
        Value::Object(o) if !agent.heap[*o].is_callable() => {
            let o = *o;
            if agent.is_array(&value)? {
                serialize_array(agent, state, o, out)?;
            } else {
                serialize_object(agent, state, o, out)?;
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// `Get(holder, key)` sin el agente cuando es una propiedad de datos propia de un objeto
/// ordinario o un elemento de un Array denso (E3); `None` si hace falta el `[[Get]]` completo.
fn own_data(agent: &Agent, holder: Gc<Object>, key: &PropertyKey) -> Option<Value> {
    let obj = &agent.heap[holder];
    match (&obj.kind, key) {
        (ObjectKind::Array(d), PropertyKey::Index(i)) => d.dense.get(*i as usize)?.clone(),
        (ObjectKind::Ordinary, _) => match &obj.props.get(key)?.value {
            crate::object::PropValue::Data { value, .. } => Some(value.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// QuoteJSONString (§25.5.2.3).
fn quote(s: &[u16], out: &mut Vec<u16>) {
    out.reserve(s.len() + 2);
    out.push(u16::from(b'"'));
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
            push_str(out, e);
        } else if c < 0x20 {
            push_str(out, &format!("\\u{c:04x}"));
        } else if (0xD800..0xDC00).contains(&c)
            && s.get(i + 1).is_some_and(|d| (0xDC00..0xE000).contains(d))
        {
            out.push(c);
            out.push(s[i + 1]);
            i += 1;
        } else if (0xD800..0xE000).contains(&c) {
            push_str(out, &format!("\\u{c:04x}"));
        } else {
            out.push(c);
        }
        i += 1;
    }
    out.push(u16::from(b'"'));
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
    let gap = state.gap.clone();
    state.indent.extend_from_slice(&gap);
    Ok(stepback)
}

/// El separador antes del elemento `i` (y la sangría con `space`).
fn separator(state: &State, i: usize, out: &mut Vec<u16>) {
    if i > 0 {
        out.push(u16::from(b','));
    }
    if !state.gap.is_empty() {
        out.push(0x0A);
        out.extend_from_slice(&state.indent);
    }
}

/// El cierre de un objeto o array con `count` elementos.
fn close(state: &State, count: usize, close: u8, stepback: &[u16], out: &mut Vec<u16>) {
    if count > 0 && !state.gap.is_empty() {
        out.push(0x0A);
        out.extend_from_slice(stepback);
    }
    out.push(u16::from(close));
}

/// SerializeJSONObject (§25.5.2.5).
fn serialize_object(
    agent: &mut Agent,
    state: &mut State,
    o: Gc<Object>,
    out: &mut Vec<u16>,
) -> JsResult<()> {
    let stepback = enter(agent, state, o)?;
    let keys: Vec<PropertyKey> = match &state.property_list {
        Some(list) => list.clone(),
        // Un objeto ordinario: sus claves y atributos se leen directo (E3).
        None if matches!(agent.heap[o].kind, ObjectKind::Ordinary) => {
            let props = &agent.heap[o].props;
            let mut keys = props.keys();
            keys.retain(|k| !k.is_symbol() && props.get(k).is_some_and(|p| p.enumerable));
            keys
        }
        None => {
            // EnumerableOwnProperties(O, key): las claves string enumerables.
            let mut keys = Vec::new();
            for k in agent.own_property_keys(o)? {
                if k.is_symbol() {
                    continue;
                }
                if let Some(d) = agent.get_own_property(o, &k)?
                    && d.enumerable == Some(true)
                {
                    keys.push(k);
                }
            }
            keys
        }
    };
    out.push(u16::from(b'{'));
    let mut count = 0;
    for p in &keys {
        let mark = out.len();
        separator(state, count, out);
        quote(p.to_js_string().unwrap_or_default().units(), out);
        out.push(u16::from(b':'));
        if !state.gap.is_empty() {
            out.push(0x20);
        }
        if serialize_property(agent, state, p, o, out)? {
            count += 1;
        } else {
            out.truncate(mark);
        }
    }
    close(state, count, b'}', &stepback, out);
    state.stack.pop();
    state.indent = stepback;
    Ok(())
}

/// SerializeJSONArray (§25.5.2.6).
fn serialize_array(
    agent: &mut Agent,
    state: &mut State,
    o: Gc<Object>,
    out: &mut Vec<u16>,
) -> JsResult<()> {
    let stepback = enter(agent, state, o)?;
    let len = agent.length_of_array_like(o)?;
    out.push(u16::from(b'['));
    let mut i = 0.0;
    let mut count = 0;
    while i < len {
        separator(state, count, out);
        if !serialize_property(agent, state, &PropertyKey::from_number(i), o, out)? {
            push_str(out, "null");
        }
        count += 1;
        i += 1.0;
    }
    close(state, count, b']', &stepback, out);
    state.stack.pop();
    state.indent = stepback;
    Ok(())
}
