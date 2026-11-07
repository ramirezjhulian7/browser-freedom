//! Intl.Locale (ECMA-402 §14) con Intl Locale Info (getCalendars, getCollations,
//! getHourCycles, getNumberingSystems, getTimeZones, getTextInfo, getWeekInfo) y los
//! algoritmos Add/Remove Likely Subtags de UTS #35 §4.3 sobre `likelySubtags` de CLDR.

use crate::builtins::intl::locale::{self, LanguageTag, UnicodeExtension};
use crate::builtins::intl::{
    IntlData, cldr, coerce_options_to_object, create_intl_object, get_string_option,
    intl_constructor, intl_data, intl_data_mut, is_type_sequence,
};
use crate::builtins::web::str_value;
use crate::heap::Gc;
use crate::object::Object;
use crate::value::{PropertyKey, Value};
use crate::vm::function::{CallArgs, NativeFn};
use crate::vm::{Agent, JsResult, RealmId};

/// Los slots de un Intl.Locale (§14.4).
#[derive(Debug, Clone, Default)]
pub struct LocaleObj {
    /// [[Locale]]: la etiqueta canónica.
    pub locale: String,
    pub calendar: Option<String>,
    pub collation: Option<String>,
    pub first_day_of_week: Option<String>,
    pub hour_cycle: Option<String>,
    pub case_first: Option<String>,
    pub numeric: bool,
    pub numbering_system: Option<String>,
}

/// %Intl.Locale%.[[RelevantExtensionKeys]] (§14.2.3).
const RELEVANT_KEYS: [&str; 7] = ["ca", "co", "fw", "hc", "kf", "kn", "nu"];

pub fn init(agent: &mut Agent, realm: RealmId, intl: Gc<Object>) {
    let (_, p) = intl_constructor(
        agent,
        realm,
        intl,
        "Locale",
        "Intl.Locale.prototype",
        1,
        constructor,
    );
    const METHODS: &[(&str, u32, NativeFn)] = &[
        ("maximize", 0, maximize),
        ("minimize", 0, minimize),
        ("toString", 0, to_string),
        ("getCalendars", 0, get_calendars),
        ("getCollations", 0, get_collations),
        ("getHourCycles", 0, get_hour_cycles),
        ("getNumberingSystems", 0, get_numbering_systems),
        ("getTimeZones", 0, get_time_zones),
        ("getTextInfo", 0, get_text_info),
        ("getWeekInfo", 0, get_week_info),
    ];
    for &(name, len, f) in METHODS {
        crate::builtins::method(agent, p, name, len, f);
    }
    const GETTERS: &[(&str, NativeFn)] = &[
        ("baseName", get_base_name),
        ("calendar", get_calendar),
        ("caseFirst", get_case_first),
        ("collation", get_collation),
        ("firstDayOfWeek", get_first_day_of_week),
        ("hourCycle", get_hour_cycle),
        ("numeric", get_numeric),
        ("numberingSystem", get_numbering_system),
        ("language", get_language),
        ("script", get_script),
        ("region", get_region),
        ("variants", get_variants),
    ];
    for &(name, f) in GETTERS {
        crate::builtins::accessor(agent, p, PropertyKey::from(name), name, Some(f), None);
    }
}

/// La etiqueta de `o` si es un Intl.Locale (`[[InitializedLocale]]`).
pub(crate) fn locale_tag(agent: &Agent, o: Gc<Object>) -> Option<String> {
    match intl_data(agent, &Value::Object(o)) {
        Some(IntlData::Locale(l)) => Some(l.locale.clone()),
        _ => None,
    }
}

fn this_locale(agent: &mut Agent, this: &Value, method: &str) -> JsResult<LocaleObj> {
    match intl_data(agent, this) {
        Some(IntlData::Locale(l)) => Ok((**l).clone()),
        _ => agent.type_error(&format!(
            "Method Intl.Locale.prototype.{method} called on incompatible receiver"
        )),
    }
}

