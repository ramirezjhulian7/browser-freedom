//! RegExp (ECMA-262 §22.2.4–§22.2.9): el constructor, RegExp.prototype con
//! `[Symbol.match|matchAll|replace|search|split]`, %RegExpStringIteratorPrototype%,
//! `RegExp.escape`, `RegExp.prototype.compile` (Annex B.2.4) y las propiedades estáticas
//! legacy (`RegExp.$1`, `RegExp.lastMatch`…, propuesta *RegExp Legacy Features* que test262
//! prueba en `annexB/built-ins/RegExp/legacy-accessors`). El motor es [`crate::regexp`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::abstract_ops::conversion::to_boolean;
use crate::builtins::{FnEntry, accessor, constructor, method, method_sym, methods, to_string_tag};
use crate::heap::{Gc, Trace, Tracer};
use crate::object::{Object, ObjectKind};
use crate::regexp::{MatchError, Regex};
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId, Throw};

/// Los internal slots de una instancia de RegExp.
#[derive(Debug)]
pub struct RegExpData {
    /// `[[OriginalSource]]`.
    pub source: JsString,
    /// `[[OriginalFlags]]`.
    pub flags: JsString,
    /// `[[RegExpMatcher]]` (y `[[RegExpRecord]]`): `None` hasta RegExpInitialize.
    pub matcher: Option<Rc<Regex>>,
    /// `[[Realm]]` y `[[LegacyFeaturesEnabled]]` (RegExp Legacy Features).
    pub realm: RealmId,
    pub legacy: bool,
}

/// Los slots de un RegExp String Iterator (§22.2.9).
#[derive(Debug)]
pub struct RegExpStringIteratorData {
    pub regexp: Value,
    pub string: JsString,
    pub global: bool,
    pub unicode: bool,
    pub done: bool,
}

impl Trace for RegExpStringIteratorData {
    fn trace(&self, t: &mut Tracer) {
        t.value(&self.regexp);
    }
}

pub fn init(agent: &mut Agent, realm: RealmId) {
    let op = agent.realms[realm].intrinsics.object_prototype;
    // %RegExp.prototype% es un objeto ordinario (§22.2.6).
    let proto = agent.new_object(Some(op));
    agent.realms[realm].intrinsics.regexp_prototype = proto;
    let ctor = constructor(agent, "RegExp", 2, regexp_ctor, proto);
    agent.realms[realm].intrinsics.regexp_ctor = ctor;
    crate::builtins::register_global(agent, realm, "RegExp", ctor);
    init_legacy_state(agent, ctor);
    method(agent, ctor, "escape", 1, escape);
    let species = PropertyKey::Symbol(agent.well_known.species);
    accessor(
        agent,
        ctor,
        species,
        "[Symbol.species]",
        Some(species_getter),
        None,
    );
    const PROTO: &[FnEntry] = &[
        ("compile", 2, compile),
        ("exec", 1, exec),
        ("test", 1, test),
        ("toString", 0, to_string),
    ];
    methods(agent, proto, PROTO);
    let flag_getters: &[(&str, crate::vm::function::NativeFn)] = &[
        ("dotAll", get_dot_all),
        ("flags", get_flags),
        ("global", get_global),
        ("hasIndices", get_has_indices),
        ("ignoreCase", get_ignore_case),
        ("multiline", get_multiline),
        ("source", get_source),
        ("sticky", get_sticky),
        ("unicode", get_unicode),
        ("unicodeSets", get_unicode_sets),
    ];
    for &(name, f) in flag_getters {
        accessor(agent, proto, PropertyKey::from(name), name, Some(f), None);
    }
    let wk = &agent.well_known;
    let syms: [(
        crate::heap::Gc<crate::value::Symbol>,
        &str,
        u32,
        crate::vm::function::NativeFn,
    ); 5] = [
        (wk.match_, "[Symbol.match]", 1, symbol_match),
        (wk.match_all, "[Symbol.matchAll]", 1, symbol_match_all),
        (wk.replace, "[Symbol.replace]", 2, symbol_replace),
        (wk.search, "[Symbol.search]", 1, symbol_search),
        (wk.split, "[Symbol.split]", 2, symbol_split),
    ];
    for (sym, name, len, f) in syms {
        method_sym(agent, proto, sym, name, len, f);
    }
    // %RegExpStringIteratorPrototype% (§22.2.9.2).
    let ip = agent.realms[realm].intrinsics.iterator_prototype;
    let sip = agent.new_object(Some(ip));
    method(agent, sip, "next", 0, string_iterator_next);
    to_string_tag(agent, sip, "RegExp String Iterator");
    agent.realms[realm]
        .intrinsics
        .regexp_string_iterator_prototype = sip;
    init_legacy_accessors(agent, ctor);
}

fn species_getter(_: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    Ok(a.this.clone())
}

// --- Caché de patrones compilados ---

thread_local! {
    /// Patrones ya compilados por (fuente, flags): los literales se evalúan muchas veces.
    static CACHE: RefCell<HashMap<(JsString, String), Rc<Regex>>> = RefCell::new(HashMap::new());
}

const CACHE_LIMIT: usize = 512;

fn compile_cached(source: &JsString, flags: &str) -> Result<Rc<Regex>, String> {
    let key = (source.clone(), flags.to_owned());
    if let Some(r) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return Ok(r);
    }
    let r = Rc::new(Regex::new(source.units(), flags)?);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= CACHE_LIMIT {
            c.clear();
        }
        c.insert(key, r.clone());
    });
    Ok(r)
}

// --- Creación ---

fn regexp_data(agent: &Agent, o: Gc<Object>) -> Option<&RegExpData> {
    match &agent.heap[o].kind {
        ObjectKind::RegExp(d) => Some(d),
        _ => None,
    }
}

fn is_regexp_object(agent: &Agent, v: &Value) -> Option<Gc<Object>> {
    match v {
        Value::Object(o) if regexp_data(agent, *o).is_some() => Some(*o),
        _ => None,
    }
}

