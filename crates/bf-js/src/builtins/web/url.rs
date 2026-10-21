//! `URL` y `URLSearchParams` (URL Standard §6.1 y §6.2) sobre el parser de `bf-url`.

use bf_url::Url;

use crate::builtins::web::{
    WebData, attr, create, create_from, data_of, data_of_mut, interface, op, require_args,
    require_new, str_value, usv_string,
};
use crate::heap::Gc;
use crate::object::{IterKind, Object, ObjectKind};
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

/// Los slots de un objeto URL: el URL record y su query object.
#[derive(Debug)]
pub struct UrlData {
    pub url: Url,
    pub query: Option<Gc<Object>>,
}

/// Los slots de un URLSearchParams: la lista de pares y el URL object asociado.
#[derive(Debug)]
pub struct SearchParamsData {
    pub list: Vec<(String, String)>,
    pub url: Option<Gc<Object>>,
}

/// Un iterador de URLSearchParams (WebIDL §3.7.10.1, default iterator objects).
#[derive(Debug)]
pub struct SearchParamsIter {
    pub params: Gc<Object>,
    pub kind: IterKind,
    pub index: usize,
}

pub fn init(agent: &mut Agent, realm: RealmId) {
    let (c, p) = interface(agent, realm, "URL", "URL.prototype", 1, url_ctor, None);
    attr(agent, p, "href", get_href, Some(set_href));
    attr(agent, p, "origin", get_origin, None);
    attr(agent, p, "protocol", get_protocol, Some(set_protocol));
    attr(agent, p, "username", get_username, Some(set_username));
    attr(agent, p, "password", get_password, Some(set_password));
    attr(agent, p, "host", get_host, Some(set_host));
    attr(agent, p, "hostname", get_hostname, Some(set_hostname));
    attr(agent, p, "port", get_port, Some(set_port));
    attr(agent, p, "pathname", get_pathname, Some(set_pathname));
    attr(agent, p, "search", get_search, Some(set_search));
    attr(agent, p, "searchParams", get_search_params, None);
    attr(agent, p, "hash", get_hash, Some(set_hash));
    op(agent, p, "toJSON", 0, to_json);
    // El stringifier (WebIDL §3.7.7.2): `toString` devuelve `href`.
    op(agent, p, "toString", 0, to_json);
    op(agent, c, "canParse", 1, can_parse);
    op(agent, c, "parse", 1, parse);

    let (_, sp) = interface(
        agent,
        realm,
        "URLSearchParams",
        "URLSearchParams.prototype",
        0,
        params_ctor,
        None,
    );
    op(agent, sp, "append", 2, append);
    op(agent, sp, "delete", 1, delete);
    op(agent, sp, "get", 1, get);
    op(agent, sp, "getAll", 1, get_all);
    op(agent, sp, "has", 1, has);
    op(agent, sp, "set", 2, set);
    op(agent, sp, "sort", 0, sort);
    op(agent, sp, "toString", 0, params_to_string);
    attr(agent, sp, "size", get_size, None);
    op(agent, sp, "forEach", 1, for_each);
    op(agent, sp, "keys", 0, keys);
    op(agent, sp, "values", 0, values);
    let entries_fn = agent.create_builtin_function(entries, 0, "entries", false);
    agent.put_data(
        sp,
        PropertyKey::from("entries"),
        Value::Object(entries_fn),
        true,
        true,
        true,
    );
    let it = PropertyKey::Symbol(agent.well_known.iterator);
    agent.put_data(sp, it, Value::Object(entries_fn), true, false, true);
    // %URLSearchParamsIteratorPrototype% (WebIDL §3.7.10.2).
    let ip = agent.realms[realm].intrinsics.iterator_prototype;
    let iter_proto = agent.new_object(Some(ip));
    op(agent, iter_proto, "next", 0, iterator_next);
    crate::builtins::to_string_tag(agent, iter_proto, "URLSearchParams Iterator");
    crate::builtins::web::registry_set(agent, realm, "URLSearchParams Iterator", iter_proto);
}

// --- URL ---

/// El API URL parser (§6.1, "API URL parser"): `None` si falla `url` o `base`.
fn api_parse(url: &str, base: Option<&str>) -> Option<Url> {
    let base = match base {
        Some(b) => Some(Url::parse(b, None).ok()?),
        None => None,
    };
    Url::parse(url, base.as_ref()).ok()
}