// --- Gramática de los subtags (UTS #35 §3.2) ---

fn is_alpha(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_alphabetic())
}

/// `unicode_language_subtag`: alpha{2,3} | alpha{5,8}.
fn is_language_subtag(s: &str) -> bool {
    is_alpha(s) && matches!(s.len(), 2 | 3 | 5..=8)
}

/// `unicode_script_subtag`: alpha{4}.
fn is_script_subtag(s: &str) -> bool {
    s.len() == 4 && is_alpha(s)
}

/// `unicode_region_subtag`: alpha{2} | digit{3}.
fn is_region_subtag(s: &str) -> bool {
    (s.len() == 2 && is_alpha(s)) || (s.len() == 3 && s.bytes().all(|b| b.is_ascii_digit()))
}

/// `unicode_variant_subtag`: alphanum{5,8} | digit alphanum{3}.
fn is_variant_subtag(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_alphanumeric())
        && ((5..=8).contains(&s.len()) || (s.len() == 4 && s.as_bytes()[0].is_ascii_digit()))
}

/// GetOption ( options, property, string, empty, default ) (§9.2.12) con un valor por
/// defecto que puede ser undefined.
fn get_string(
    agent: &mut Agent,
    options: Gc<Object>,
    property: &str,
    default: Option<String>,
) -> JsResult<Option<String>> {
    let v = agent.get_str(options, property)?;
    if v.is_undefined() {
        return Ok(default);
    }
    Ok(Some(agent.to_string(&v)?.to_string_lossy()))
}

/// UpdateLanguageId ( tag, options ) (§14.1.2).
fn update_language_id(agent: &mut Agent, tag: &str, options: Gc<Object>) -> JsResult<String> {
    let parsed = locale::parse(tag).expect("etiqueta válida");
    let base = LanguageTag {
        language: parsed.language.clone(),
        script: parsed.script.clone(),
        region: parsed.region.clone(),
        variants: parsed.variants.clone(),
        ..LanguageTag::default()
    };
    // El texto original de cada parte (GetLocaleLanguage y compañía, sobre el baseName).
    let parts: Vec<&str> = tag.split('-').collect();
    let mut i = 0;
    let language = parts[i].to_owned();
    i += 1;
    let script = base.script.as_ref().map(|_| {
        i += 1;
        parts[i - 1].to_owned()
    });
    let region = base.region.as_ref().map(|_| {
        i += 1;
        parts[i - 1].to_owned()
    });
    let variants = if base.variants.is_empty() {
        None
    } else {
        let v = parts[i..i + base.variants.len()].join("-");
        i += base.variants.len();
        Some(v)
    };
    let all_extensions: String = parts[i..].iter().map(|p| format!("-{p}")).collect();

    let language = get_string(agent, options, "language", Some(language))?.unwrap_or_default();
    if !is_language_subtag(&language) {
        return agent.range_error(&format!("Incorrect locale information provided: {language}"));
    }
    let script = get_string(agent, options, "script", script)?;
    if let Some(s) = &script
        && !is_script_subtag(s)
    {
        return agent.range_error(&format!("Incorrect locale information provided: {s}"));
    }
    let region = get_string(agent, options, "region", region)?;
    if let Some(r) = &region
        && !is_region_subtag(r)
    {
        return agent.range_error(&format!("Incorrect locale information provided: {r}"));
    }
    let variants = get_string(agent, options, "variants", variants)?;
    if let Some(v) = &variants {
        let lower = v.to_ascii_lowercase();
        let subtags: Vec<&str> = lower.split('-').collect();
        let mut seen: Vec<&str> = Vec::new();
        for s in &subtags {
            if !is_variant_subtag(s) || seen.contains(s) {
                return agent.range_error(&format!("Incorrect locale information provided: {v}"));
            }
            seen.push(s);
        }
    }
    let mut new_tag = language;
    for part in [&script, &region, &variants].into_iter().flatten() {
        new_tag.push('-');
        new_tag.push_str(part);
    }
    new_tag.push_str(&all_extensions);
    Ok(new_tag)
}