/// RegExp ( pattern, flags ) (§22.2.4.1).
fn regexp_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let pattern = a.arg(0);
    let flags = a.arg(1);
    let pattern_is_regexp = agent.is_regexp(&pattern)?;
    let new_target = match a.new_target {
        Some(nt) => nt,
        None => {
            let nt = a.callee;
            if pattern_is_regexp && flags.is_undefined() {
                let Value::Object(po) = &pattern else {
                    unreachable!("IsRegExp de un no objeto")
                };
                let k = agent.keys.constructor.clone();
                let pc = agent.get(*po, &k, pattern.clone())?;
                if agent.same_value(&Value::Object(nt), &pc) {
                    return Ok(pattern);
                }
            }
            nt
        }
    };
    let (p, f) = if let Some(po) = is_regexp_object(agent, &pattern) {
        let d = regexp_data(agent, po).expect("RegExp");
        let src = Value::String(d.source.clone());
        let f = if flags.is_undefined() {
            Value::String(d.flags.clone())
        } else {
            flags
        };
        (src, f)
    } else if pattern_is_regexp {
        let Value::Object(po) = &pattern else {
            unreachable!("IsRegExp de un no objeto")
        };
        let src = agent.get_str(*po, "source")?;
        let f = if flags.is_undefined() {
            agent.get_str(*po, "flags")?
        } else {
            flags
        };
        (src, f)
    } else {
        (pattern, flags)
    };
    let o = regexp_alloc(agent, new_target)?;
    regexp_initialize(agent, o, &p, &f)
}

/// RegExpAlloc ( newTarget ) (§22.2.3.2).
fn regexp_alloc(agent: &mut Agent, new_target: Gc<Object>) -> JsResult<Gc<Object>> {
    let realm = agent.current_realm;
    let legacy = new_target == agent.intrinsics().regexp_ctor;
    let o = agent.ordinary_create_from_constructor(
        new_target,
        |i| i.regexp_prototype,
        ObjectKind::RegExp(Box::new(RegExpData {
            source: JsString::default(),
            flags: JsString::default(),
            matcher: None,
            realm,
            legacy,
        })),
    )?;
    agent.put_data(
        o,
        PropertyKey::from("lastIndex"),
        Value::Undefined,
        true,
        false,
        false,
    );
    Ok(o)
}

/// RegExpInitialize ( obj, pattern, flags ) (§22.2.3.3).
fn regexp_initialize(
    agent: &mut Agent,
    o: Gc<Object>,
    pattern: &Value,
    flags: &Value,
) -> JsResult<Value> {
    let p = if pattern.is_undefined() {
        JsString::default()
    } else {
        agent.to_string(pattern)?
    };
    let f = if flags.is_undefined() {
        JsString::default()
    } else {
        agent.to_string(flags)?
    };
    let Some(fs) = f.to_str() else {
        return agent.syntax_error(&format!(
            "Invalid regular expression flags '{}'",
            f.to_string_lossy()
        ));
    };
    let r = match compile_cached(&p, &fs) {
        Ok(r) => r,
        Err(m) => return agent.syntax_error(&m),
    };
    if let ObjectKind::RegExp(d) = &mut agent.heap[o].kind {
        d.source = p;
        d.flags = f;
        d.matcher = Some(r);
    }
    agent.set_prop(o, PropertyKey::from("lastIndex"), Value::Number(0.0))?;
    Ok(Value::Object(o))
}

/// RegExpCreate ( P, F ) (§22.2.3.1) para un literal ya validado (§13.2.7.3).
pub fn regexp_create_literal(
    agent: &mut Agent,
    pattern: &JsString,
    flags: &JsString,
) -> JsResult<Value> {
    let ctor = agent.intrinsics().regexp_ctor;
    let o = regexp_alloc(agent, ctor)?;
    regexp_initialize(
        agent,
        o,
        &Value::String(pattern.clone()),
        &Value::String(flags.clone()),
    )
}

// --- RegExp.prototype ---

/// `this` como RegExp inicializado (RequireInternalSlot(R, [[RegExpMatcher]])).
fn this_regexp(agent: &mut Agent, this: &Value, method: &str) -> JsResult<Gc<Object>> {
    if let Value::Object(o) = this
        && let Some(d) = regexp_data(agent, *o)
        && d.matcher.is_some()
    {
        return Ok(*o);
    }
    agent.type_error(&format!(
        "RegExp.prototype.{method} requires that 'this' be a RegExp object"
    ))
}

fn this_object(agent: &mut Agent, this: &Value, method: &str) -> JsResult<Gc<Object>> {
    match this {
        Value::Object(o) => Ok(*o),
        _ => agent.type_error(&format!(
            "RegExp.prototype.{method} called on incompatible receiver"
        )),
    }
}

/// RegExp.prototype.exec ( string ) (§22.2.6.2).
fn exec(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_regexp(agent, &a.this, "exec")?;
    let s = agent.to_string(&a.arg(0))?;
    regexp_builtin_exec(agent, r, &s)
}

/// RegExpExec ( R, S ) (§22.2.7.1).
pub fn regexp_exec(agent: &mut Agent, r: Gc<Object>, s: &JsString) -> JsResult<Value> {
    let exec = agent.get_str(r, "exec")?;
    if agent.is_callable(&exec) {
        let result = agent.call(&exec, Value::Object(r), &[Value::String(s.clone())])?;
        if !matches!(result, Value::Object(_) | Value::Null) {
            return agent.type_error("exec result must be an object or null");
        }
        return Ok(result);
    }
    if regexp_data(agent, r).is_none_or(|d| d.matcher.is_none()) {
        return agent.type_error("RegExp exec method called on incompatible receiver");
    }
    regexp_builtin_exec(agent, r, s)
}

fn match_error(agent: &mut Agent, e: MatchError) -> Throw {
    match e {
        MatchError::Timeout => Throw::Terminate("tiempo agotado".into()),
        MatchError::Limit(what) => match agent.internal_error::<()>(&format!("RegExp: {what}")) {
            Err(t) => t,
            Ok(()) => unreachable!("internal_error"),
        },
    }
}

