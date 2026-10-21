//! Intl.DateTimeFormat (ECMA-402 §11): CreateDateTimeFormat (opciones, ciclo horario y zona
//! horaria, con offsets `±HH:MM` e identificadores IANA sin canonicalizar), un best-fit matcher
//! sobre los `availableFormats` de CLDR (y `dateStyle`/`timeStyle`), el formateo de patrones
//! con sus partes, `formatRange` con los `intervalFormats` y `resolvedOptions`.

use crate::builtins::date::{
    date_from_time, hour_from_time, min_from_time, month_from_time, ms_from_time, sec_from_time,
    week_day, year_from_time, zone_offset_ms,
};
use crate::builtins::intl::number_format::chain_legacy;
use crate::builtins::intl::{
    IntlData, cldr, coerce_options_to_object, create_intl_object, get_boolean_option,
    get_number_option, get_string_option, intl_constructor, intl_data, intl_data_mut,
    is_type_sequence, locale, numbering_system_digits, numbering_systems, time_zone_identifier,
};
use crate::builtins::web::str_value;
use crate::heap::Gc;
use crate::object::Object;
use crate::value::{PropertyKey, Value};
use crate::vm::function::CallArgs;
use crate::vm::{Agent, JsResult, RealmId};

/// Los componentes (Table 16) en orden: (nombre, valores).
const COMPONENTS: &[(&str, &[&str])] = &[
    ("weekday", &["narrow", "short", "long"]),
    ("era", &["narrow", "short", "long"]),
    ("year", &["2-digit", "numeric"]),
    ("month", &["2-digit", "numeric", "narrow", "short", "long"]),
    ("day", &["2-digit", "numeric"]),
    ("dayPeriod", &["narrow", "short", "long"]),
    ("hour", &["2-digit", "numeric"]),
    ("minute", &["2-digit", "numeric"]),
    ("second", &["2-digit", "numeric"]),
    ("fractionalSecondDigits", &[]),
    (
        "timeZoneName",
        &[
            "short",
            "long",
            "shortOffset",
            "longOffset",
            "shortGeneric",
            "longGeneric",
        ],
    ),
];

/// Los slots de un Intl.DateTimeFormat.
#[derive(Debug, Clone)]
pub struct DateTimeFormat {
    pub locale: String,
    pub data_locale: String,
    pub calendar: String,
    pub numbering_system: String,
    pub time_zone: String,
    pub hour_cycle: Option<String>,
    pub date_style: Option<String>,
    pub time_style: Option<String>,
    /// El patrón de CLDR resultante.
    pub pattern: String,
    pub bound_format: Option<Gc<Object>>,
}