/// WeekdayToUValue ( fw ) (§14.5.x): los números y nombres de día a `mon`…`sun`.
fn weekday_to_u_value(fw: &str) -> String {
    const DAYS: [&str; 8] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    if let Ok(n) = fw.parse::<usize>()
        && fw.len() == 1
        && n < DAYS.len()
    {
        return DAYS[n].to_owned();
    }
    fw.to_owned()
}

/// MakeLocaleRecord ( tag, options, relevantExtensionKeys ) (§14.1.3): la etiqueta con las
/// claves pedidas por opción, y el valor de cada clave relevante.
fn make_locale_record(tag: &str, options: &[(&str, Option<String>)]) -> LocaleObj {
    let mut parsed = locale::parse(tag).expect("etiqueta válida");
    let mut ext = parsed.unicode.take().unwrap_or_default();
    let mut values: Vec<(&str, Option<String>)> = Vec::new();
    for key in RELEVANT_KEYS {
        let mut value = ext
            .keywords
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone());
        if let Some((_, Some(over))) = options.iter().find(|(k, _)| *k == key) {
            let canon = locale::canonicalize_keyword_value(key, &over.to_ascii_lowercase());
            match ext.keywords.iter_mut().find(|(k, _)| k == key) {
                Some(entry) => entry.1 = canon.clone(),
                None => ext.keywords.push((key.to_owned(), canon.clone())),
            }
            value = Some(canon);
        }
        values.push((key, value));
    }
    if !ext.attributes.is_empty() || !ext.keywords.is_empty() {
        parsed.unicode = Some(UnicodeExtension {
            attributes: ext.attributes,
            keywords: ext.keywords,
        });
    }
    let locale = locale::canonicalize(&parsed.to_string_canonical());
    let get = |k: &str| {
        values
            .iter()
            .find(|(key, _)| *key == k)
            .and_then(|(_, v)| v.clone())
    };
    LocaleObj {
        locale,
        calendar: get("ca"),
        collation: get("co"),
        first_day_of_week: get("fw"),
        hour_cycle: get("hc"),
        case_first: get("kf"),
        numeric: matches!(get("kn").as_deref(), Some("" | "true")),
        numbering_system: get("nu"),
    }
}

/// Intl.Locale ( tag [ , options ] ) (§14.1.1).
fn constructor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(nt) = a.new_target else {
        return agent.type_error("Constructor Intl.Locale requires 'new'");
    };
    let obj = create_intl_object(
        agent,
        nt,
        "Intl.Locale.prototype",
        IntlData::Locale(Box::default()),
    )?;
    let tag_arg = a.arg(0);
    let tag = match &tag_arg {
        Value::Object(o) => match locale_tag(agent, *o) {
            Some(t) => t,
            None => agent.to_string(&tag_arg)?.to_string_lossy(),
        },
        Value::String(s) => s.to_string_lossy(),
        _ => return agent.type_error("First argument to Intl.Locale constructor can't be empty or missing"),
    };
    let options = coerce_options_to_object(agent, &a.arg(1))?;
    if !locale::is_structurally_valid(&tag) {
        return agent.range_error(&format!("Incorrect locale information provided: {tag}"));
    }
    // Canonicalizar antes de aplicar las opciones (`und-Armn-SU` + `ru` es `ru-Armn-AM`).
    let tag = locale::canonicalize(&tag);
    let tag = update_language_id(agent, &tag, options)?;
    if !locale::is_structurally_valid(&tag) {
        return agent.range_error(&format!("Incorrect locale information provided: {tag}"));
    }
    let tag = locale::canonicalize(&tag);
    let type_option = |agent: &mut Agent, name: &str| -> JsResult<Option<String>> {
        let v = get_string_option(agent, options, name, &[], None)?;
        if let Some(s) = &v
            && !is_type_sequence(s)
        {
            return agent.range_error(&format!("Incorrect {name} provided: {s}"));
        }
        Ok(v)
    };
    let ca = type_option(agent, "calendar")?;
    let co = type_option(agent, "collation")?;
    let fw = get_string_option(agent, options, "firstDayOfWeek", &[], None)?.map(|f| weekday_to_u_value(&f));
    if let Some(f) = &fw
        && !is_type_sequence(f)
    {
        return agent.range_error(&format!("Incorrect firstDayOfWeek provided: {f}"));
    }
    let hc = get_string_option(agent, options, "hourCycle", &["h11", "h12", "h23", "h24"], None)?;
    let kf = get_string_option(agent, options, "caseFirst", &["upper", "lower", "false"], None)?;
    let kn = crate::builtins::intl::get_boolean_option(agent, options, "numeric")?
        .map(|b| if b { "true".to_owned() } else { "false".to_owned() });
    let nu = type_option(agent, "numberingSystem")?;
    let r = make_locale_record(
        &tag,
        &[
            ("ca", ca),
            ("co", co),
            ("fw", fw),
            ("hc", hc),
            ("kf", kf),
            ("kn", kn),
            ("nu", nu),
        ],
    );
    if let Some(IntlData::Locale(l)) = intl_data_mut(agent, obj) {
        **l = r;
    }
    Ok(Value::Object(obj))
}