/// RegExpBuiltinExec ( R, S ) (§22.2.7.2).
pub fn regexp_builtin_exec(agent: &mut Agent, r: Gc<Object>, s: &JsString) -> JsResult<Value> {
    let length = s.len() as f64;
    let li = agent.get_str(r, "lastIndex")?;
    let mut last_index = agent.to_length(&li)?;
    let (matcher, rrealm, legacy) = {
        let d = regexp_data(agent, r).expect("RegExp");
        (d.matcher.clone().expect("RegExpMatcher"), d.realm, d.legacy)
    };
    let flags = matcher.flags;
    let global = flags.global;
    let sticky = flags.sticky;
    let has_indices = flags.has_indices;
    if !global && !sticky {
        last_index = 0.0;
    }
    if last_index > length {
        if global || sticky {
            agent.set_prop(r, PropertyKey::from("lastIndex"), Value::Number(0.0))?;
        }
        return Ok(Value::Null);
    }
    let deadline = agent.options.deadline;
    let found = matcher.search_from(s.units(), last_index as usize, sticky, deadline);
    let found = match found {
        Ok(f) => f,
        Err(e) => return Err(match_error(agent, e)),
    };
    let Some((start, caps)) = found else {
        if global || sticky {
            agent.set_prop(r, PropertyKey::from("lastIndex"), Value::Number(0.0))?;
        }
        return Ok(Value::Null);
    };
    let last_index = start;
    let (_, e) = caps[0].expect("match");
    if global || sticky {
        agent.set_prop(r, PropertyKey::from("lastIndex"), Value::Number(e as f64))?;
    }
    let units = s.units();
    let n = matcher.group_count();
    let a = agent.array_create(n as u64 + 1, None)?;
    agent.create_data_property_or_throw(
        a,
        PropertyKey::from("index"),
        Value::Number(last_index as f64),
    )?;
    agent.create_data_property_or_throw(a, PropertyKey::from("input"), Value::String(s.clone()))?;
    let mut indices: Vec<Option<(usize, usize)>> = vec![Some((last_index, e))];
    let mut group_names: Vec<Option<String>> = Vec::new();
    let matched = JsString::from_units(units[last_index.min(e)..e].to_vec());
    agent.create_data_property_or_throw(a, PropertyKey::Index(0), Value::String(matched))?;
    let has_groups = matcher.group_names.iter().any(Option::is_some);
    let groups = if has_groups {
        Some(agent.new_object(None))
    } else {
        None
    };
    agent.create_data_property_or_throw(a, PropertyKey::from("groups"), Value::from(groups))?;
    let mut matched_names: Vec<String> = Vec::new();
    let mut captured_values: Vec<Option<(usize, usize)>> = Vec::with_capacity(n);
    for (i, &cap) in caps.iter().enumerate().skip(1) {
        let value = match cap {
            Some((cs, ce)) => Value::String(JsString::from_units(units[cs..ce].to_vec())),
            None => Value::Undefined,
        };
        indices.push(cap);
        captured_values.push(cap);
        agent.create_data_property_or_throw(a, PropertyKey::from(i as u32), value.clone())?;
        match &matcher.group_names[i - 1] {
            Some(name) => {
                if matched_names.contains(name) {
                    group_names.push(None);
                } else {
                    if !value.is_undefined() {
                        matched_names.push(name.clone());
                    }
                    if let Some(g) = groups {
                        agent.create_data_property_or_throw(
                            g,
                            PropertyKey::from(name.as_str()),
                            value,
                        )?;
                    }
                    group_names.push(Some(name.clone()));
                }
            }
            None => group_names.push(None),
        }
    }
    // RegExp Legacy Features: UpdateLegacyRegExpStaticProperties o la invalidación.
    if rrealm == agent.current_realm {
        let ctor = agent.intrinsics().regexp_ctor;
        if legacy {
            update_legacy(agent, ctor, s, last_index, e, &captured_values);
        } else {
            invalidate_legacy(agent, ctor);
        }
    }
    if has_indices {
        let ia = make_match_indices(agent, &indices, &group_names, has_groups)?;
        agent.create_data_property_or_throw(a, PropertyKey::from("indices"), Value::Object(ia))?;
    }
    Ok(Value::Object(a))
}

/// MakeMatchIndicesIndexPairArray (§22.2.7.8).
fn make_match_indices(
    agent: &mut Agent,
    indices: &[Option<(usize, usize)>],
    group_names: &[Option<String>],
    has_groups: bool,
) -> JsResult<Gc<Object>> {
    let a = agent.array_create(indices.len() as u64, None)?;
    let groups = if has_groups {
        Some(agent.new_object(None))
    } else {
        None
    };
    agent.create_data_property_or_throw(a, PropertyKey::from("groups"), Value::from(groups))?;
    for (i, m) in indices.iter().enumerate() {
        let pair =
            match m {
                Some((s, e)) => Value::Object(agent.create_array_from_list(&[
                    Value::Number(*s as f64),
                    Value::Number(*e as f64),
                ])?),
                None => Value::Undefined,
            };
        agent.create_data_property_or_throw(a, PropertyKey::from(i as u32), pair.clone())?;
        if i > 0
            && let Some(name) = &group_names[i - 1]
            && let Some(g) = groups
        {
            agent.create_data_property_or_throw(g, PropertyKey::from(name.as_str()), pair)?;
        }
    }
    Ok(a)
}

/// RegExp.prototype.test ( S ) (§22.2.6.16).
fn test(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_object(agent, &a.this, "test")?;
    let s = agent.to_string(&a.arg(0))?;
    let m = regexp_exec(agent, r, &s)?;
    Ok(Value::Boolean(!m.is_null()))
}

/// RegExp.prototype.toString ( ) (§22.2.6.17).
fn to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_object(agent, &a.this, "toString")?;
    let src = agent.get_str(r, "source")?;
    let p = agent.to_string(&src)?;
    let fl = agent.get_str(r, "flags")?;
    let f = agent.to_string(&fl)?;
    let mut u = vec![u16::from(b'/')];
    u.extend_from_slice(p.units());
    u.push(u16::from(b'/'));
    u.extend_from_slice(f.units());
    Ok(Value::String(JsString::from_units(u)))
}