fn url_and_base(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<(String, Option<String>)> {
    let url = usv_string(agent, &a.arg(0))?;
    let base = if a.arg(1).is_undefined() {
        None
    } else {
        Some(usv_string(agent, &a.arg(1))?)
    };
    Ok((url, base))
}

/// `new URL(url, base)`.
fn url_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let nt = require_new(agent, a, "URL")?;
    require_args(agent, a, 1, "Failed to construct 'URL'")?;
    let (url, base) = url_and_base(agent, a)?;
    let Some(parsed) = api_parse(&url, base.as_deref()) else {
        return agent.type_error(&format!("Failed to construct 'URL': Invalid URL '{url}'"));
    };
    let o = create_from(
        agent,
        nt,
        "URL.prototype",
        WebData::Url(Box::new(UrlData {
            url: parsed,
            query: None,
        })),
    )?;
    initialize(agent, o);
    Ok(Value::Object(o))
}

/// Initialize (§6.1): el query object con la lista de la query.
fn initialize(agent: &mut Agent, o: Gc<Object>) {
    let list = match data_of(agent, &Value::Object(o)) {
        Some(WebData::Url(u)) => parse_query(u.url.query.as_deref()),
        _ => Vec::new(),
    };
    let q = create(
        agent,
        "URLSearchParams.prototype",
        WebData::SearchParams(Box::new(SearchParamsData { list, url: Some(o) })),
    );
    if let Some(WebData::Url(u)) = data_of_mut(agent, o) {
        u.query = Some(q);
    }
}

fn parse_query(q: Option<&str>) -> Vec<(String, String)> {
    match q {
        Some(q) => bf_url::form::parse(q.as_bytes()),
        None => Vec::new(),
    }
}

fn this_url(agent: &mut Agent, this: &Value) -> JsResult<Gc<Object>> {
    match (this, data_of(agent, this)) {
        (Value::Object(o), Some(WebData::Url(_))) => Ok(*o),
        _ => agent.type_error("Illegal invocation"),
    }
}

fn with_url<T>(agent: &mut Agent, this: &Value, f: impl FnOnce(&Url) -> T) -> JsResult<T> {
    let o = this_url(agent, this)?;
    match data_of(agent, &Value::Object(o)) {
        Some(WebData::Url(u)) => Ok(f(&u.url)),
        _ => unreachable!("URL"),
    }
}

fn mutate_url(agent: &mut Agent, o: Gc<Object>, f: impl FnOnce(&mut Url)) {
    if let Some(WebData::Url(u)) = data_of_mut(agent, o) {
        f(&mut u.url);
    }
}

/// Después de cambiar la query de la URL: la lista del query object.
fn refresh_query_list(agent: &mut Agent, o: Gc<Object>) {
    let (q, list) = match data_of(agent, &Value::Object(o)) {
        Some(WebData::Url(u)) => (u.query, parse_query(u.url.query.as_deref())),
        _ => return,
    };
    if let Some(q) = q
        && let Some(WebData::SearchParams(p)) = data_of_mut(agent, q)
    {
        p.list = list;
    }
}

macro_rules! url_getter {
    ($name:ident, $body:expr) => {
        fn $name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
            let f: fn(&Url) -> String = $body;
            let s = with_url(agent, &a.this, f)?;
            Ok(str_value(&s))
        }
    };
}

url_getter!(get_href, |u| u.href());
url_getter!(get_origin, |u| u.origin().to_string());
url_getter!(get_protocol, |u| u.protocol());
url_getter!(get_username, |u| u.username.clone());
url_getter!(get_password, |u| u.password.clone());
url_getter!(get_host, |u| u.host_string());
url_getter!(get_hostname, |u| u.hostname());
url_getter!(get_port, |u| u.port_string());
url_getter!(get_pathname, |u| u.path_string());
url_getter!(get_search, |u| u.search());
url_getter!(get_hash, |u| u.hash());

fn to_json(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    get_href(agent, a)
}

macro_rules! url_setter {
    ($name:ident, $method:ident) => {
        fn $name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
            let o = this_url(agent, &a.this)?;
            let v = usv_string(agent, &a.arg(0))?;
            mutate_url(agent, o, |u| u.$method(&v));
            Ok(Value::Undefined)
        }
    };
}

url_setter!(set_protocol, set_protocol);
url_setter!(set_username, set_username);
url_setter!(set_password, set_password);
url_setter!(set_host, set_host);
url_setter!(set_hostname, set_hostname);
url_setter!(set_port, set_port);
url_setter!(set_pathname, set_pathname);
url_setter!(set_hash, set_hash);