/// Un Intl.Locale nuevo del realm actual para una etiqueta válida (Construct(%Intl.Locale%,
/// « tag »)).
fn new_locale(agent: &mut Agent, tag: &str) -> JsResult<Value> {
    let r = make_locale_record(&locale::canonicalize(tag), &[]);
    let o = crate::builtins::web::create(
        agent,
        "Intl.Locale.prototype",
        crate::builtins::web::WebData::Intl(Box::new(IntlData::Locale(Box::new(r)))),
    );
    Ok(Value::Object(o))
}

// --- Likely Subtags (UTS #35 §4.3) ---

/// (language, script, region) en minúsculas.
type Lsr = (String, Option<String>, Option<String>);

fn lookup_likely(key: &str) -> Option<&'static str> {
    cldr::LIKELY_SUBTAGS
        .binary_search_by(|(k, _)| (*k).cmp(key))
        .ok()
        .map(|i| cldr::LIKELY_SUBTAGS[i].1)
}

/// Add Likely Subtags: `None` si no hay coincidencia.
fn add_likely(lsr: &Lsr) -> Option<Lsr> {
    let (l, s, r) = lsr;
    let mut keys: Vec<String> = Vec::new();
    if let (Some(s), Some(r)) = (s, r) {
        keys.push(format!("{l}-{s}-{r}"));
    }
    if let Some(r) = r {
        keys.push(format!("{l}-{r}"));
    }
    if let Some(s) = s {
        keys.push(format!("{l}-{s}"));
    }
    keys.push(l.clone());
    if let Some(s) = s
        && l != "und"
    {
        keys.push(format!("und-{s}"));
    }
    let m = keys.iter().find_map(|k| lookup_likely(k))?;
    let mt = locale::parse(m)?;
    Some((
        if l == "und" {
            mt.language
        } else {
            l.clone()
        },
        s.clone().or(mt.script),
        r.clone().or(mt.region),
    ))
}

/// Remove Likely Subtags, favoreciendo la región.
fn remove_likely(lsr: &Lsr) -> Option<Lsr> {
    let max = add_likely(lsr)?;
    let (l, s, r) = &max;
    let trials = [
        (l.clone(), None, None),
        (l.clone(), None, r.clone()),
        (l.clone(), s.clone(), None),
    ];
    for t in trials {
        if add_likely(&t).as_ref() == Some(&max) {
            return Some(t);
        }
    }
    Some(max)
}

fn lsr_of(tag: &LanguageTag) -> Lsr {
    (tag.language.clone(), tag.script.clone(), tag.region.clone())
}