/// RegExp.prototype.compile ( pattern, flags ) (Annex B.2.4.1), con las comprobaciones de
/// realm y subclase de RegExp Legacy Features.
fn compile(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_regexp(agent, &a.this, "compile")?;
    let (realm, legacy) = {
        let d = regexp_data(agent, o).expect("RegExp");
        (d.realm, d.legacy)
    };
    if realm != agent.current_realm {
        return agent.type_error("RegExp.prototype.compile called on a RegExp of another realm");
    }
    if !legacy {
        return agent.type_error("RegExp.prototype.compile called on a RegExp subclass instance");
    }
    let pattern = a.arg(0);
    let flags = a.arg(1);
    let (p, f) = if let Some(po) = is_regexp_object(agent, &pattern) {
        if !flags.is_undefined() {
            return agent
                .type_error("Cannot supply flags when constructing one RegExp from another");
        }
        let d = regexp_data(agent, po).expect("RegExp");
        (
            Value::String(d.source.clone()),
            Value::String(d.flags.clone()),
        )
    } else {
        (pattern, flags)
    };
    regexp_initialize(agent, o, &p, &f)
}

// --- Getters ---

/// RegExpHasFlag ( R, codeUnit ) (§22.2.6.4.1).
fn has_flag(agent: &mut Agent, this: &Value, flag: char, name: &str) -> JsResult<Value> {
    let Value::Object(o) = this else {
        return agent.type_error(&format!(
            "RegExp.prototype.{name} getter called on non-object"
        ));
    };
    match regexp_data(agent, *o) {
        Some(d) => Ok(Value::Boolean(d.flags.units().contains(&(flag as u16)))),
        None => {
            if *o == agent.intrinsics().regexp_prototype {
                return Ok(Value::Undefined);
            }
            agent.type_error(&format!(
                "RegExp.prototype.{name} getter called on non-RegExp object"
            ))
        }
    }
}

fn get_dot_all(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    has_flag(agent, &a.this, 's', "dotAll")
}

fn get_global(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    has_flag(agent, &a.this, 'g', "global")
}

fn get_has_indices(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    has_flag(agent, &a.this, 'd', "hasIndices")
}

fn get_ignore_case(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    has_flag(agent, &a.this, 'i', "ignoreCase")
}

fn get_multiline(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    has_flag(agent, &a.this, 'm', "multiline")
}

fn get_sticky(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    has_flag(agent, &a.this, 'y', "sticky")
}

fn get_unicode(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    has_flag(agent, &a.this, 'u', "unicode")
}

fn get_unicode_sets(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    has_flag(agent, &a.this, 'v', "unicodeSets")
}

/// get RegExp.prototype.flags (§22.2.6.4).
fn get_flags(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Value::Object(r) = a.this else {
        return agent.type_error("RegExp.prototype.flags getter called on non-object");
    };
    let mut out = Vec::new();
    for (name, c) in [
        ("hasIndices", 'd'),
        ("global", 'g'),
        ("ignoreCase", 'i'),
        ("multiline", 'm'),
        ("dotAll", 's'),
        ("unicode", 'u'),
        ("unicodeSets", 'v'),
        ("sticky", 'y'),
    ] {
        let v = agent.get_str(r, name)?;
        if to_boolean(agent, &v) {
            out.push(c as u16);
        }
    }
    Ok(Value::String(JsString::from_units(out)))
}

/// get RegExp.prototype.source (§22.2.6.13).
fn get_source(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Value::Object(r) = a.this else {
        return agent.type_error("RegExp.prototype.source getter called on non-object");
    };
    match regexp_data(agent, r) {
        Some(d) => Ok(Value::String(escape_regexp_pattern(&d.source))),
        None => {
            if r == agent.intrinsics().regexp_prototype {
                return Ok(Value::str("(?:)"));
            }
            agent.type_error("RegExp.prototype.source getter called on non-RegExp object")
        }
    }
}

/// EscapeRegExpPattern ( P, F ) (§22.2.6.13.1): `/` y los LineTerminator escapados para que
/// `/S/F` sea un literal equivalente; el patrón vacío es `(?:)`.
fn escape_regexp_pattern(src: &JsString) -> JsString {
    if src.is_empty() {
        return JsString::from("(?:)");
    }
    let u = src.units();
    let mut out = Vec::with_capacity(u.len());
    let mut i = 0;
    let mut in_class = false;
    while i < u.len() {
        let c = u[i];
        match c {
            0x5C => {
                // `\` y lo que escapa (un LineTerminator escapado se reescribe igual).
                out.push(c);
                if let Some(&n) = u.get(i + 1) {
                    i += 1;
                    match n {
                        0x0A => out.extend("n".encode_utf16()),
                        0x0D => out.extend("r".encode_utf16()),
                        0x2028 => out.extend("u2028".encode_utf16()),
                        0x2029 => out.extend("u2029".encode_utf16()),
                        _ => out.push(n),
                    }
                }
            }
            0x2F if !in_class => out.extend("\\/".encode_utf16()),
            0x0A => out.extend("\\n".encode_utf16()),
            0x0D => out.extend("\\r".encode_utf16()),
            0x2028 => out.extend("\\u2028".encode_utf16()),
            0x2029 => out.extend("\\u2029".encode_utf16()),
            _ => {
                if c == 0x5B {
                    in_class = true;
                } else if c == 0x5D {
                    in_class = false;
                }
                out.push(c);
            }
        }
        i += 1;
    }
    JsString::from_units(out)
}

// --- Symbol.match / matchAll / replace / search / split ---

/// AdvanceStringIndex ( S, index, unicode ) (§22.2.7.3) con índices de ToLength.
fn advance_index(s: &JsString, index: f64, unicode: bool) -> f64 {
    if !unicode || index + 1.0 >= s.len() as f64 {
        return index + 1.0;
    }
    crate::regexp::advance_string_index(s.units(), index as usize, true) as f64
}

fn flags_of(agent: &mut Agent, rx: Gc<Object>) -> JsResult<JsString> {
    let f = agent.get_str(rx, "flags")?;
    agent.to_string(&f)
}

fn has_unit(s: &JsString, c: char) -> bool {
    s.units().contains(&(c as u16))
}