/// El setter de `href`: un fallo es un TypeError; la lista de la query se rehace.
fn set_href(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_url(agent, &a.this)?;
    let v = usv_string(agent, &a.arg(0))?;
    let Ok(parsed) = Url::parse(&v, None) else {
        return agent.type_error(&format!(
            "Failed to set the 'href' property on 'URL': Invalid URL '{v}'"
        ));
    };
    mutate_url(agent, o, |u| *u = parsed);
    refresh_query_list(agent, o);
    Ok(Value::Undefined)
}

/// El setter de `search`: también rehace la lista del query object.
fn set_search(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_url(agent, &a.this)?;
    let v = usv_string(agent, &a.arg(0))?;
    mutate_url(agent, o, |u| u.set_search(&v));
    refresh_query_list(agent, o);
    Ok(Value::Undefined)
}

fn get_search_params(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_url(agent, &a.this)?;
    match data_of(agent, &Value::Object(o)) {
        Some(WebData::Url(u)) => Ok(Value::from(u.query)),
        _ => Ok(Value::Undefined),
    }
}

/// `URL.canParse(url, base)`.
fn can_parse(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    require_args(agent, a, 1, "Failed to execute 'canParse' on 'URL'")?;
    let (url, base) = url_and_base(agent, a)?;
    Ok(Value::Boolean(api_parse(&url, base.as_deref()).is_some()))
}

/// `URL.parse(url, base)`: el objeto URL o null.
fn parse(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    require_args(agent, a, 1, "Failed to execute 'parse' on 'URL'")?;
    let (url, base) = url_and_base(agent, a)?;
    let Some(parsed) = api_parse(&url, base.as_deref()) else {
        return Ok(Value::Null);
    };
    let o = create(
        agent,
        "URL.prototype",
        WebData::Url(Box::new(UrlData {
            url: parsed,
            query: None,
        })),
    );
    initialize(agent, o);
    Ok(Value::Object(o))
}

// --- URLSearchParams ---

/// `new URLSearchParams(init)`: secuencia de pares, record o string (WebIDL, conversión de la
/// unión `(sequence<sequence<USVString>> or record<USVString, USVString> or USVString)`).
fn params_ctor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let nt = require_new(agent, a, "URLSearchParams")?;
    let init = a.arg(0);
    let list = match &init {
        Value::Undefined => Vec::new(),
        Value::Object(o) => {
            let it = PropertyKey::Symbol(agent.well_known.iterator);
            match agent.get_method_of(&init, &it)? {
                Some(method) => pairs_from_sequence(agent, &init, &method)?,
                None => pairs_from_record(agent, *o)?,
            }
        }
        v => {
            let s = usv_string(agent, v)?;
            let s = s.strip_prefix('?').unwrap_or(&s);
            bf_url::form::parse(s.as_bytes())
        }
    };
    let o = create_from(
        agent,
        nt,
        "URLSearchParams.prototype",
        WebData::SearchParams(Box::new(SearchParamsData { list, url: None })),
    )?;
    Ok(Value::Object(o))
}

fn pairs_from_sequence(
    agent: &mut Agent,
    init: &Value,
    method: &Value,
) -> JsResult<Vec<(String, String)>> {
    let (it, next) = agent.get_iterator_from_method(init, method)?;
    let mut out = Vec::new();
    while let Some(pair) = agent.iterator_step_value(&it, &next)? {
        let r = pair_of(agent, &pair);
        match r {
            Ok(p) => out.push(p),
            Err(e) => {
                agent.iterator_close_on_throw(&it);
                return Err(e);
            }
        }
    }
    Ok(out)
}

/// Un elemento de la secuencia: una `sequence<USVString>` de exactamente dos elementos.
fn pair_of(agent: &mut Agent, v: &Value) -> JsResult<(String, String)> {
    if !v.is_object() {
        return agent.type_error(
            "Failed to construct 'URLSearchParams': The provided value cannot be converted to a sequence",
        );
    }
    let it_key = PropertyKey::Symbol(agent.well_known.iterator);
    let Some(m) = agent.get_method_of(v, &it_key)? else {
        return agent.type_error(
            "Failed to construct 'URLSearchParams': The object must have a callable @@iterator property",
        );
    };
    let (it, next) = agent.get_iterator_from_method(v, &m)?;
    let mut items = Vec::new();
    while let Some(x) = agent.iterator_step_value(&it, &next)? {
        items.push(usv_string(agent, &x)?);
    }
    if items.len() != 2 {
        return agent.type_error(
            "Failed to construct 'URLSearchParams': Sequence initializer must only contain pair elements",
        );
    }
    let b = items.pop().unwrap_or_default();
    let a = items.pop().unwrap_or_default();
    Ok((a, b))
}