pub fn init(agent: &mut Agent, realm: RealmId, intl: Gc<Object>) {
    let (c, p) = intl_constructor(
        agent,
        realm,
        intl,
        "DateTimeFormat",
        "Intl.DateTimeFormat.prototype",
        0,
        constructor,
    );
    crate::builtins::web::registry_set(agent, realm, "Intl.DateTimeFormat", c);
    crate::builtins::method(agent, c, "supportedLocalesOf", 1, supported_locales_of);
    crate::builtins::accessor(
        agent,
        p,
        PropertyKey::from("format"),
        "format",
        Some(get_format),
        None,
    );
    crate::builtins::method(agent, p, "formatToParts", 1, format_to_parts);
    crate::builtins::method(agent, p, "formatRange", 2, format_range);
    crate::builtins::method(agent, p, "formatRangeToParts", 2, format_range_to_parts);
    crate::builtins::method(agent, p, "resolvedOptions", 0, resolved_options);
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Required {
    Date,
    Time,
    Any,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Defaults {
    Date,
    Time,
    All,
}

/// El ciclo horario por defecto de un locale (timeData de su región).
fn default_hour_cycle(locale: &str) -> &'static str {
    let tag = locale::parse(locale);
    let region = tag
        .as_ref()
        .and_then(|t| t.region.clone())
        .map(|r| r.to_ascii_uppercase())
        .or_else(|| {
            tag.as_ref().and_then(|t| {
                cldr::LIKELY_REGION
                    .binary_search_by(|(k, _)| (*k).cmp(t.language.as_str()))
                    .ok()
                    .map(|i| cldr::LIKELY_REGION[i].1.to_owned())
            })
        })
        .unwrap_or_else(|| "001".into());
    let pref = cldr::HOUR_CYCLES
        .iter()
        .find(|(r, _)| *r == region)
        .map_or("H", |(_, p)| p);
    match pref {
        "h" => "h12",
        "K" => "h11",
        "k" => "h24",
        _ => "h23",
    }
}

/// CreateDateTimeFormat ( newTarget, locales, options, required, defaults ) (§11.1.2).
pub fn create_date_time_format(
    agent: &mut Agent,
    new_target: Gc<Object>,
    locales: &Value,
    options: &Value,
    required: Required,
    defaults: Defaults,
) -> JsResult<Gc<Object>> {
    let requested = locale::canonicalize_locale_list(agent, locales)?;
    let options = coerce_options_to_object(agent, options)?;
    get_string_option(
        agent,
        options,
        "localeMatcher",
        &["lookup", "best fit"],
        Some("best fit"),
    )?;
    let calendar = get_string_option(agent, options, "calendar", &[], None)?;
    if let Some(c) = &calendar
        && !is_type_sequence(c)
    {
        return agent.range_error(&format!("Invalid calendar : {c}"));
    }
    let nu = get_string_option(agent, options, "numberingSystem", &[], None)?;
    if let Some(n) = &nu
        && !is_type_sequence(n)
    {
        return agent.range_error(&format!("Invalid numberingSystem : {n}"));
    }
    let hour12 = get_boolean_option(agent, options, "hour12")?;
    let mut hour_cycle = get_string_option(
        agent,
        options,
        "hourCycle",
        &["h11", "h12", "h23", "h24"],
        None,
    )?;
    if hour12.is_some() {
        // La opción `hc` es null: en ResolveLocale anula la extensión `-u-hc`.
        hour_cycle = Some(locale::NULL_OPTION.to_owned());
    }
    let r = locale::resolve_locale(
        &requested,
        &[("ca", calendar), ("hc", hour_cycle), ("nu", nu)],
        &["ca", "hc", "nu"],
        &|key, _| match key {
            "ca" => vec!["gregory".into(), "iso8601".into()],
            "hc" => vec![
                String::new(),
                "h11".into(),
                "h12".into(),
                "h23".into(),
                "h24".into(),
            ],
            "nu" => {
                let mut v = vec!["latn".to_owned()];
                v.extend(numbering_systems().into_iter().filter(|n| n != "latn"));
                v
            }
            _ => Vec::new(),
        },
    );
    let data_locale = r.data_locale.clone();
    let hc: String = match hour12 {
        Some(true) => {
            if default_hour_cycle(&data_locale) == "h11" {
                "h11".into()
            } else {
                "h12".into()
            }
        }
        Some(false) => "h23".into(),
        None => match r.get("hc") {
            Some(h) if !h.is_empty() => h.to_owned(),
            _ => default_hour_cycle(&data_locale).to_owned(),
        },
    };
    // La zona horaria.
    let tzv = agent.get_str(options, "timeZone")?;
    let time_zone = if tzv.is_undefined() {
        crate::builtins::date::system_time_zone_identifier()
    } else {
        let s = agent.to_string(&tzv)?.to_string_lossy();
        match crate::builtins::date::parse_offset_identifier(&s) {
            Some(Ok(m)) => crate::builtins::date::format_offset_identifier(m),
            Some(Err(())) => {
                return agent.range_error(&format!("Invalid time zone specified: {s}"));
            }
            None => match time_zone_identifier(&s) {
                Some((id, _)) => id.to_owned(),
                None => return agent.range_error(&format!("Invalid time zone specified: {s}")),
            },
        }
    };
    // Los componentes.
    let mut comps: Vec<(&'static str, Option<String>)> = Vec::new();
    let mut explicit = false;
    for &(prop, values) in COMPONENTS {
        let v = if prop == "fractionalSecondDigits" {
            get_number_option(agent, options, prop, 1.0, 3.0, None)?.map(|n| (n as u32).to_string())
        } else {
            get_string_option(agent, options, prop, values, None)?
        };
        if v.is_some() {
            explicit = true;
        }
        comps.push((prop, v));
    }
    get_string_option(
        agent,
        options,
        "formatMatcher",
        &["basic", "best fit"],
        Some("best fit"),
    )?;
    let date_style = get_string_option(
        agent,
        options,
        "dateStyle",
        &["full", "long", "medium", "short"],
        None,
    )?;
    let time_style = get_string_option(
        agent,
        options,
        "timeStyle",
        &["full", "long", "medium", "short"],
        None,
    )?;
    let data = &locale::data_for(&data_locale).dates;
    let pattern = if date_style.is_some() || time_style.is_some() {
        if explicit {
            return agent
                .type_error("Can't set option dateStyle/timeStyle with explicit components");
        }
        if required == Required::Date && time_style.is_some() {
            return agent.type_error("timeStyle is not allowed here");
        }
        if required == Required::Time && date_style.is_some() {
            return agent.type_error("dateStyle is not allowed here");
        }
        style_pattern(data, date_style.as_deref(), time_style.as_deref(), &hc)
    } else {
        let get = |comps: &[(&str, Option<String>)], k: &str| -> bool {
            comps.iter().any(|(p, v)| *p == k && v.is_some())
        };
        let mut need_defaults = true;
        if matches!(required, Required::Date | Required::Any)
            && ["weekday", "year", "month", "day"]
                .iter()
                .any(|k| get(&comps, k))
        {
            need_defaults = false;
        }
        if matches!(required, Required::Time | Required::Any)
            && [
                "dayPeriod",
                "hour",
                "minute",
                "second",
                "fractionalSecondDigits",
            ]
            .iter()
            .any(|k| get(&comps, k))
        {
            need_defaults = false;
        }
        if need_defaults && matches!(defaults, Defaults::Date | Defaults::All) {
            for k in ["year", "month", "day"] {
                set_comp(&mut comps, k, "numeric");
            }
        }
        if need_defaults && matches!(defaults, Defaults::Time | Defaults::All) {
            for k in ["hour", "minute", "second"] {
                set_comp(&mut comps, k, "numeric");
            }
        }
        best_fit_pattern(data, &comps, &hc)
    };
    let has_hour = pattern_fields(&pattern)
        .iter()
        .any(|(c, _)| matches!(c, 'h' | 'H' | 'k' | 'K'));
    let dtf = DateTimeFormat {
        locale: r.locale.clone(),
        data_locale,
        calendar: r.get("ca").unwrap_or("gregory").to_owned(),
        numbering_system: r.get("nu").unwrap_or("latn").to_owned(),
        time_zone,
        hour_cycle: has_hour.then_some(hc),
        date_style,
        time_style,
        pattern,
        bound_format: None,
    };
    create_intl_object(
        agent,
        new_target,
        "Intl.DateTimeFormat.prototype",
        IntlData::DateTimeFormat(Box::new(dtf)),
    )
}

fn set_comp(comps: &mut [(&'static str, Option<String>)], k: &str, v: &str) {
    if let Some(e) = comps.iter_mut().find(|(p, _)| *p == k) {
        e.1 = Some(v.to_owned());
    }
}

/// El carácter de hora de un ciclo horario.
fn hour_char(hc: &str) -> char {
    match hc {
        "h11" => 'K',
        "h12" => 'h',
        "h24" => 'k',
        _ => 'H',
    }
}

/// Los campos de un patrón: (letra, cantidad); los literales como ('\'', 0).
fn pattern_fields(p: &str) -> Vec<(char, usize)> {
    tokenize(p)
        .into_iter()
        .filter_map(|t| match t {
            PatTok::Field(c, n) => Some((c, n)),
            PatTok::Lit(_) => None,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
enum PatTok {
    Field(char, usize),
    Lit(String),
}

fn tokenize(p: &str) -> Vec<PatTok> {
    let chars: Vec<char> = p.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut lit = String::new();
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            if chars.get(i + 1) == Some(&'\'') {
                lit.push('\'');
                i += 2;
                continue;
            }
            i += 1;
            while i < chars.len() && chars[i] != '\'' {
                lit.push(chars[i]);
                i += 1;
            }
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() {
            if !lit.is_empty() {
                out.push(PatTok::Lit(std::mem::take(&mut lit)));
            }
            let start = i;
            while i < chars.len() && chars[i] == c {
                i += 1;
            }
            out.push(PatTok::Field(c, i - start));
            continue;
        }
        lit.push(c);
        i += 1;
    }
    if !lit.is_empty() {
        out.push(PatTok::Lit(lit));
    }
    out
}

fn untokenize(toks: &[PatTok]) -> String {
    let mut out = String::new();
    for t in toks {
        match t {
            PatTok::Field(c, n) => out.extend(std::iter::repeat_n(*c, *n)),
            PatTok::Lit(s) => {
                if s.chars().any(|c| c.is_ascii_alphabetic() || c == '\'') {
                    out.push('\'');
                    out.push_str(&s.replace('\'', "''"));
                    out.push('\'');
                } else {
                    out.push_str(s);
                }
            }
        }
    }
    out
}

/// Ajusta el ciclo horario de un patrón de hora (cambia la letra y agrega o quita `a`).
fn apply_hour_cycle(p: &str, hc: &str) -> String {
    let hcch = hour_char(hc);
    let twelve = matches!(hcch, 'h' | 'K');
    let mut toks = tokenize(p);
    let mut has_hour = false;
    let mut has_period = false;
    for t in &mut toks {
        if let PatTok::Field(c, _) = t {
            if matches!(c, 'h' | 'H' | 'k' | 'K') {
                *c = hcch;
                has_hour = true;
            }
            if matches!(c, 'a' | 'b' | 'B') {
                has_period = true;
            }
        }
    }
    if !has_hour {
        return p.to_owned();
    }
    if !twelve && has_period {
        // Quitar `a` y el espacio que lo separa.
        let mut out: Vec<PatTok> = Vec::new();
        for t in toks {
            if matches!(t, PatTok::Field('a', _)) {
                if let Some(PatTok::Lit(l)) = out.last_mut() {
                    let trimmed = l.trim_end_matches([' ', '\u{a0}', '\u{202f}']).to_owned();
                    *l = trimmed;
                    if l.is_empty() {
                        out.pop();
                    }
                }
                continue;
            }
            out.push(t);
        }
        // Un espacio que quedó al principio.
        if let Some(PatTok::Lit(l)) = out.first_mut() {
            let trimmed = l.trim_start_matches([' ', '\u{a0}', '\u{202f}']).to_owned();
            *l = trimmed;
            if l.is_empty() {
                out.remove(0);
            }
        }
        return untokenize(&out);
    }
    if twelve && !has_period {
        // Agregar ` a` después del último campo de tiempo.
        let last = toks
            .iter()
            .rposition(
                |t| matches!(t, PatTok::Field(c, _) if matches!(c, 'h' | 'K' | 'm' | 's' | 'S')),
            )
            .unwrap_or(toks.len() - 1);
        toks.insert(last + 1, PatTok::Field('a', 1));
        toks.insert(last + 1, PatTok::Lit(" ".into()));
    }
    untokenize(&toks)
}

/// DateTimeStyleFormat (§11.5.?).
fn style_pattern(data: &cldr::DateData, ds: Option<&str>, ts: Option<&str>, hc: &str) -> String {
    let idx = |s: &str| match s {
        "full" => 0,
        "long" => 1,
        "medium" => 2,
        _ => 3,
    };
    let date = ds.map(|s| data.date_formats[idx(s)].to_owned());
    let time = ts.map(|s| apply_hour_cycle(data.time_formats[idx(s)], hc));
    match (date, time) {
        (Some(d), Some(t)) => data.date_time_formats[idx(ds.unwrap_or("short"))]
            .replace("{1}", &d)
            .replace("{0}", &t),
        (Some(d), None) => d,
        (None, Some(t)) => t,
        (None, None) => String::new(),
    }
}

/// Campos de patrón: (letra, cantidad).
type PatternFields = Vec<(char, usize)>;

/// El skeleton pedido por los componentes: (campos de fecha, campos de hora).
fn skeleton(comps: &[(&'static str, Option<String>)], hc: &str) -> (PatternFields, PatternFields) {
    let mut date = Vec::new();
    let mut time = Vec::new();
    for (k, v) in comps {
        let Some(v) = v else { continue };
        let width = |n: &str| match n {
            "narrow" => 5,
            "long" => 4,
            _ => 3,
        };
        match *k {
            "era" => date.push((
                'G',
                match v.as_str() {
                    "long" => 4,
                    "narrow" => 5,
                    _ => 1,
                },
            )),
            "year" => date.push(('y', if v == "2-digit" { 2 } else { 1 })),
            "month" => date.push((
                'M',
                match v.as_str() {
                    "numeric" => 1,
                    "2-digit" => 2,
                    other => width(other),
                },
            )),
            "day" => date.push(('d', if v == "2-digit" { 2 } else { 1 })),
            "weekday" => date.push(('E', width(v))),
            "dayPeriod" => time.push((
                'B',
                match v.as_str() {
                    "long" => 4,
                    "narrow" => 5,
                    _ => 1,
                },
            )),
            "hour" => time.push((hour_char(hc), if v == "2-digit" { 2 } else { 1 })),
            "minute" => time.push(('m', if v == "2-digit" { 2 } else { 1 })),
            "second" => time.push(('s', if v == "2-digit" { 2 } else { 1 })),
            "fractionalSecondDigits" => time.push(('S', v.parse().unwrap_or(1))),
            "timeZoneName" => time.push(match v.as_str() {
                "long" => ('z', 4),
                "shortOffset" => ('O', 1),
                "longOffset" => ('O', 4),
                "shortGeneric" => ('v', 1),
                "longGeneric" => ('v', 4),
                _ => ('z', 1),
            }),
            _ => {}
        }
    }
    (date, time)
}

/// El tipo canónico de un campo de patrón (para comparar con el skeleton).
fn field_type(c: char) -> char {
    match c {
        'L' => 'M',
        'c' | 'e' => 'E',
        'H' | 'K' | 'k' => 'h',
        'b' => 'a',
        other => other,
    }
}

/// El patrón de availableFormats que mejor cubre los campos (los mismos tipos), con los
/// anchos ajustados a los pedidos.
fn match_available(data: &cldr::DateData, want: &[(char, usize)], hc: &str) -> Option<String> {
    if want.is_empty() {
        return None;
    }
    let twelve = matches!(hour_char(hc), 'h' | 'K');
    let mut want_types: Vec<char> = want
        .iter()
        .filter(|(c, _)| !matches!(c, 'z' | 'O' | 'v' | 'S'))
        .map(|(c, _)| field_type(*c))
        .collect();
    want_types.sort();
    want_types.dedup();
    let mut best: Option<(i64, &str)> = None;
    for &(sk, pat) in data.available {
        let sk_fields = pattern_fields(sk);
        let mut types: Vec<char> = sk_fields.iter().map(|(c, _)| field_type(*c)).collect();
        // Las variantes de 12/24 horas: `h` con `a`, `H` sin.
        let has_h = sk_fields.iter().any(|(c, _)| *c == 'h');
        let has_hh = sk_fields.iter().any(|(c, _)| *c == 'H');
        if (has_h && !twelve) || (has_hh && twelve) {
            continue;
        }
        types.retain(|c| *c != 'a' || !want_types.contains(&'h') || twelve);
        types.retain(|c| !matches!(c, 'v' | 'z'));
        types.sort();
        types.dedup();
        let mut wt = want_types.clone();
        if twelve && wt.contains(&'h') && !wt.contains(&'a') {
            wt.push('a');
            wt.sort();
        }
        let mut tt = types.clone();
        if twelve && tt.contains(&'h') && !tt.contains(&'a') {
            tt.push('a');
            tt.sort();
        }
        if tt != wt {
            continue;
        }
        if sk.contains('v') || (sk.contains('B') != want.iter().any(|(c, _)| *c == 'B')) {
            continue;
        }
        // La distancia de anchos.
        let mut score = 0i64;
        for (c, n) in want {
            if let Some((_, m)) = sk_fields
                .iter()
                .find(|(x, _)| field_type(*x) == field_type(*c))
            {
                let text = |k: usize| k >= 3;
                score += if text(*n) != text(*m) {
                    100
                } else {
                    (*n as i64 - *m as i64).abs()
                };
            }
        }
        if best.is_none_or(|(s, _)| score < s) {
            best = Some((score, pat));
        }
    }
    let (_, pat) = best?;
    // Ajustar anchos y la letra de la hora.
    let mut toks = tokenize(pat);
    for t in &mut toks {
        if let PatTok::Field(c, n) = t {
            let ty = field_type(*c);
            if let Some((wc, wn)) = want.iter().find(|(x, _)| field_type(*x) == ty) {
                // Como el generador de patrones de ICU: los campos numéricos conservan el ancho
                // del patrón salvo que se pida uno mayor; los de texto toman el pedido.
                match ty {
                    'h' => {
                        *c = *wc;
                        *n = (*n).max(*wn);
                    }
                    'd' | 'm' | 's' => *n = (*n).max(*wn),
                    'M' if *n < 3 && *wn < 3 => *n = (*n).max(*wn),
                    'M' | 'E' | 'G' => {
                        if (*n >= 3) == (*wn >= 3) || ty != 'M' {
                            *n = *wn;
                        }
                    }
                    'y' => *n = *wn,
                    _ => {}
                }
            }
        }
    }
    Some(untokenize(&toks))
}

/// BestFitFormatMatcher (§11.5.?): fecha y hora por separado, unidas con dateTimeFormats.
fn best_fit_pattern(
    data: &cldr::DateData,
    comps: &[(&'static str, Option<String>)],
    hc: &str,
) -> String {
    let (date, time) = skeleton(comps, hc);
    let date_pat = if date.is_empty() {
        None
    } else {
        Some(match_available(data, &date, hc).unwrap_or_else(|| fallback(&date)))
    };
    let core_time: Vec<(char, usize)> = time
        .iter()
        .copied()
        .filter(|(c, _)| !matches!(c, 'z' | 'O' | 'v' | 'S'))
        .collect();
    let mut time_pat = if core_time.is_empty() {
        None
    } else {
        Some(match_available(data, &core_time, hc).unwrap_or_else(|| fallback(&core_time)))
    };
    // Los segundos fraccionarios después de los segundos y la zona al final.
    if let Some(&(_, n)) = time.iter().find(|(c, _)| *c == 'S') {
        let p = time_pat.get_or_insert_with(String::new);
        if p.contains('s') {
            let mut toks = tokenize(p);
            let at = toks
                .iter()
                .rposition(|t| matches!(t, PatTok::Field('s', _)))
                .map_or(toks.len(), |i| i + 1);
            toks.insert(at, PatTok::Field('S', n));
            toks.insert(at, PatTok::Lit(".".into()));
            *p = untokenize(&toks);
        } else if p.is_empty() {
            *p = "S".repeat(n);
        } else {
            p.push(' ');
            p.push_str(&"S".repeat(n));
        }
    }
    if let Some(&(c, n)) = time.iter().find(|(c, _)| matches!(c, 'z' | 'O' | 'v')) {
        let p = time_pat.get_or_insert_with(String::new);
        if !p.is_empty() {
            p.push(' ');
        }
        p.extend(std::iter::repeat_n(c, n));
    }
    match (date_pat, time_pat) {
        (Some(d), Some(t)) => {
            let month_text = date.iter().any(|(c, n)| *c == 'M' && *n >= 3);
            let long = date.iter().any(|(c, n)| *c == 'M' && *n >= 4);
            let idx = if long && date.iter().any(|(c, _)| *c == 'E') {
                0
            } else if long {
                1
            } else if month_text {
                2
            } else {
                3
            };
            data.date_time_formats[idx]
                .replace("{1}", &d)
                .replace("{0}", &t)
        }
        (Some(d), None) => d,
        (None, Some(t)) => t,
        (None, None) => String::new(),
    }
}

fn fallback(fields: &[(char, usize)]) -> String {
    fields
        .iter()
        .map(|(c, n)| std::iter::repeat_n(*c, *n).collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

// --- Formateo ---

/// Los campos locales de un instante.
struct Fields {
    year: f64,
    month: f64,
    day: f64,
    weekday: f64,
    hour: f64,
    minute: f64,
    second: f64,
    ms: f64,
    offset_ms: f64,
}

fn fields_at(tz: &str, t: f64) -> Fields {
    let offset = zone_offset_ms(tz, t);
    let l = t + offset;
    Fields {
        year: year_from_time(l),
        month: month_from_time(l),
        day: date_from_time(l),
        weekday: week_day(l),
        hour: hour_from_time(l),
        minute: min_from_time(l),
        second: sec_from_time(l),
        ms: ms_from_time(l),
        offset_ms: offset,
    }
}

/// El nombre de la zona para `z`/`O`/`v`: los offsets como `GMT-8` / `GMT-08:00`.
fn zone_name(tz: &str, offset_ms: f64, c: char, n: usize) -> String {
    let utc = time_zone_identifier(tz).is_some_and(|(_, p)| p == "UTC");
    if utc && c != 'O' {
        return if n >= 4 {
            "Coordinated Universal Time".into()
        } else {
            "UTC".into()
        };
    }
    let minutes = (offset_ms / 60_000.0).round() as i64;
    if minutes == 0 {
        return "GMT".into();
    }
    let sign = if minutes < 0 { '-' } else { '+' };
    let m = minutes.abs();
    if n >= 4 {
        format!("GMT{sign}{:02}:{:02}", m / 60, m % 60)
    } else if m % 60 == 0 {
        format!("GMT{sign}{}", m / 60)
    } else {
        format!("GMT{sign}{}:{:02}", m / 60, m % 60)
    }
}

/// El período flexible (`B`) de una hora.
fn flexible_period(data: &cldr::DateData, locale: &str, hour: f64, minute: f64) -> String {
    let es = locale.starts_with("es");
    let _ = minute;
    let name = if es {
        match hour as u32 {
            0..=5 => "morning1",
            6..=11 => "morning2",
            12..=19 => "evening1",
            _ => "night1",
        }
    } else {
        match hour as u32 {
            6..=11 => "morning1",
            12..=17 => "afternoon1",
            18..=20 => "evening1",
            _ => "night1",
        }
    };
    data.flexible_periods
        .iter()
        .find(|p| p.0 == name)
        .map_or(String::new(), |p| p.1.to_owned())
}

/// FormatDateTimePattern (§11.5.?): las partes de `pattern` en el instante `t`.
fn format_pattern(dtf: &DateTimeFormat, pattern: &str, t: f64) -> Vec<(&'static str, String)> {
    let data = &locale::data_for(&dtf.data_locale).dates;
    let f = fields_at(&dtf.time_zone, t);
    let digits = numbering_system_digits(&dtf.numbering_system);
    let num = |v: f64, width: usize| -> String {
        let s = format!("{:0width$}", v as i64, width = width);
        match &digits {
            Some(d) if dtf.numbering_system != "latn" => s
                .chars()
                .map(|c| c.to_digit(10).map_or(c, |x| d[x as usize]))
                .collect(),
            _ => s,
        }
    };
    let mut out = Vec::new();
    for tok in tokenize(pattern) {
        match tok {
            PatTok::Lit(s) => out.push(("literal", s)),
            PatTok::Field(c, n) => {
                let part = match c {
                    'G' => {
                        let era = usize::from(f.year > 0.0);
                        let w = match n {
                            4 => 1,
                            5 => 2,
                            _ => 0,
                        };
                        ("era", data.eras[w][era].to_owned())
                    }
                    'y' => {
                        let y = if f.year <= 0.0 { 1.0 - f.year } else { f.year };
                        if n == 2 {
                            ("year", num(y % 100.0, 2))
                        } else {
                            ("year", num(y, n))
                        }
                    }
                    'M' | 'L' => {
                        let m = f.month as usize;
                        let base = if c == 'L' { 3 } else { 0 };
                        let v = match n {
                            1 => num(f.month + 1.0, 1),
                            2 => num(f.month + 1.0, 2),
                            3 => data.months[base + 1][m].to_owned(),
                            4 => data.months[base][m].to_owned(),
                            _ => data.months[base + 2][m].to_owned(),
                        };
                        ("month", v)
                    }
                    'd' => ("day", num(f.day, n.min(2))),
                    'E' | 'c' | 'e' => {
                        let w = f.weekday as usize;
                        let base = if c == 'c' { 4 } else { 0 };
                        let v = match n {
                            4 => data.days[base][w],
                            5 => data.days[base + 3][w],
                            6 => data.days[base + 2][w],
                            _ => data.days[base + 1][w],
                        };
                        ("weekday", v.to_owned())
                    }
                    'a' | 'b' => {
                        let pm = usize::from(f.hour >= 12.0);
                        let w = match n {
                            4 => 1,
                            5 => 2,
                            _ => 0,
                        };
                        ("dayPeriod", data.day_periods[w][pm].to_owned())
                    }
                    'B' => (
                        "dayPeriod",
                        flexible_period(data, &dtf.data_locale, f.hour, f.minute),
                    ),
                    'h' => {
                        let h = f.hour % 12.0;
                        ("hour", num(if h == 0.0 { 12.0 } else { h }, n.min(2)))
                    }
                    'K' => ("hour", num(f.hour % 12.0, n.min(2))),
                    'H' => ("hour", num(f.hour, n.min(2))),
                    'k' => (
                        "hour",
                        num(if f.hour == 0.0 { 24.0 } else { f.hour }, n.min(2)),
                    ),
                    'm' => ("minute", num(f.minute, n.min(2))),
                    's' => ("second", num(f.second, n.min(2))),
                    'S' => {
                        let s = format!("{:03}", f.ms as i64);
                        (
                            "fractionalSecond",
                            num(s[..n.min(3)].parse().unwrap_or(0.0), n.min(3)),
                        )
                    }
                    'z' | 'O' | 'v' | 'V' => {
                        ("timeZoneName", zone_name(&dtf.time_zone, f.offset_ms, c, n))
                    }
                    _ => ("literal", std::iter::repeat_n(c, n).collect()),
                };
                out.push(part);
            }
        }
    }
    out
}

fn time_value(agent: &mut Agent, v: &Value) -> JsResult<f64> {
    let x = if v.is_undefined() {
        crate::builtins::date::now_ms()
    } else {
        agent.to_number(v)?
    };
    let t = crate::builtins::date::time_clip(x);
    if t.is_nan() {
        return agent.range_error("Invalid time value");
    }
    Ok(t)
}

fn parts_array(agent: &mut Agent, parts: &[(&str, String, Option<&str>)]) -> JsResult<Value> {
    let mut vals = Vec::with_capacity(parts.len());
    for (t, v, source) in parts {
        let o = agent.new_plain_object();
        agent.create_data_property_or_throw(o, PropertyKey::from("type"), str_value(t))?;
        agent.create_data_property_or_throw(o, PropertyKey::from("value"), str_value(v))?;
        if let Some(s) = source {
            agent.create_data_property_or_throw(o, PropertyKey::from("source"), str_value(s))?;
        }
        vals.push(Value::Object(o));
    }
    Ok(Value::Object(agent.create_array_from_list(&vals)?))
}

/// La cadena formateada (FormatDateTime).
pub fn format_string(dtf: &DateTimeFormat, t: f64) -> String {
    format_pattern(dtf, &dtf.pattern, t)
        .into_iter()
        .map(|(_, v)| v)
        .collect()
}

/// PartitionDateTimeRangePattern (§11.5.?).
fn range_parts(
    dtf: &DateTimeFormat,
    x: f64,
    y: f64,
) -> Vec<(&'static str, String, Option<&'static str>)> {
    let data = &locale::data_for(&dtf.data_locale).dates;
    let fx = fields_at(&dtf.time_zone, x);
    let fy = fields_at(&dtf.time_zone, y);
    let fields = pattern_fields(&dtf.pattern);
    let has = |cs: &[char]| fields.iter().any(|(c, _)| cs.contains(c));
    // El campo más grande que cambia, entre los del patrón.
    let checks: [(char, bool, bool); 7] = [
        ('G', (fx.year > 0.0) != (fy.year > 0.0), has(&['G'])),
        ('y', fx.year != fy.year, has(&['y'])),
        ('M', fx.month != fy.month, has(&['M', 'L'])),
        ('d', fx.day != fy.day, has(&['d', 'E', 'c'])),
        (
            'a',
            (fx.hour >= 12.0) != (fy.hour >= 12.0),
            has(&['a', 'h', 'K']),
        ),
        ('h', fx.hour != fy.hour, has(&['h', 'H', 'k', 'K'])),
        ('m', fx.minute != fy.minute, has(&['m'])),
    ];
    let mut greatest: Option<char> = None;
    for (c, differs, present) in checks {
        if differs && (present || greatest.is_none() && matches!(c, 'G' | 'y' | 'M' | 'd')) {
            greatest = Some(c);
            break;
        }
    }
    let second_differs = (fx.second != fy.second && has(&['s'])) || (fx.ms != fy.ms && has(&['S']));
    if greatest.is_none() && !second_differs {
        return format_pattern(dtf, &dtf.pattern, x)
            .into_iter()
            .map(|(t, v)| (t, v, Some("shared")))
            .collect();
    }
    // El patrón de intervalo del skeleton del patrón, si hay.
    let skeleton: String = {
        let mut s: Vec<(char, usize)> = fields
            .iter()
            .filter(|(c, _)| !matches!(c, 'a' | 'b' | 'B'))
            .map(|(c, n)| {
                (
                    if *c == 'L' {
                        'M'
                    } else if *c == 'c' {
                        'E'
                    } else {
                        *c
                    },
                    *n,
                )
            })
            .collect();
        let order = "GyMEdBhHkKmsSzOv";
        s.sort_by_key(|(c, _)| order.find(*c).unwrap_or(99));
        s.iter()
            .map(|(c, n)| std::iter::repeat_n(*c, *n).collect::<String>())
            .collect()
    };
    let field_key = match greatest {
        Some('a') => "a",
        Some('h') => {
            if fields.iter().any(|(c, _)| *c == 'H') {
                "H"
            } else {
                "h"
            }
        }
        Some('m') => "m",
        Some('d') => "d",
        Some('M') => "M",
        Some('y') | Some('G') => "y",
        _ => "",
    };
    if let Some(&(_, _, pat)) = data
        .intervals
        .iter()
        .find(|(sk, f, _)| *sk == skeleton && *f == field_key)
    {
        // El patrón de intervalo: la primera repetición de un campo empieza la segunda parte.
        let toks = tokenize(pat);
        let mut seen: Vec<char> = Vec::new();
        let mut split = toks.len();
        for (i, t) in toks.iter().enumerate() {
            if let PatTok::Field(c, _) = t {
                let ty = field_type(*c);
                if seen.contains(&ty) {
                    split = i;
                    break;
                }
                seen.push(ty);
            }
        }
        // Retroceder sobre el literal que separa.
        let first: String = untokenize(&toks[..split]);
        let second: String = untokenize(&toks[split..]);
        let mut out: Vec<(&'static str, String, Option<&'static str>)> = Vec::new();
        let p1 = format_pattern(dtf, &first, x);
        let p2 = format_pattern(dtf, &second, y);
        // El literal final de la primera parte es el separador compartido.
        let mut p1 = p1;
        let sep = match p1.last() {
            Some(("literal", _)) => p1.pop(),
            _ => None,
        };
        out.extend(p1.into_iter().map(|(t, v)| (t, v, Some("startRange"))));
        if let Some((t, v)) = sep {
            out.push((t, v, Some("shared")));
        }
        out.extend(p2.into_iter().map(|(t, v)| (t, v, Some("endRange"))));
        return out;
    }
    let fallback = data.interval_fallback;
    let (before, rest) = fallback.split_once("{0}").unwrap_or(("", fallback));
    let (mid, after) = rest.split_once("{1}").unwrap_or((rest, ""));
    let mut out: Vec<(&'static str, String, Option<&'static str>)> = Vec::new();
    if !before.is_empty() {
        out.push(("literal", before.to_owned(), Some("shared")));
    }
    out.extend(
        format_pattern(dtf, &dtf.pattern, x)
            .into_iter()
            .map(|(t, v)| (t, v, Some("startRange"))),
    );
    out.push(("literal", mid.to_owned(), Some("shared")));
    out.extend(
        format_pattern(dtf, &dtf.pattern, y)
            .into_iter()
            .map(|(t, v)| (t, v, Some("endRange"))),
    );
    if !after.is_empty() {
        out.push(("literal", after.to_owned(), Some("shared")));
    }
    out
}

// --- Constructor y prototipo ---

/// Intl.DateTimeFormat ( [ locales [ , options ] ] ) (§11.1.1).
fn constructor(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let nt = a.new_target.unwrap_or(a.callee);
    let o = create_date_time_format(
        agent,
        nt,
        &a.arg(0),
        &a.arg(1),
        Required::Any,
        Defaults::Date,
    )?;
    if a.new_target.is_none()
        && let Some(v) = chain_legacy(agent, &a.this, a.callee, o)?
    {
        return Ok(v);
    }
    Ok(Value::Object(o))
}

/// UnwrapDateTimeFormat (§11.5.?).
fn unwrap(agent: &mut Agent, v: &Value, method: &str) -> JsResult<Gc<Object>> {
    let Value::Object(o) = v else {
        return agent.type_error(&format!(
            "Method Intl.DateTimeFormat.prototype.{method} called on incompatible receiver"
        ));
    };
    if matches!(intl_data(agent, v), Some(IntlData::DateTimeFormat(_))) {
        return Ok(*o);
    }
    let realm = agent.current_realm;
    if let Some(ctor) = crate::builtins::web::registry_get(agent, realm, "Intl.DateTimeFormat")
        && agent.ordinary_has_instance(&Value::Object(ctor), v)?
    {
        let sym = crate::builtins::intl::number_format::fallback_symbol(agent);
        let inner = agent.get(*o, &PropertyKey::Symbol(sym), v.clone())?;
        if let Value::Object(i) = inner
            && matches!(intl_data(agent, &inner), Some(IntlData::DateTimeFormat(_)))
        {
            return Ok(i);
        }
    }
    agent.type_error(&format!(
        "Method Intl.DateTimeFormat.prototype.{method} called on incompatible receiver"
    ))
}

fn dtf_of(agent: &Agent, o: Gc<Object>) -> DateTimeFormat {
    match intl_data(agent, &Value::Object(o)) {
        Some(IntlData::DateTimeFormat(d)) => (**d).clone(),
        _ => unreachable!("DateTimeFormat"),
    }
}

fn this_dtf(agent: &mut Agent, this: &Value, method: &str) -> JsResult<Gc<Object>> {
    match (this, intl_data(agent, this)) {
        (Value::Object(o), Some(IntlData::DateTimeFormat(_))) => Ok(*o),
        _ => agent.type_error(&format!(
            "Method Intl.DateTimeFormat.prototype.{method} called on incompatible receiver"
        )),
    }
}

/// get Intl.DateTimeFormat.prototype.format (§11.3.3).
fn get_format(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = unwrap(agent, &a.this, "format")?;
    if let Some(IntlData::DateTimeFormat(d)) = intl_data(agent, &Value::Object(o))
        && let Some(f) = d.bound_format
    {
        return Ok(Value::Object(f));
    }
    let f = agent.create_builtin_with_slots(format_function, 1, "", vec![Value::Object(o)]);
    if let Some(IntlData::DateTimeFormat(d)) = intl_data_mut(agent, o) {
        d.bound_format = Some(f);
    }
    Ok(Value::Object(f))
}

/// DateTime Format Functions (§11.5.?).
fn format_function(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let Some(Value::Object(o)) = crate::builtins::slots(agent, a.callee).into_iter().next() else {
        return Ok(Value::Undefined);
    };
    let dtf = dtf_of(agent, o);
    let t = time_value(agent, &a.arg(0))?;
    Ok(str_value(&format_string(&dtf, t)))
}

/// Intl.DateTimeFormat.prototype.formatToParts ( date ) (§11.3.4).
fn format_to_parts(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_dtf(agent, &a.this, "formatToParts")?;
    let dtf = dtf_of(agent, o);
    let t = time_value(agent, &a.arg(0))?;
    let parts: Vec<(&str, String, Option<&str>)> = format_pattern(&dtf, &dtf.pattern, t)
        .into_iter()
        .map(|(t, v)| (t, v, None))
        .collect();
    parts_array(agent, &parts)
}

fn range_args(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<(f64, f64)> {
    let (s, e) = (a.arg(0), a.arg(1));
    if s.is_undefined() || e.is_undefined() {
        return agent.type_error("startDate and endDate must not be undefined");
    }
    let x = agent.to_number(&s)?;
    let y = agent.to_number(&e)?;
    let x = crate::builtins::date::time_clip(x);
    let y = crate::builtins::date::time_clip(y);
    if x.is_nan() || y.is_nan() {
        return agent.range_error("Invalid time value");
    }
    Ok((x, y))
}

/// Intl.DateTimeFormat.prototype.formatRange ( startDate, endDate ) (§11.3.5).
fn format_range(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_dtf(agent, &a.this, "formatRange")?;
    let dtf = dtf_of(agent, o);
    let (x, y) = range_args(agent, a)?;
    let s: String = range_parts(&dtf, x, y).into_iter().map(|p| p.1).collect();
    Ok(str_value(&s))
}

/// Intl.DateTimeFormat.prototype.formatRangeToParts ( startDate, endDate ) (§11.3.6).
fn format_range_to_parts(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = this_dtf(agent, &a.this, "formatRangeToParts")?;
    let dtf = dtf_of(agent, o);
    let (x, y) = range_args(agent, a)?;
    let parts = range_parts(&dtf, x, y);
    parts_array(agent, &parts)
}

/// Intl.DateTimeFormat.prototype.resolvedOptions ( ) (§11.3.7).
fn resolved_options(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let o = unwrap(agent, &a.this, "resolvedOptions")?;
    let dtf = dtf_of(agent, o);
    let r = agent.new_plain_object();
    let set = |agent: &mut Agent, k: &str, v: Value| {
        agent.create_data_property_or_throw(r, PropertyKey::from(k), v)
    };
    set(agent, "locale", str_value(&dtf.locale))?;
    set(agent, "calendar", str_value(&dtf.calendar))?;
    set(agent, "numberingSystem", str_value(&dtf.numbering_system))?;
    set(agent, "timeZone", str_value(&dtf.time_zone))?;
    if let Some(hc) = &dtf.hour_cycle {
        set(agent, "hourCycle", str_value(hc))?;
        set(agent, "hour12", Value::Boolean(hc == "h11" || hc == "h12"))?;
    }
    if dtf.date_style.is_none() && dtf.time_style.is_none() {
        let fields = pattern_fields(&dtf.pattern);
        let find = |cs: &[char]| fields.iter().find(|(c, _)| cs.contains(c)).copied();
        let text = |n: usize| match n {
            4 => "long",
            5 => "narrow",
            _ => "short",
        };
        let numeric = |n: usize| if n == 2 { "2-digit" } else { "numeric" };
        let entries: [(&str, Option<String>); 11] = [
            (
                "weekday",
                find(&['E', 'c', 'e']).map(|(_, n)| text(n).to_owned()),
            ),
            ("era", find(&['G']).map(|(_, n)| text(n).to_owned())),
            ("year", find(&['y']).map(|(_, n)| numeric(n).to_owned())),
            (
                "month",
                find(&['M', 'L']).map(|(_, n)| match n {
                    1 => "numeric".to_owned(),
                    2 => "2-digit".to_owned(),
                    n => text(n).to_owned(),
                }),
            ),
            ("day", find(&['d']).map(|(_, n)| numeric(n).to_owned())),
            ("dayPeriod", find(&['B']).map(|(_, n)| text(n).to_owned())),
            (
                "hour",
                find(&['h', 'H', 'k', 'K']).map(|(_, n)| numeric(n).to_owned()),
            ),
            ("minute", find(&['m']).map(|(_, n)| numeric(n).to_owned())),
            ("second", find(&['s']).map(|(_, n)| numeric(n).to_owned())),
            (
                "fractionalSecondDigits",
                find(&['S']).map(|(_, n)| n.to_string()),
            ),
            (
                "timeZoneName",
                find(&['z', 'O', 'v']).map(|(c, n)| {
                    match (c, n >= 4) {
                        ('z', false) => "short",
                        ('z', true) => "long",
                        ('O', false) => "shortOffset",
                        ('O', true) => "longOffset",
                        ('v', false) => "shortGeneric",
                        _ => "longGeneric",
                    }
                    .to_owned()
                }),
            ),
        ];
        for (k, v) in entries {
            if let Some(v) = v {
                let val = if k == "fractionalSecondDigits" {
                    Value::Number(v.parse().unwrap_or(1.0))
                } else {
                    str_value(&v)
                };
                set(agent, k, val)?;
            }
        }
    }
    if let Some(d) = &dtf.date_style {
        set(agent, "dateStyle", str_value(d))?;
    }
    if let Some(t) = &dtf.time_style {
        set(agent, "timeStyle", str_value(t))?;
    }
    Ok(Value::Object(r))
}

fn supported_locales_of(agent: &mut Agent, a: &CallArgs<'_>) -> JsResult<Value> {
    let requested = locale::canonicalize_locale_list(agent, &a.arg(0))?;
    locale::supported_locales(agent, &requested, &a.arg(1))
}

/// Date.prototype.toLocaleString / toLocaleDateString / toLocaleTimeString (§20.4.1–3).
pub fn to_locale_string(
    agent: &mut Agent,
    t: f64,
    locales: &Value,
    options: &Value,
    required: Required,
    defaults: Defaults,
) -> JsResult<Value> {
    if t.is_nan() {
        return Ok(str_value("Invalid Date"));
    }
    let ctor =
        crate::builtins::web::registry_get(agent, agent.current_realm, "Intl.DateTimeFormat")
            .expect("Intl.DateTimeFormat");
    let o = create_date_time_format(agent, ctor, locales, options, required, defaults)?;
    let dtf = dtf_of(agent, o);
    Ok(str_value(&format_string(&dtf, t)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dtf(pattern: &str) -> DateTimeFormat {
        DateTimeFormat {
            locale: "en-US".into(),
            data_locale: "en-US".into(),
            calendar: "gregory".into(),
            numbering_system: "latn".into(),
            time_zone: "UTC".into(),
            hour_cycle: None,
            date_style: None,
            time_style: None,
            pattern: pattern.into(),
            bound_format: None,
        }
    }

    #[test]
    fn patterns() {
        let data = &locale::data_for("en").dates;
        let comps = |v: &[(&'static str, &str)]| -> Vec<(&'static str, Option<String>)> {
            COMPONENTS
                .iter()
                .map(|(k, _)| {
                    (
                        *k,
                        v.iter().find(|(x, _)| x == k).map(|(_, y)| (*y).to_owned()),
                    )
                })
                .collect()
        };
        let p = best_fit_pattern(
            data,
            &comps(&[
                ("year", "numeric"),
                ("month", "numeric"),
                ("day", "numeric"),
            ]),
            "h12",
        );
        assert_eq!(p, "M/d/y");
        let p = best_fit_pattern(
            data,
            &comps(&[("hour", "numeric"), ("minute", "2-digit")]),
            "h12",
        );
        assert_eq!(p, "h:mm\u{202f}a");
        let p = best_fit_pattern(
            data,
            &comps(&[("hour", "numeric"), ("minute", "2-digit")]),
            "h23",
        );
        assert_eq!(p, "HH:mm");
        let d = dtf("M/d/y, h:mm:ss a");
        assert_eq!(format_string(&d, 0.0), "1/1/1970, 12:00:00 AM");
        let d = dtf("EEEE, MMMM d, y");
        assert_eq!(
            format_string(&d, 86_400_000.0 * 365.0),
            "Friday, January 1, 1971"
        );
        assert_eq!(
            apply_hour_cycle("h:mm:ss a", "h23"),
            "HH:mm:ss".replace("HH", "H")
        );
    }
}