/// Después de un match vacío con `g`: lastIndex avanza (pasos comunes de @@match, @@replace
/// y %RegExpStringIteratorPrototype%.next).
fn bump_last_index(
    agent: &mut Agent,
    rx: Gc<Object>,
    s: &JsString,
    full_unicode: bool,
) -> JsResult<()> {
    let li = agent.get_str(rx, "lastIndex")?;
    let this_index = agent.to_length(&li)?;
    let next = advance_index(s, this_index, full_unicode);
    agent.set_prop(rx, PropertyKey::from("lastIndex"), Value::Number(next))
}

fn match_string(agent: &mut Agent, result: &Value) -> JsResult<JsString> {
    let Value::Object(ro) = result else {
        unreachable!("resultado de exec")
    };
    let m = agent.get(*ro, &PropertyKey::Index(0), result.clone())?;
    agent.to_string(&m)
}

/// RegExp.prototype [ @@match ] ( string ) (§22.2.6.8).
fn symbol_match(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let rx = this_object(agent, &a.this, "[Symbol.match]")?;
    let s = agent.to_string(&a.arg(0))?;
    let flags = flags_of(agent, rx)?;
    if !has_unit(&flags, 'g') {
        return regexp_exec(agent, rx, &s);
    }
    let full_unicode = has_unit(&flags, 'u') || has_unit(&flags, 'v');
    agent.set_prop(rx, PropertyKey::from("lastIndex"), Value::Number(0.0))?;
    let arr = agent.array_create(0, None)?;
    let mut n: u32 = 0;
    loop {
        let result = regexp_exec(agent, rx, &s)?;
        if result.is_null() {
            return Ok(if n == 0 {
                Value::Null
            } else {
                Value::Object(arr)
            });
        }
        let m = match_string(agent, &result)?;
        let empty = m.is_empty();
        agent.create_data_property_or_throw(arr, PropertyKey::from(n), Value::String(m))?;
        if empty {
            bump_last_index(agent, rx, &s, full_unicode)?;
        }
        n += 1;
    }
}

/// RegExp.prototype [ @@matchAll ] ( string ) (§22.2.6.9).
fn symbol_match_all(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let r = this_object(agent, &a.this, "[Symbol.matchAll]")?;
    let s = agent.to_string(&a.arg(0))?;
    let default = agent.intrinsics().regexp_ctor;
    let c = agent.species_constructor(r, default)?;
    let flags = flags_of(agent, r)?;
    let matcher = agent.construct(c, &[Value::Object(r), Value::String(flags.clone())], None)?;
    let li = agent.get_str(r, "lastIndex")?;
    let last_index = agent.to_length(&li)?;
    let Value::Object(mo) = matcher else {
        return agent.type_error("RegExp species constructor returned a non-object");
    };
    agent.set_prop(
        mo,
        PropertyKey::from("lastIndex"),
        Value::Number(last_index),
    )?;
    let global = has_unit(&flags, 'g');
    let unicode = has_unit(&flags, 'u') || has_unit(&flags, 'v');
    let proto = agent.intrinsics().regexp_string_iterator_prototype;
    let it = agent.alloc(Object::new(
        Some(proto),
        ObjectKind::RegExpStringIterator(Box::new(RegExpStringIteratorData {
            regexp: Value::Object(mo),
            string: s,
            global,
            unicode,
            done: false,
        })),
    ));
    Ok(Value::Object(it))
}

/// %RegExpStringIteratorPrototype%.next ( ) (§22.2.9.2.1).
fn string_iterator_next(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Value::Object(o) = a.this else {
        return agent.type_error("RegExp String Iterator next called on incompatible receiver");
    };
    let (r, s, global, unicode, done) = match &agent.heap[o].kind {
        ObjectKind::RegExpStringIterator(d) => (
            d.regexp.clone(),
            d.string.clone(),
            d.global,
            d.unicode,
            d.done,
        ),
        _ => {
            return agent.type_error("RegExp String Iterator next called on incompatible receiver");
        }
    };
    if done {
        return agent.create_iter_result(Value::Undefined, true);
    }
    let set_done = |agent: &mut Agent| {
        if let ObjectKind::RegExpStringIterator(d) = &mut agent.heap[o].kind {
            d.done = true;
        }
    };
    let Value::Object(ro) = r else {
        unreachable!("RegExp del iterador")
    };
    let m = regexp_exec(agent, ro, &s)?;
    if m.is_null() {
        set_done(agent);
        return agent.create_iter_result(Value::Undefined, true);
    }
    if !global {
        set_done(agent);
        return agent.create_iter_result(m, false);
    }
    let ms = match_string(agent, &m)?;
    if ms.is_empty() {
        bump_last_index(agent, ro, &s, unicode)?;
    }
    agent.create_iter_result(m, false)
}

/// RegExp.prototype [ @@replace ] ( string, replaceValue ) (§22.2.6.11).
fn symbol_replace(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let rx = this_object(agent, &a.this, "[Symbol.replace]")?;
    let s = agent.to_string(&a.arg(0))?;
    let length_s = s.len();
    let mut replace_value = a.arg(1);
    let functional = agent.is_callable(&replace_value);
    if !functional {
        replace_value = Value::String(agent.to_string(&replace_value)?);
    }
    let flags = flags_of(agent, rx)?;
    let global = has_unit(&flags, 'g');
    let mut full_unicode = false;
    if global {
        full_unicode = has_unit(&flags, 'u') || has_unit(&flags, 'v');
        agent.set_prop(rx, PropertyKey::from("lastIndex"), Value::Number(0.0))?;
    }
    let mut results = Vec::new();
    loop {
        let result = regexp_exec(agent, rx, &s)?;
        if result.is_null() {
            break;
        }
        // Los resultados quedan enraizados mientras se siguen ejecutando callbacks.
        results.push(result.clone());
        if !global {
            break;
        }
        let m = match_string(agent, &result)?;
        if m.is_empty() {
            bump_last_index(agent, rx, &s, full_unicode)?;
        }
    }
    let scope = agent.heap.open_scope();
    for r in &results {
        agent.heap.root(r.clone());
    }
    let out = replace_results(agent, &s, length_s, &results, functional, &replace_value);
    agent.heap.close_scope(scope);
    out
}