/// La conversión a `record<USVString, USVString>` (WebIDL §3.2.25).
fn pairs_from_record(agent: &mut Agent, o: Gc<Object>) -> JsResult<Vec<(String, String)>> {
    let keys = agent.own_property_keys(o)?;
    let mut out: Vec<(String, String)> = Vec::new();
    for k in keys {
        let Some(desc) = agent.get_own_property(o, &k)? else {
            continue;
        };
        if desc.enumerable != Some(true) {
            continue;
        }
        let key = usv_string(agent, &k.to_value())?;
        let v = agent.get(o, &k, Value::Object(o))?;
        let value = usv_string(agent, &v)?;
        match out.iter_mut().find(|(n, _)| *n == key) {
            Some(e) => e.1 = value,
            None => out.push((key, value)),
        }
    }
    Ok(out)
}

fn this_params(agent: &mut Agent, this: &Value) -> JsResult<Gc<Object>> {
    match (this, data_of(agent, this)) {
        (Value::Object(o), Some(WebData::SearchParams(_))) => Ok(*o),
        _ => agent.type_error("Illegal invocation"),
    }
}

fn list_of(agent: &Agent, o: Gc<Object>) -> &[(String, String)] {
    match data_of(agent, &Value::Object(o)) {
        Some(WebData::SearchParams(p)) => &p.list,
        _ => &[],
    }
}

/// Cambia la lista y corre los "update steps" (§6.2).
fn update(agent: &mut Agent, o: Gc<Object>, f: impl FnOnce(&mut Vec<(String, String)>)) {
    let (url, serialized) = match data_of_mut(agent, o) {
        Some(WebData::SearchParams(p)) => {
            f(&mut p.list);
            (p.url, bf_url::form::serialize(&p.list))
        }
        _ => return,
    };
    let Some(u) = url else { return };
    mutate_url(agent, u, |url| {
        if serialized.is_empty() {
            url.query = None;
            // Potentially strip trailing spaces from an opaque path.
            if url.fragment.is_none()
                && let bf_url::Path::Opaque(p) = &mut url.path
            {
                let n = p.trim_end_matches(' ').len();
                p.truncate(n);
            }
        } else {
            url.query = Some(serialized);
        }
    });
}

fn name_value(
    agent: &mut Agent,
    a: &CallArgs<'_>,
    what: &str,
    n: usize,
) -> JsResult<(String, String)> {
    require_args(agent, a, n, what)?;
    let name = usv_string(agent, &a.arg(0))?;
    let value = if n == 2 {
        usv_string(agent, &a.arg(1))?
    } else {
        String::new()
    };
    Ok((name, value))
}

fn append(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_params(agent, &a.this)?;
    let (n, v) = name_value(
        agent,
        a,
        "Failed to execute 'append' on 'URLSearchParams'",
        2,
    )?;
    update(agent, o, |l| l.push((n, v)));
    Ok(Value::Undefined)
}