fn with_lsr(tag: &LanguageTag, lsr: Lsr) -> String {
    let mut t = tag.clone();
    t.language = lsr.0;
    t.script = lsr.1;
    t.region = lsr.2;
    t.to_string_canonical()
}

/// Intl.Locale.prototype.maximize ( ) (§14.3.3).
fn maximize(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "maximize")?;
    let tag = locale::parse(&loc.locale).expect("etiqueta válida");
    let out = match add_likely(&lsr_of(&tag)) {
        Some(m) => with_lsr(&tag, m),
        None => loc.locale.clone(),
    };
    new_locale(agent, &out)
}

/// Intl.Locale.prototype.minimize ( ) (§14.3.4).
fn minimize(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "minimize")?;
    let tag = locale::parse(&loc.locale).expect("etiqueta válida");
    let out = match remove_likely(&lsr_of(&tag)) {
        Some(m) => with_lsr(&tag, m),
        None => loc.locale.clone(),
    };
    new_locale(agent, &out)
}

/// Intl.Locale.prototype.toString ( ) (§14.3.5).
fn to_string(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "toString")?;
    Ok(str_value(&loc.locale))
}

// --- Getters (§14.3.6–§14.3.17) ---

fn opt_str(v: &Option<String>) -> Value {
    v.as_deref().map_or(Value::Undefined, str_value)
}

fn base_tag(loc: &LocaleObj) -> LanguageTag {
    locale::parse(&loc.locale).expect("etiqueta válida")
}

fn get_base_name(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "baseName")?;
    Ok(str_value(&base_tag(&loc).language_id()))
}

fn get_calendar(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "calendar")?;
    Ok(opt_str(&loc.calendar))
}

fn get_case_first(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "caseFirst")?;
    Ok(opt_str(&loc.case_first))
}

fn get_collation(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "collation")?;
    Ok(opt_str(&loc.collation))
}

fn get_first_day_of_week(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "firstDayOfWeek")?;
    Ok(opt_str(&loc.first_day_of_week))
}

fn get_hour_cycle(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "hourCycle")?;
    Ok(opt_str(&loc.hour_cycle))
}

fn get_numeric(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "numeric")?;
    Ok(Value::Boolean(loc.numeric))
}

fn get_numbering_system(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "numberingSystem")?;
    Ok(opt_str(&loc.numbering_system))
}

fn get_language(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "language")?;
    Ok(str_value(&base_tag(&loc).language))
}

fn get_script(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "script")?;
    let t = base_tag(&loc);
    Ok(match t.script {
        Some(s) => {
            let mut c = s.chars();
            let first = c.next().map(|f| f.to_ascii_uppercase()).unwrap_or('X');
            str_value(&format!("{first}{}", c.as_str()))
        }
        None => Value::Undefined,
    })
}

fn get_region(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "region")?;
    Ok(base_tag(&loc)
        .region
        .map_or(Value::Undefined, |r| str_value(&r.to_ascii_uppercase())))
}

fn get_variants(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "variants")?;
    let t = base_tag(&loc);
    Ok(if t.variants.is_empty() {
        Value::Undefined
    } else {
        str_value(&t.variants.join("-"))
    })
}

// --- Intl Locale Info ---