fn replace_results(
    agent: &mut Agent,
    s: &JsString,
    length_s: usize,
    results: &[Value],
    functional: bool,
    replace_value: &Value,
) -> JsResult<Value> {
    let mut acc: Vec<u16> = Vec::new();
    let mut next_source = 0usize;
    for result in results {
        let Value::Object(ro) = result else {
            unreachable!("resultado de exec")
        };
        let ro = *ro;
        let result_length = agent.length_of_array_like(ro)?;
        let n_captures = (result_length - 1.0).max(0.0);
        let matched = match_string(agent, result)?;
        let match_length = matched.len();
        let pos = agent.get_str(ro, "index")?;
        let position = agent.to_integer_or_infinity(&pos)?;
        let position = position.clamp(0.0, length_s as f64) as usize;
        let mut captures: Vec<Value> = Vec::new();
        let mut n = 1.0;
        while n <= n_captures {
            let cap = agent.get(ro, &PropertyKey::from_number(n), result.clone())?;
            let cap = if cap.is_undefined() {
                cap
            } else {
                Value::String(agent.to_string(&cap)?)
            };
            captures.push(cap);
            n += 1.0;
        }
        let named = agent.get_str(ro, "groups")?;
        let replacement = if functional {
            let mut args = vec![Value::String(matched.clone())];
            args.extend(captures.iter().cloned());
            args.push(Value::Number(position as f64));
            args.push(Value::String(s.clone()));
            if !named.is_undefined() {
                args.push(named);
            }
            let v = agent.call(replace_value, Value::Undefined, &args)?;
            agent.to_string(&v)?
        } else {
            let named = if named.is_undefined() {
                None
            } else {
                Some(agent.to_object(&named)?)
            };
            let Value::String(template) = replace_value else {
                unreachable!("replaceValue string")
            };
            get_substitution(agent, &matched, s, position, &captures, named, template)?
        };
        if position >= next_source {
            acc.extend_from_slice(&s.units()[next_source..position]);
            acc.extend_from_slice(replacement.units());
            next_source = position + match_length;
        }
    }
    if next_source < length_s {
        acc.extend_from_slice(&s.units()[next_source..]);
    }
    Ok(Value::String(JsString::from_units(acc)))
}

/// GetSubstitution ( matched, str, position, captures, namedCaptures, replacementTemplate )
/// (§22.1.3.19.1). `captures` son Strings o undefined.
pub fn get_substitution(
    agent: &mut Agent,
    matched: &JsString,
    s: &JsString,
    position: usize,
    captures: &[Value],
    named: Option<Gc<Object>>,
    template: &JsString,
) -> JsResult<JsString> {
    let str_units = s.units();
    let string_length = str_units.len();
    let t = template.units();
    let mut result: Vec<u16> = Vec::new();
    let mut i = 0;
    let dollar = u16::from(b'$');
    let is_digit = |u: u16| (u16::from(b'0')..=u16::from(b'9')).contains(&u);
    while i < t.len() {
        let c = t[i];
        if c != dollar || i + 1 >= t.len() {
            result.push(c);
            i += 1;
            continue;
        }
        let n = t[i + 1];
        match n {
            0x24 => {
                result.push(dollar);
                i += 2;
            }
            0x60 => {
                // $`
                result.extend_from_slice(&str_units[..position.min(string_length)]);
                i += 2;
            }
            0x26 => {
                // $&
                result.extend_from_slice(matched.units());
                i += 2;
            }
            0x27 => {
                // $'
                let tail = (position + matched.len()).min(string_length);
                result.extend_from_slice(&str_units[tail..]);
                i += 2;
            }
            _ if is_digit(n) => {
                let two = i + 2 < t.len() && is_digit(t[i + 2]);
                let mut digit_count = if two { 2 } else { 1 };
                let d1 = usize::from(n - u16::from(b'0'));
                let mut index = if two {
                    d1 * 10 + usize::from(t[i + 2] - u16::from(b'0'))
                } else {
                    d1
                };
                let capture_len = captures.len();
                if index > capture_len && digit_count == 2 {
                    digit_count = 1;
                    index = d1;
                }
                let ref_len = 1 + digit_count;
                if (1..=capture_len).contains(&index) {
                    if let Value::String(cs) = &captures[index - 1] {
                        result.extend_from_slice(cs.units());
                    }
                } else {
                    result.extend_from_slice(&t[i..i + ref_len]);
                }
                i += ref_len;
            }
            0x3C => {
                // $<
                let gt = t[i..].iter().position(|&u| u == u16::from(b'>'));
                match (gt, named) {
                    (Some(gt), Some(nc)) => {
                        let name = JsString::from_units(t[i + 2..i + gt].to_vec());
                        let key = PropertyKey::from_js_string(name);
                        let cap = agent.get(nc, &key, Value::Object(nc))?;
                        if !cap.is_undefined() {
                            let cs = agent.to_string(&cap)?;
                            result.extend_from_slice(cs.units());
                        }
                        i += gt + 1;
                    }
                    _ => {
                        result.extend_from_slice(&t[i..i + 2]);
                        i += 2;
                    }
                }
            }
            _ => {
                result.push(c);
                i += 1;
            }
        }
    }
    Ok(JsString::from_units(result))
}

/// RegExp.prototype [ @@search ] ( string ) (§22.2.6.12).
fn symbol_search(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let rx = this_object(agent, &a.this, "[Symbol.search]")?;
    let s = agent.to_string(&a.arg(0))?;
    let previous = agent.get_str(rx, "lastIndex")?;
    let zero = Value::Number(0.0);
    if !agent.same_value(&previous, &zero) {
        agent.set_prop(rx, PropertyKey::from("lastIndex"), zero)?;
    }
    let result = regexp_exec(agent, rx, &s)?;
    let current = agent.get_str(rx, "lastIndex")?;
    if !agent.same_value(&current, &previous) {
        agent.set_prop(rx, PropertyKey::from("lastIndex"), previous)?;
    }
    match result {
        Value::Object(ro) => agent.get_str(ro, "index"),
        _ => Ok(Value::Number(-1.0)),
    }
}