/// El `value` opcional de `delete`/`has` (undefined cuenta como ausente).
fn optional_value(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Option<String>> {
    let v = a.arg(1);
    if v.is_undefined() {
        Ok(None)
    } else {
        Ok(Some(usv_string(agent, &v)?))
    }
}

fn delete(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_params(agent, &a.this)?;
    let (n, _) = name_value(
        agent,
        a,
        "Failed to execute 'delete' on 'URLSearchParams'",
        1,
    )?;
    let value = optional_value(agent, a)?;
    update(agent, o, |l| {
        l.retain(|(k, v)| !(*k == n && value.as_ref().is_none_or(|x| x == v)))
    });
    Ok(Value::Undefined)
}

fn get(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_params(agent, &a.this)?;
    let (n, _) = name_value(agent, a, "Failed to execute 'get' on 'URLSearchParams'", 1)?;
    Ok(list_of(agent, o)
        .iter()
        .find(|(k, _)| *k == n)
        .map_or(Value::Null, |(_, v)| str_value(v)))
}

fn get_all(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_params(agent, &a.this)?;
    let (n, _) = name_value(
        agent,
        a,
        "Failed to execute 'getAll' on 'URLSearchParams'",
        1,
    )?;
    let vals: Vec<Value> = list_of(agent, o)
        .iter()
        .filter(|(k, _)| *k == n)
        .map(|(_, v)| str_value(v))
        .collect();
    Ok(Value::Object(agent.create_array_from_list(&vals)?))
}

fn has(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_params(agent, &a.this)?;
    let (n, _) = name_value(agent, a, "Failed to execute 'has' on 'URLSearchParams'", 1)?;
    let value = optional_value(agent, a)?;
    Ok(Value::Boolean(list_of(agent, o).iter().any(|(k, v)| {
        *k == n && value.as_ref().is_none_or(|x| x == v)
    })))
}

fn set(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_params(agent, &a.this)?;
    let (n, v) = name_value(agent, a, "Failed to execute 'set' on 'URLSearchParams'", 2)?;
    update(agent, o, |l| match l.iter().position(|(k, _)| *k == n) {
        Some(i) => {
            l[i].1 = v;
            let mut j = 0;
            l.retain(|(k, _)| {
                let keep = j <= i || *k != n;
                j += 1;
                keep
            });
        }
        None => l.push((n, v)),
    });
    Ok(Value::Undefined)
}

/// `sort()`: orden estable por nombre en code units.
fn sort(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_params(agent, &a.this)?;
    update(agent, o, |l| {
        l.sort_by(|x, y| x.0.encode_utf16().cmp(y.0.encode_utf16()))
    });
    Ok(Value::Undefined)
}

fn params_to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_params(agent, &a.this)?;
    Ok(str_value(&bf_url::form::serialize(list_of(agent, o))))
}

fn get_size(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_params(agent, &a.this)?;
    Ok(Value::Number(list_of(agent, o).len() as f64))
}

/// `forEach(callback, thisArg)` (WebIDL §3.7.10, sobre la lista viva).
fn for_each(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_params(agent, &a.this)?;
    require_args(
        agent,
        a,
        1,
        "Failed to execute 'forEach' on 'URLSearchParams'",
    )?;
    let cb = a.arg(0);
    if !agent.is_callable(&cb) {
        return agent.type_error(
            "Failed to execute 'forEach' on 'URLSearchParams': The callback provided as parameter 1 is not a function.",
        );
    }
    let this_arg = a.arg(1);
    let mut i = 0;
    while let Some((k, v)) = list_of(agent, o).get(i).cloned() {
        agent.call(
            &cb,
            this_arg.clone(),
            &[str_value(&v), str_value(&k), Value::Object(o)],
        )?;
        i += 1;
    }
    Ok(Value::Undefined)
}

fn make_iterator(agent: &mut Agent, this: &Value, kind: IterKind) -> JsResult<Value> {
    let o = this_params(agent, this)?;
    let it = create(
        agent,
        "URLSearchParams Iterator",
        WebData::SearchParamsIterator(Box::new(SearchParamsIter {
            params: o,
            kind,
            index: 0,
        })),
    );
    Ok(Value::Object(it))
}

fn entries(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    make_iterator(agent, &a.this, IterKind::Entries)
}

fn keys(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    make_iterator(agent, &a.this, IterKind::Keys)
}

fn values(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    make_iterator(agent, &a.this, IterKind::Values)
}

fn iterator_next(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (params, kind, index) = match (&a.this, data_of(agent, &a.this)) {
        (Value::Object(_), Some(WebData::SearchParamsIterator(i))) => (i.params, i.kind, i.index),
        _ => return agent.type_error("Illegal invocation"),
    };
    let Some((k, v)) = list_of(agent, params).get(index).cloned() else {
        return agent.create_iter_result(Value::Undefined, true);
    };
    if let Value::Object(it) = a.this
        && let Some(WebData::SearchParamsIterator(i)) = data_of_mut(agent, it)
    {
        i.index += 1;
    }
    let result = match kind {
        IterKind::Keys => str_value(&k),
        IterKind::Values => str_value(&v),
        IterKind::Entries => {
            Value::Object(agent.create_array_from_list(&[str_value(&k), str_value(&v)])?)
        }
    };
    agent.create_iter_result(result, false)
}

/// Para otros módulos: ¿es un objeto URL?
pub fn is_url(agent: &Agent, o: Gc<Object>) -> bool {
    matches!(&agent.heap[o].kind, ObjectKind::Web(w) if matches!(**w, WebData::Url(_)))
}
