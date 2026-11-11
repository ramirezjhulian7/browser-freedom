//! Intl.Segmenter (ECMA-402 §18): grafemas, palabras y oraciones con las reglas por
//! defecto de UAX #29 ([`super::segment`]), %IntlSegmentsPrototype% (§18.5) y
//! %IntlSegmentIteratorPrototype% (§18.6).

use std::rc::Rc;

use crate::builtins::intl::segment::{self, Granularity};
use crate::builtins::intl::{
    IntlData, create_intl_object, get_string_option, intl_constructor, intl_data, locale,
};
use crate::builtins::web::{self, str_value};
use crate::heap::Gc;
use crate::object::Object;
use crate::string::JsString;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

const SEGMENTS_PROTO: &str = "Intl.Segments.prototype";
const ITERATOR_PROTO: &str = "Intl.SegmentIterator.prototype";

/// Los slots de un Intl.Segmenter (§18.4).
#[derive(Debug, Clone)]
pub struct Segmenter {
    pub locale: String,
    pub granularity: Granularity,
}

/// Un objeto Segments (§18.5): [[SegmentsSegmenter]] (su granularidad) y
/// [[SegmentsString]], con los límites ya calculados.
#[derive(Debug, Clone)]
pub struct Segments {
    pub granularity: Granularity,
    pub string: JsString,
    pub boundaries: Rc<Vec<usize>>,
}

/// Un Segment Iterator (§18.6): además, [[IteratedStringNextSegmentCodeUnitIndex]].
#[derive(Debug, Clone)]
pub struct SegmentIterator {
    pub segments: Segments,
    pub next: usize,
}

pub fn init(agent: &mut Agent, realm: RealmId, intl: Gc<Object>) {
    let (c, p) = intl_constructor(
        agent,
        realm,
        intl,
        "Segmenter",
        "Intl.Segmenter.prototype",
        0,
        constructor,
    );
    crate::builtins::method(agent, c, "supportedLocalesOf", 1, supported_locales_of);
    crate::builtins::method(agent, p, "resolvedOptions", 0, resolved_options);
    crate::builtins::method(agent, p, "segment", 1, segment);
    // %IntlSegmentsPrototype% (§18.5.2).
    let op = agent.realms[realm].intrinsics.object_prototype;
    let sp = agent.new_object(Some(op));
    crate::builtins::method(agent, sp, "containing", 1, containing);
    let it = agent.well_known.iterator;
    crate::builtins::method_sym(agent, sp, it, "[Symbol.iterator]", 0, segments_iterator);
    web::registry_set(agent, realm, SEGMENTS_PROTO, sp);
    // %IntlSegmentIteratorPrototype% (§18.6.2).
    let ip = agent.realms[realm].intrinsics.iterator_prototype;
    let sip = agent.new_object(Some(ip));
    crate::builtins::method(agent, sip, "next", 0, iterator_next);
    crate::builtins::to_string_tag(agent, sip, "Segmenter String Iterator");
    web::registry_set(agent, realm, ITERATOR_PROTO, sip);
}

/// Intl.Segmenter ( [ locales [ , options ] ] ) (§18.1.1).
fn constructor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor Intl.Segmenter requires 'new'");
    };
    let obj = create_intl_object(
        agent,
        nt,
        "Intl.Segmenter.prototype",
        IntlData::Segmenter(Box::new(Segmenter {
            locale: String::new(),
            granularity: Granularity::Grapheme,
        })),
    )?;
    let requested = locale::canonicalize_locale_list(agent, &a.arg(0))?;
    let options = super::list_format::get_options_object(agent, &a.arg(1))?;
    get_string_option(
        agent,
        options,
        "localeMatcher",
        &["lookup", "best fit"],
        Some("best fit"),
    )?;
    // ResolveLocale sin claves de extensión relevantes.
    let loc = requested
        .iter()
        .find_map(|l| available(l))
        .unwrap_or_else(|| locale::default_locale().to_owned());
    let g = get_string_option(
        agent,
        options,
        "granularity",
        &["grapheme", "word", "sentence"],
        Some("grapheme"),
    )?;
    let granularity = match g.as_deref() {
        Some("word") => Granularity::Word,
        Some("sentence") => Granularity::Sentence,
        _ => Granularity::Grapheme,
    };
    if let Some(IntlData::Segmenter(s)) = crate::builtins::intl::intl_data_mut(agent, obj) {
        **s = Segmenter {
            locale: loc,
            granularity,
        };
    }
    Ok(Value::Object(obj))
}

fn this_segmenter(agent: &mut Agent, this: &Value, method: &str) -> JsResult<Segmenter> {
    match intl_data(agent, this) {
        Some(IntlData::Segmenter(s)) => Ok((**s).clone()),
        _ => agent.type_error(&format!(
            "Method Intl.Segmenter.prototype.{method} called on incompatible receiver"
        )),
    }
}

/// Intl.Segmenter.prototype.resolvedOptions ( ) (§18.3.3).
fn resolved_options(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_segmenter(agent, &a.this, "resolvedOptions")?;
    let o = agent.new_plain_object();
    agent.create_data_property_or_throw(o, PropertyKey::from("locale"), str_value(&s.locale))?;
    agent.create_data_property_or_throw(
        o,
        PropertyKey::from("granularity"),
        str_value(s.granularity.as_str()),
    )?;
    Ok(Value::Object(o))
}