/// RegExp.prototype [ @@split ] ( string, limit ) (§22.2.6.14).
fn symbol_split(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let rx = this_object(agent, &a.this, "[Symbol.split]")?;
    let s = agent.to_string(&a.arg(0))?;
    let default = agent.intrinsics().regexp_ctor;
    let c = agent.species_constructor(rx, default)?;
    let flags = flags_of(agent, rx)?;
    let unicode_matching = has_unit(&flags, 'u') || has_unit(&flags, 'v');
    let new_flags = if has_unit(&flags, 'y') {
        flags
    } else {
        let mut u = flags.units().to_vec();
        u.push(u16::from(b'y'));
        JsString::from_units(u)
    };
    let splitter = agent.construct(c, &[Value::Object(rx), Value::String(new_flags)], None)?;
    let Value::Object(splitter) = splitter else {
        return agent.type_error("RegExp species constructor returned a non-object");
    };
    let arr = agent.array_create(0, None)?;
    let mut length_a: u32 = 0;
    let limit = a.arg(1);
    let lim = if limit.is_undefined() {
        u32::MAX
    } else {
        agent.to_uint32(&limit)?
    };
    if lim == 0 {
        return Ok(Value::Object(arr));
    }
    let units = s.units();
    let size = units.len();
    if size == 0 {
        let z = regexp_exec(agent, splitter, &s)?;
        if !z.is_null() {
            return Ok(Value::Object(arr));
        }
        agent.create_data_property_or_throw(arr, PropertyKey::Index(0), Value::String(s))?;
        return Ok(Value::Object(arr));
    }
    let mut p = 0usize;
    let mut q = p;
    while q < size {
        agent.set_prop(
            splitter,
            PropertyKey::from("lastIndex"),
            Value::Number(q as f64),
        )?;
        let z = regexp_exec(agent, splitter, &s)?;
        let Value::Object(zo) = z else {
            q = advance_index(&s, q as f64, unicode_matching) as usize;
            continue;
        };
        let li = agent.get_str(splitter, "lastIndex")?;
        let e = agent.to_length(&li)?.min(size as f64) as usize;
        if e == p {
            q = advance_index(&s, q as f64, unicode_matching) as usize;
            continue;
        }
        let t = JsString::from_units(units[p..q.max(p)].to_vec());
        agent.create_data_property_or_throw(arr, PropertyKey::from(length_a), Value::String(t))?;
        length_a += 1;
        if length_a == lim {
            return Ok(Value::Object(arr));
        }
        p = e;
        let ncap = agent.length_of_array_like(zo)?;
        let ncap = (ncap - 1.0).max(0.0);
        let mut i = 1.0;
        while i <= ncap {
            let cap = agent.get(zo, &PropertyKey::from_number(i), Value::Object(zo))?;
            agent.create_data_property_or_throw(arr, PropertyKey::from(length_a), cap)?;
            i += 1.0;
            length_a += 1;
            if length_a == lim {
                return Ok(Value::Object(arr));
            }
        }
        q = p;
    }
    let t = JsString::from_units(units[p.min(size)..].to_vec());
    agent.create_data_property_or_throw(arr, PropertyKey::from(length_a), Value::String(t))?;
    Ok(Value::Object(arr))
}

// --- RegExp.escape ---

/// RegExp.escape ( S ) (§22.2.5.1).
fn escape(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Value::String(s) = a.arg(0) else {
        return agent.type_error("RegExp.escape requires a string");
    };
    let cps = crate::regexp::parser::decode_code_points(s.units());
    let mut out: Vec<u16> = Vec::new();
    for cp in cps {
        let ascii_alnum = char::from_u32(cp).is_some_and(|c| c.is_ascii_alphanumeric());
        if out.is_empty() && ascii_alnum {
            out.extend(format!("\\x{cp:02x}").encode_utf16());
        } else {
            encode_for_regexp_escape(cp, &mut out);
        }
    }
    Ok(Value::String(JsString::from_units(out)))
}

/// EncodeForRegExpEscape ( cp ) (§22.2.5.1.1).
fn encode_for_regexp_escape(cp: u32, out: &mut Vec<u16>) {
    let chr = char::from_u32(cp);
    if let Some(c) = chr
        && ("^$\\.*+?()[]{}|/".contains(c))
    {
        out.push(u16::from(b'\\'));
        out.push(cp as u16);
        return;
    }
    let control = match cp {
        0x09 => Some('t'),
        0x0A => Some('n'),
        0x0B => Some('v'),
        0x0C => Some('f'),
        0x0D => Some('r'),
        _ => None,
    };
    if let Some(c) = control {
        out.push(u16::from(b'\\'));
        out.push(c as u16);
        return;
    }
    let other = ",-=<>#&!%:;@~'`\"";
    // WhiteSpace o LineTerminator.
    let is_ws = crate::regexp::unicode::white_space().contains(cp);
    let surrogate = (0xD800..0xE000).contains(&cp);
    if chr.is_some_and(|c| other.contains(c)) || is_ws || surrogate {
        if cp <= 0xFF {
            out.extend(format!("\\x{cp:02x}").encode_utf16());
            return;
        }
        let mut buf = [0u16; 2];
        let units: &[u16] = match chr {
            Some(c) => c.encode_utf16(&mut buf),
            None => {
                buf[0] = cp as u16;
                &buf[..1]
            }
        };
        for &u in units {
            out.extend(format!("\\u{u:04x}").encode_utf16());
        }
        return;
    }
    match chr {
        Some(c) => {
            let mut buf = [0u16; 2];
            out.extend_from_slice(c.encode_utf16(&mut buf));
        }
        None => out.push(cp as u16),
    }
}

// --- RegExp Legacy Features ---
//
// El estado ([[RegExpInput]], [[RegExpLastMatch]], …) vive en los slots de %RegExp%:
// 0 = input (Null si está invalidado), 1 = el string de los matches, 2–3 = lastMatch,
// 4–5 = lastParen, 6–23 = $1…$9 (pares inicio/fin; NaN = string vacío).