/// El valor de la clave `key` de la extensión `-u-` de la etiqueta.
fn keyword(tag: &LanguageTag, key: &str) -> Option<String> {
    tag.unicode
        .as_ref()?
        .keywords
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

/// La región de un valor de `rg`/`sd` (`afzzzz` → `AF`, `inka` → `IN`, `419zzzz` → `419`).
fn subdivision_region(v: &str) -> Option<String> {
    let b = v.as_bytes();
    if b.len() >= 3 && b[..3].iter().all(u8::is_ascii_digit) {
        return Some(v[..3].to_owned());
    }
    if b.len() >= 2 && b[..2].iter().all(u8::is_ascii_alphabetic) {
        return Some(v[..2].to_ascii_uppercase());
    }
    None
}

/// RegionPreference ( loc ): (región de `rg`, región) por orden de prioridad: la clave `rg`,
/// el subtag de región, la clave `sd`, Add Likely Subtags y `001`.
fn region_preference(loc: &LocaleObj) -> (Option<String>, String) {
    let t = base_tag(loc);
    let rg = keyword(&t, "rg").and_then(|v| subdivision_region(&v));
    let region = if let Some(r) = &t.region {
        r.to_ascii_uppercase()
    } else if let Some(r) = keyword(&t, "sd").and_then(|v| subdivision_region(&v)) {
        r
    } else if let Some((_, _, Some(r))) = add_likely(&lsr_of(&t)) {
        r.to_ascii_uppercase()
    } else {
        "001".to_owned()
    };
    (rg, region)
}

/// Los datos de la región de `rg` si existen, si no los de la región.
fn regional<T>(loc: &LocaleObj, f: impl Fn(&str) -> Option<T>) -> Option<T> {
    let (rg, region) = region_preference(loc);
    rg.and_then(|r| f(&r)).or_else(|| f(&region))
}

fn lookup_pair(table: &'static [(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    table
        .binary_search_by(|(k, _)| (*k).cmp(key))
        .ok()
        .map(|i| table[i].1)
}

fn string_array(agent: &mut Agent, list: &[String]) -> JsResult<Value> {
    let vals: Vec<Value> = list.iter().map(|s| str_value(s)).collect();
    Ok(Value::Object(agent.create_array_from_list(&vals)?))
}

/// Intl.Locale.prototype.getCalendars ( ): CalendarsOfLocale.
fn get_calendars(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "getCalendars")?;
    let list: Vec<String> = match &loc.calendar {
        Some(c) => vec![c.clone()],
        None => regional(&loc, |r| lookup_pair(cldr::CALENDAR_PREFERENCES, r))
            .unwrap_or("gregory")
            .split(' ')
            .map(str::to_owned)
            .collect(),
    };
    string_array(agent, &list)
}

/// Intl.Locale.prototype.getCollations ( ): CollationsOfLocale. Las colaciones de la raíz
/// (`emoji`, `eor`), que Intl.Collator acepta para todo locale.
fn get_collations(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "getCollations")?;
    let list: Vec<String> = match &loc.collation {
        Some(c) => vec![c.clone()],
        None => ROOT_COLLATIONS.iter().map(|s| (*s).to_owned()).collect(),
    };
    string_array(agent, &list)
}

/// Las colaciones de CLDR disponibles en todo locale (ordenadas).
pub(crate) const ROOT_COLLATIONS: [&str; 2] = ["emoji", "eor"];

/// Intl.Locale.prototype.getHourCycles ( ): HourCyclesOfLocale (timeData de CLDR).
fn get_hour_cycles(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "getHourCycles")?;
    let list: Vec<String> = match &loc.hour_cycle {
        Some(h) => vec![h.clone()],
        None => {
            let sym = regional(&loc, |r| lookup_pair(cldr::HOUR_CYCLES, r))
                .or_else(|| lookup_pair(cldr::HOUR_CYCLES, "001"))
                .unwrap_or("H");
            vec![
                match sym {
                    "h" => "h12",
                    "K" => "h11",
                    "k" => "h24",
                    _ => "h23",
                }
                .to_owned(),
            ]
        }
    };
    string_array(agent, &list)
}

/// Intl.Locale.prototype.getNumberingSystems ( ): NumberingSystemsOfLocale (los locales con
/// datos usan `latn`).
fn get_numbering_systems(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "getNumberingSystems")?;
    let list = vec![loc.numbering_system.clone().unwrap_or_else(|| "latn".into())];
    string_array(agent, &list)
}