/// Intl.Segmenter.prototype.segment ( string ) (§18.3.4) y CreateSegmentsObject (§18.5.1).
fn segment(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_segmenter(agent, &a.this, "segment")?;
    let string = agent.to_string(&a.arg(0))?;
    let boundaries = Rc::new(segment::boundaries(string.units(), s.granularity));
    let o = web::create(
        agent,
        SEGMENTS_PROTO,
        web::WebData::Intl(Box::new(IntlData::Segments(Box::new(Segments {
            granularity: s.granularity,
            string,
            boundaries,
        })))),
    );
    Ok(Value::Object(o))
}

impl Segments {
    /// FindBoundary ( segmenter, string, startIndex, before ) (§18.8.1): el límite más
    /// cercano en o antes de `n`.
    fn boundary_before(&self, n: usize) -> usize {
        match self.boundaries.binary_search(&n) {
            Ok(i) => self.boundaries[i],
            Err(i) => self.boundaries[i - 1],
        }
    }

    /// FindBoundary ( …, after ): el primer límite después de `n`.
    fn boundary_after(&self, n: usize) -> usize {
        match self.boundaries.binary_search(&n) {
            Ok(i) => self.boundaries[i + 1],
            Err(i) => self.boundaries[i],
        }
    }

    /// CreateSegmentDataObject ( segmenter, string, startIndex, endIndex ) (§18.7.1).
    fn data_object(&self, agent: &mut Agent, start: usize, end: usize) -> JsResult<Value> {
        let o = agent.new_plain_object();
        let units = &self.string.units()[start..end];
        let seg = Value::String(JsString::from_units(units.to_vec()));
        agent.create_data_property_or_throw(o, PropertyKey::from("segment"), seg)?;
        agent.create_data_property_or_throw(
            o,
            PropertyKey::from("index"),
            Value::Number(start as f64),
        )?;
        agent.create_data_property_or_throw(
            o,
            PropertyKey::from("input"),
            Value::String(self.string.clone()),
        )?;
        if self.granularity == Granularity::Word {
            let word = segment::is_word_like(units);
            agent.create_data_property_or_throw(
                o,
                PropertyKey::from("isWordLike"),
                Value::Boolean(word),
            )?;
        }
        Ok(Value::Object(o))
    }
}

fn this_segments(agent: &mut Agent, this: &Value, method: &str) -> JsResult<Segments> {
    match intl_data(agent, this) {
        Some(IntlData::Segments(s)) => Ok((**s).clone()),
        _ => agent.type_error(&format!(
            "Method %Segments.prototype%.{method} called on incompatible receiver"
        )),
    }
}

/// %IntlSegmentsPrototype%.containing ( index ) (§18.5.2.1).
fn containing(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_segments(agent, &a.this, "containing")?;
    let n = agent.to_integer_or_infinity(&a.arg(0))?;
    let len = s.string.units().len();
    if n < 0.0 || n >= len as f64 {
        return Ok(Value::Undefined);
    }
    let n = n as usize;
    let (start, end) = (s.boundary_before(n), s.boundary_after(n));
    s.data_object(agent, start, end)
}

/// %IntlSegmentsPrototype% [ @@iterator ] ( ) (§18.5.2.2) y CreateSegmentIterator
/// (§18.6.1).
fn segments_iterator(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let s = this_segments(agent, &a.this, "[Symbol.iterator]")?;
    let o = web::create(
        agent,
        ITERATOR_PROTO,
        web::WebData::Intl(Box::new(IntlData::SegmentIterator(Box::new(
            SegmentIterator {
                segments: s,
                next: 0,
            },
        )))),
    );
    Ok(Value::Object(o))
}

/// %IntlSegmentIteratorPrototype%.next ( ) (§18.6.2.1).
fn iterator_next(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let (segments, start) = match intl_data(agent, &a.this) {
        Some(IntlData::SegmentIterator(it)) => (it.segments.clone(), it.next),
        _ => {
            return agent.type_error(
                "Method %SegmentIterator.prototype%.next called on incompatible receiver",
            );
        }
    };
    if start >= segments.string.units().len() {
        return agent.create_iter_result(Value::Undefined, true);
    }
    let end = segments.boundary_after(start);
    if let Value::Object(o) = &a.this
        && let Some(IntlData::SegmentIterator(it)) = crate::builtins::intl::intl_data_mut(agent, *o)
    {
        it.next = end;
    }
    let data = segments.data_object(agent, start, end)?;
    agent.create_iter_result(data, false)
}

/// [[AvailableLocales]] de Intl.Segmenter: las reglas por defecto de UAX #29 valen igual
/// para cualquier idioma, así que está disponible todo locale cuyo idioma conoce CLDR
/// (`likelySubtags`). Devuelve el locale sin la extensión `-u-`.
fn available(requested: &str) -> Option<String> {
    let (no_ext, _) = locale::remove_unicode_extension(requested);
    let lang = no_ext.split('-').next().unwrap_or("");
    super::cldr::LIKELY_SUBTAGS
        .iter()
        .any(|(k, _)| *k == lang)
        .then_some(no_ext)
}

/// Intl.Segmenter.supportedLocalesOf ( locales [ , options ] ) (§18.2.2) con
/// SupportedLocales (§9.2.11) sobre [`available`].
fn supported_locales_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let requested = locale::canonicalize_locale_list(agent, &a.arg(0))?;
    let o = crate::builtins::intl::coerce_options_to_object(agent, &a.arg(1))?;
    get_string_option(
        agent,
        o,
        "localeMatcher",
        &["lookup", "best fit"],
        Some("best fit"),
    )?;
    let vals: Vec<Value> = requested
        .iter()
        .filter(|l| available(l).is_some())
        .map(|l| str_value(l))
        .collect();
    Ok(Value::Object(agent.create_array_from_list(&vals)?))
}