const LEGACY_SLOTS: usize = 24;

fn init_legacy_state(agent: &mut Agent, ctor: Gc<Object>) {
    let mut slots = vec![Value::Number(f64::NAN); LEGACY_SLOTS];
    slots[0] = Value::String(JsString::default());
    slots[1] = Value::String(JsString::default());
    if let ObjectKind::Builtin(b) = &mut agent.heap[ctor].kind {
        b.slots = slots;
    }
}

fn set_pair(slots: &mut [Value], at: usize, cap: Option<(usize, usize)>) {
    let (s, e) = match cap {
        Some((s, e)) => (s as f64, e as f64),
        None => (f64::NAN, f64::NAN),
    };
    slots[at] = Value::Number(s);
    slots[at + 1] = Value::Number(e);
}

/// UpdateLegacyRegExpStaticProperties ( C, S, startIndex, endIndex, capturedValues ).
fn update_legacy(
    agent: &mut Agent,
    ctor: Gc<Object>,
    s: &JsString,
    start: usize,
    end: usize,
    captures: &[Option<(usize, usize)>],
) {
    let ObjectKind::Builtin(b) = &mut agent.heap[ctor].kind else {
        return;
    };
    let slots = &mut b.slots;
    if slots.len() < LEGACY_SLOTS {
        return;
    }
    slots[0] = Value::String(s.clone());
    slots[1] = Value::String(s.clone());
    set_pair(slots, 2, Some((start, end)));
    set_pair(slots, 4, captures.last().copied().flatten());
    for i in 0..9 {
        set_pair(slots, 6 + 2 * i, captures.get(i).copied().flatten());
    }
}

/// InvalidateLegacyRegExpStaticProperties ( C ).
fn invalidate_legacy(agent: &mut Agent, ctor: Gc<Object>) {
    if let ObjectKind::Builtin(b) = &mut agent.heap[ctor].kind
        && b.slots.len() >= LEGACY_SLOTS
    {
        b.slots[0] = Value::Null;
    }
}

/// GetLegacyRegExpStaticProperty: `which` es el slot (0 = input, 2 = lastMatch, 4 =
/// lastParen, 6… = $n, 100 = leftContext, 101 = rightContext).
fn legacy_get(agent: &mut Agent, this: &Value, which: usize) -> JsResult<Value> {
    let ctor = agent.intrinsics().regexp_ctor;
    if !matches!(this, Value::Object(o) if *o == ctor) {
        return agent.type_error("RegExp legacy static property accessed on a non-RegExp receiver");
    }
    let slots = crate::builtins::slots(agent, ctor);
    if slots.len() < LEGACY_SLOTS || slots[0].is_null() {
        return agent.type_error("RegExp legacy static properties are not available");
    }
    if which == 0 {
        return Ok(slots[0].clone());
    }
    let Value::String(s) = &slots[1] else {
        return Ok(Value::str(""));
    };
    let u = s.units();
    let num = |i: usize| slots[i].as_number().unwrap_or(f64::NAN);
    let sub = |a: f64, b: f64| -> Value {
        if a.is_nan() || b.is_nan() {
            return Value::str("");
        }
        Value::String(JsString::from_units(u[a as usize..b as usize].to_vec()))
    };
    Ok(match which {
        100 => sub(0.0, num(2)),
        101 => sub(num(3), u.len() as f64),
        k => sub(num(k), num(k + 1)),
    })
}

fn legacy_set_input(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let ctor = agent.intrinsics().regexp_ctor;
    if !matches!(a.this, Value::Object(o) if o == ctor) {
        return agent.type_error("RegExp legacy static property set on a non-RegExp receiver");
    }
    let s = agent.to_string(&a.arg(0))?;
    if let ObjectKind::Builtin(b) = &mut agent.heap[ctor].kind
        && b.slots.len() >= LEGACY_SLOTS
    {
        b.slots[0] = Value::String(s);
    }
    Ok(Value::Undefined)
}

macro_rules! legacy_getters {
    ($($name:ident = $slot:expr),* $(,)?) => {
        $(fn $name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
            legacy_get(agent, &a.this, $slot)
        })*
    };
}

legacy_getters!(
    legacy_input = 0,
    legacy_last_match = 2,
    legacy_last_paren = 4,
    legacy_left_context = 100,
    legacy_right_context = 101,
    legacy_p1 = 6,
    legacy_p2 = 8,
    legacy_p3 = 10,
    legacy_p4 = 12,
    legacy_p5 = 14,
    legacy_p6 = 16,
    legacy_p7 = 18,
    legacy_p8 = 20,
    legacy_p9 = 22,
);

fn init_legacy_accessors(agent: &mut Agent, ctor: Gc<Object>) {
    type G = crate::vm::function::NativeFn;
    let pairs: &[(&str, &str, G, bool)] = &[
        ("input", "$_", legacy_input, true),
        ("lastMatch", "$&", legacy_last_match, false),
        ("lastParen", "$+", legacy_last_paren, false),
        ("leftContext", "$`", legacy_left_context, false),
        ("rightContext", "$'", legacy_right_context, false),
    ];
    for &(long, short, g, settable) in pairs {
        let set: Option<G> = if settable {
            Some(legacy_set_input)
        } else {
            None
        };
        accessor(agent, ctor, PropertyKey::from(long), long, Some(g), set);
        accessor(agent, ctor, PropertyKey::from(short), short, Some(g), set);
    }
    let parens: [G; 9] = [
        legacy_p1, legacy_p2, legacy_p3, legacy_p4, legacy_p5, legacy_p6, legacy_p7, legacy_p8,
        legacy_p9,
    ];
    for (i, g) in parens.into_iter().enumerate() {
        let name = format!("${}", i + 1);
        accessor(
            agent,
            ctor,
            PropertyKey::from(name.as_str()),
            &name,
            Some(g),
            None,
        );
    }
}

/// IsRegExp también reconoce `[[RegExpMatcher]]` (§7.2.8 paso 4).
pub fn has_regexp_matcher(agent: &Agent, o: Gc<Object>) -> bool {
    regexp_data(agent, o).is_some()
}