/// Intl.Locale.prototype.getTimeZones ( ): TimeZonesOfLocale; undefined sin región.
fn get_time_zones(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "getTimeZones")?;
    let Some(region) = base_tag(&loc).region else {
        return Ok(Value::Undefined);
    };
    let region = region.to_ascii_uppercase();
    let mut list: Vec<String> = lookup_pair(cldr::TIME_ZONES_BY_REGION, &region)
        .map(|z| z.split(' ').map(str::to_owned).collect())
        .unwrap_or_default();
    list.sort();
    string_array(agent, &list)
}

/// Intl.Locale.prototype.getTextInfo ( ): TextDirectionOfLocale (scriptMetadata de CLDR);
/// undefined si la dirección del script no se conoce.
fn get_text_info(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "getTextInfo")?;
    let t = base_tag(&loc);
    let script = t
        .script
        .clone()
        .or_else(|| add_likely(&lsr_of(&t)).and_then(|m| m.1));
    let dir = script.and_then(|s| {
        let mut c = s.chars();
        let first = c.next()?.to_ascii_uppercase();
        lookup_pair(cldr::SCRIPT_DIRECTION, &format!("{first}{}", c.as_str()))
    });
    let o = agent.new_plain_object();
    let v = dir.map_or(Value::Undefined, str_value);
    agent.create_data_property_or_throw(o, PropertyKey::from("direction"), v)?;
    Ok(Value::Object(o))
}

/// Intl.Locale.prototype.getWeekInfo ( ): WeekInfoOfLocale (weekData de CLDR).
fn get_week_info(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let loc = this_locale(agent, &a.this, "getWeekInfo")?;
    let data = |r: &str| {
        cldr::WEEK_DATA
            .binary_search_by(|(k, ..)| (*k).cmp(r))
            .ok()
            .map(|i| cldr::WEEK_DATA[i])
    };
    let (_, mut first, start, end) = regional(&loc, data)
        .or_else(|| data("001"))
        .unwrap_or(("001", 1, 6, 7));
    const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    if let Some(fw) = &loc.first_day_of_week
        && let Some(i) = DAYS.iter().position(|d| d == fw)
    {
        first = i as u8 + 1;
    }
    let mut weekend: Vec<u8> = Vec::new();
    let mut d = start;
    loop {
        weekend.push(d);
        if d == end || weekend.len() >= 7 {
            break;
        }
        d = d % 7 + 1;
    }
    weekend.sort_unstable();
    let o = agent.new_plain_object();
    agent.create_data_property_or_throw(
        o,
        PropertyKey::from("firstDay"),
        Value::Number(f64::from(first)),
    )?;
    let we: Vec<Value> = weekend
        .iter()
        .map(|d| Value::Number(f64::from(*d)))
        .collect();
    let arr = agent.create_array_from_list(&we)?;
    agent.create_data_property_or_throw(o, PropertyKey::from("weekend"), Value::Object(arr))?;
    Ok(Value::Object(o))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn max(t: &str) -> String {
        let tag = locale::parse(t).unwrap();
        with_lsr(&tag, add_likely(&lsr_of(&tag)).unwrap())
    }

    fn min(t: &str) -> String {
        let tag = locale::parse(t).unwrap();
        with_lsr(&tag, remove_likely(&lsr_of(&tag)).unwrap())
    }

    #[test]
    fn likely_subtags() {
        assert_eq!(max("en"), "en-Latn-US");
        assert_eq!(max("en-Arab"), "en-Arab-US");
        assert_eq!(max("und-150"), "en-Latn-150");
        assert_eq!(max("und-Cyrl-RO"), "bg-Cyrl-RO");
        assert_eq!(max("it-Kana-CA"), "it-Kana-CA");
        assert_eq!(min("en-Shaw-GB"), "en-Shaw");
        assert_eq!(min("und-Latn-AQ"), "en-AQ");
        assert_eq!(min("zh-Hant"), "zh-TW");
        assert_eq!(min("zh-Hani"), "zh-Hani");
        assert_eq!(min("aae-Thai-CO"), "aae-Thai-CO");
    }
}
