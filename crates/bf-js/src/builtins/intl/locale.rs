//! Etiquetas de idioma y negociación de locales (ECMA-402 §6.2 y §9.2): la gramática
//! `unicode_locale_id` de UTS #35, IsStructurallyValidLanguageTag,
//! CanonicalizeUnicodeLocaleId (forma canónica y reemplazo de alias de CLDR, UTS #35 Annex C),
//! CanonicalizeLocaleList, BestAvailableLocale, LookupMatcher, ResolveLocale y
//! SupportedLocales.

use crate::builtins::intl::cldr;
use crate::heap::Gc;
use crate::object::Object;
use crate::value::{PropertyKey, Value};
use crate::vm::{Agent, JsResult};

/// Los locales con datos (`en` y `es`, y las variantes regionales que los usan).
pub const AVAILABLE_LOCALES: &[&str] = &[
    "en", "en-GB", "en-US", "es", "es-419", "es-ES", "es-MX", "es-US",
];

/// El valor `null` de una opción en ResolveLocale (p. ej. `hc` cuando hay `hour12`).
pub const NULL_OPTION: &str = "\u{0}";

/// DefaultLocale (§6.2.4).
pub fn default_locale() -> &'static str {
    "en-US"
}

/// Una etiqueta analizada (`unicode_locale_id`), en minúsculas.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LanguageTag {
    pub language: String,
    pub script: Option<String>,
    pub region: Option<String>,
    pub variants: Vec<String>,
    /// La extensión `-u-`: atributos y (clave, valor) en orden.
    pub unicode: Option<UnicodeExtension>,
    /// La extensión `-t-`: tlang y campos.
    pub transformed: Option<TransformedExtension>,
    /// Otras extensiones: (singleton, subtags).
    pub others: Vec<(char, Vec<String>)>,
    pub private_use: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnicodeExtension {
    pub attributes: Vec<String>,
    pub keywords: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransformedExtension {
    pub tlang: Option<Box<LanguageTag>>,
    pub fields: Vec<(String, String)>,
}

fn is_alpha(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_alphabetic())
}

fn is_digit(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_digit())
}

fn is_alnum(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn is_language(s: &str) -> bool {
    is_alpha(s) && matches!(s.len(), 2 | 3 | 5..=8)
}

fn is_script(s: &str) -> bool {
    s.len() == 4 && is_alpha(s)
}

fn is_region(s: &str) -> bool {
    (s.len() == 2 && is_alpha(s)) || (s.len() == 3 && is_digit(s))
}

fn is_variant(s: &str) -> bool {
    is_alnum(s)
        && ((5..=8).contains(&s.len()) || (s.len() == 4 && s.as_bytes()[0].is_ascii_digit()))
}

/// Analiza `unicode_language_id` desde `parts[*i]`.
fn parse_language_id(parts: &[String], i: &mut usize) -> Option<LanguageTag> {
    let language = parts.get(*i)?;
    if !is_language(language) {
        return None;
    }
    let mut tag = LanguageTag {
        language: language.clone(),
        ..LanguageTag::default()
    };
    *i += 1;
    if let Some(s) = parts.get(*i)
        && is_script(s)
    {
        tag.script = Some(s.clone());
        *i += 1;
    }
    if let Some(r) = parts.get(*i)
        && is_region(r)
    {
        tag.region = Some(r.clone());
        *i += 1;
    }
    while let Some(v) = parts.get(*i)
        && is_variant(v)
    {
        // Sin variantes duplicadas (§6.2.1).
        if tag.variants.contains(v) {
            return None;
        }
        tag.variants.push(v.clone());
        *i += 1;
    }
    Some(tag)
}

/// IsStructurallyValidLanguageTag (§6.2.1) + el análisis: `None` si no es válida.
pub fn parse(locale: &str) -> Option<LanguageTag> {
    if locale.is_empty()
        || locale.starts_with('-')
        || locale.ends_with('-')
        || locale.contains("--")
    {
        return None;
    }
    if !locale.is_ascii() {
        return None;
    }
    let parts: Vec<String> = locale.split('-').map(str::to_ascii_lowercase).collect();
    if parts
        .iter()
        .any(|p| p.is_empty() || p.len() > 8 || !is_alnum(p))
    {
        return None;
    }
    let mut i = 0;
    let mut tag = parse_language_id(&parts, &mut i)?;
    let mut singletons: Vec<char> = Vec::new();
    while i < parts.len() {
        let p = &parts[i];
        if p.len() != 1 {
            return None;
        }
        let s = p.chars().next()?;
        if s == 'x' {
            i += 1;
            let rest: Vec<String> = parts[i..].to_vec();
            if rest.is_empty() || rest.iter().any(|r| r.len() > 8) {
                return None;
            }
            tag.private_use = rest;
            return Some(tag);
        }
        if singletons.contains(&s) {
            return None;
        }
        singletons.push(s);
        i += 1;
        match s {
            'u' => {
                let mut ext = UnicodeExtension::default();
                while let Some(a) = parts.get(i)
                    && (3..=8).contains(&a.len())
                {
                    ext.attributes.push(a.clone());
                    i += 1;
                }
                while let Some(k) = parts.get(i)
                    && k.len() == 2
                {
                    let kb = k.as_bytes();
                    if !(kb[0].is_ascii_alphanumeric() && kb[1].is_ascii_alphabetic()) {
                        return None;
                    }
                    i += 1;
                    let mut types = Vec::new();
                    while let Some(t) = parts.get(i)
                        && (3..=8).contains(&t.len())
                    {
                        types.push(t.clone());
                        i += 1;
                    }
                    ext.keywords.push((k.clone(), types.join("-")));
                }
                if ext.attributes.is_empty() && ext.keywords.is_empty() {
                    return None;
                }
                tag.unicode = Some(ext);
            }
            't' => {
                let mut ext = TransformedExtension::default();
                if let Some(p) = parts.get(i)
                    && is_language(p)
                {
                    ext.tlang = Some(Box::new(parse_language_id(&parts, &mut i)?));
                }
                while let Some(k) = parts.get(i)
                    && k.len() == 2
                {
                    let kb = k.as_bytes();
                    if !(kb[0].is_ascii_alphabetic() && kb[1].is_ascii_digit()) {
                        return None;
                    }
                    i += 1;
                    let mut vals = Vec::new();
                    while let Some(v) = parts.get(i)
                        && (3..=8).contains(&v.len())
                    {
                        vals.push(v.clone());
                        i += 1;
                    }
                    if vals.is_empty() {
                        return None;
                    }
                    ext.fields.push((k.clone(), vals.join("-")));
                }
                if ext.tlang.is_none() && ext.fields.is_empty() {
                    return None;
                }
                tag.transformed = Some(ext);
            }
            _ => {
                let mut subtags = Vec::new();
                while let Some(v) = parts.get(i)
                    && (2..=8).contains(&v.len())
                {
                    subtags.push(v.clone());
                    i += 1;
                }
                if subtags.is_empty() {
                    return None;
                }
                tag.others.push((s, subtags));
            }
        }
    }
    Some(tag)
}

/// IsStructurallyValidLanguageTag (§6.2.1).
pub fn is_structurally_valid(locale: &str) -> bool {
    parse(locale).is_some()
}

fn lookup<'a>(table: &'a [(&'static str, &'static str)], key: &str) -> Option<&'a str> {
    table
        .binary_search_by(|(k, _)| (*k).cmp(key))
        .ok()
        .map(|i| table[i].1)
}

/// El reemplazo de alias de UTS #35 Annex C (language, script, region y variantes) sobre un
/// `unicode_language_id`, hasta un punto fijo.
fn replace_aliases(tag: &mut LanguageTag) {
    for _ in 0..16 {
        let mut changed = false;
        // languageAlias: primero con variantes (`art-lojban`), después el idioma solo.
        // (qué se emparejó: 0 variante, 1 región, 2 script, 3 solo idioma).
        let mut matched: Option<(u8, Option<String>, &str)> = None;
        for v in &tag.variants {
            let key = format!("{}-{}", tag.language, v);
            if let Some(r) = lookup(cldr::LANGUAGE_ALIASES, &key) {
                matched = Some((0, Some(v.clone()), r));
                break;
            }
        }
        if matched.is_none()
            && let Some(region) = &tag.region
            && let Some(r) = lookup(
                cldr::LANGUAGE_ALIASES,
                &format!("{}-{}", tag.language, region),
            )
        {
            matched = Some((1, None, r));
        }
        if matched.is_none()
            && let Some(script) = &tag.script
            && let Some(r) = lookup(
                cldr::LANGUAGE_ALIASES,
                &format!("{}-{}", tag.language, script),
            )
        {
            matched = Some((2, None, r));
        }
        if matched.is_none()
            && let Some(r) = lookup(cldr::LANGUAGE_ALIASES, &tag.language)
        {
            matched = Some((3, None, r));
        }
        if let Some((how, variant, replacement)) = matched
            && let Some(rep) = parse(replacement)
        {
            if let Some(v) = variant {
                tag.variants.retain(|x| *x != v);
            }
            tag.language = rep.language;
            if how == 2 || tag.script.is_none() {
                tag.script = rep
                    .script
                    .or(if how == 2 { None } else { tag.script.take() });
            }
            if how == 1 || tag.region.is_none() {
                tag.region = rep
                    .region
                    .or(if how == 1 { None } else { tag.region.take() });
            }
            for v in rep.variants {
                if !tag.variants.contains(&v) {
                    tag.variants.push(v);
                }
            }
            changed = true;
        }
        if let Some(s) = &tag.script {
            let title = title_case(s);
            if let Some(r) = lookup(cldr::SCRIPT_ALIASES, &title) {
                tag.script = Some(r.to_ascii_lowercase());
                changed = true;
            }
        }
        if let Some(r) = &tag.region
            && let Some(rep) = lookup(cldr::REGION_ALIASES, r)
        {
            let options: Vec<&str> = rep.split(' ').collect();
            let chosen = if options.len() == 1 {
                options[0]
            } else {
                // Varios reemplazos: el de la región probable del idioma si está, si no el
                // primero.
                let key = match (&tag.language[..], &tag.script) {
                    ("und", Some(s)) => format!("und-{s}"),
                    (l, _) => l.to_owned(),
                };
                let likely = lookup(cldr::LIKELY_REGION, &key);
                likely
                    .and_then(|l| options.iter().find(|o| o.eq_ignore_ascii_case(l)).copied())
                    .unwrap_or(options[0])
            };
            tag.region = Some(chosen.to_ascii_lowercase());
            changed = true;
        }
        for i in 0..tag.variants.len() {
            if let Some(r) = lookup(cldr::VARIANT_ALIASES, &tag.variants[i]) {
                tag.variants[i] = r.to_ascii_lowercase();
                changed = true;
            }
        }
        tag.variants.sort();
        tag.variants.dedup();
        if !changed {
            break;
        }
    }
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
        None => String::new(),
    }
}

/// El valor canónico de una clave de extensión Unicode (alias de BCP 47, `true` vacío,
/// subdivisiones de `rg`/`sd`).
pub fn canonicalize_keyword_value(key: &str, value: &str) -> String {
    let mut v = value.to_ascii_lowercase();
    if let Ok(i) =
        cldr::KEYWORD_VALUE_ALIASES.binary_search_by(|(k, a, _)| (*k, *a).cmp(&(key, v.as_str())))
    {
        v = cldr::KEYWORD_VALUE_ALIASES[i].2.to_owned();
    }
    if (key == "rg" || key == "sd")
        && let Some(r) = lookup(cldr::SUBDIVISION_ALIASES, &v)
    {
        v = r.split(' ').next().unwrap_or(r).to_owned();
    }
    if v == "true" {
        v.clear();
    }
    v
}

fn canonicalize_tag(tag: &mut LanguageTag) {
    replace_aliases(tag);
    tag.variants.sort();
    if let Some(u) = &mut tag.unicode {
        // Atributos sin repetir y ordenados; claves ordenadas, la primera ocurrencia gana.
        u.attributes.sort();
        u.attributes.dedup();
        let mut kws: Vec<(String, String)> = Vec::new();
        for (k, v) in &u.keywords {
            if kws.iter().any(|(x, _)| x == k) {
                continue;
            }
            kws.push((k.clone(), canonicalize_keyword_value(k, v)));
        }
        kws.sort_by(|a, b| a.0.cmp(&b.0));
        u.keywords = kws;
    }
    if let Some(t) = &mut tag.transformed {
        if let Some(l) = &mut t.tlang {
            canonicalize_tag(l);
        }
        for (k, v) in &mut t.fields {
            let full = format!("t:{k}");
            let canon = canonicalize_keyword_value(&full, v);
            *v = if canon.is_empty() {
                "true".into()
            } else {
                canon
            };
        }
        t.fields.sort_by(|a, b| a.0.cmp(&b.0));
    }
    tag.others.sort_by_key(|(s, _)| *s);
}

impl LanguageTag {
    /// La etiqueta serializada en forma canónica (minúsculas, Script en título, REGIÓN en
    /// mayúsculas).
    pub fn to_string_canonical(&self) -> String {
        let mut out = self.language_id();
        let mut exts: Vec<(char, String)> = Vec::new();
        if let Some(t) = &self.transformed {
            let mut s = String::from("t");
            if let Some(l) = &t.tlang {
                s.push('-');
                s.push_str(&l.language_id().to_ascii_lowercase());
            }
            for (k, v) in &t.fields {
                s.push('-');
                s.push_str(k);
                s.push('-');
                s.push_str(v);
            }
            exts.push(('t', s));
        }
        if let Some(u) = &self.unicode {
            let mut s = String::from("u");
            for a in &u.attributes {
                s.push('-');
                s.push_str(a);
            }
            for (k, v) in &u.keywords {
                s.push('-');
                s.push_str(k);
                if !v.is_empty() {
                    s.push('-');
                    s.push_str(v);
                }
            }
            exts.push(('u', s));
        }
        for (c, subs) in &self.others {
            exts.push((*c, format!("{c}-{}", subs.join("-"))));
        }
        exts.sort_by_key(|(c, _)| *c);
        for (_, s) in exts {
            out.push('-');
            out.push_str(&s);
        }
        if !self.private_use.is_empty() {
            out.push_str("-x-");
            out.push_str(&self.private_use.join("-"));
        }
        out
    }

    /// El `unicode_language_id` en forma canónica.
    pub fn language_id(&self) -> String {
        let mut out = self.language.clone();
        if let Some(s) = &self.script {
            out.push('-');
            out.push_str(&title_case(s));
        }
        if let Some(r) = &self.region {
            out.push('-');
            out.push_str(&r.to_ascii_uppercase());
        }
        for v in &self.variants {
            out.push('-');
            out.push_str(v);
        }
        out
    }
}

/// CanonicalizeUnicodeLocaleId (§6.2.2): `locale` debe ser estructuralmente válida.
pub fn canonicalize(locale: &str) -> String {
    match parse(locale) {
        Some(mut t) => {
            canonicalize_tag(&mut t);
            t.to_string_canonical()
        }
        None => locale.to_owned(),
    }
}

/// CanonicalizeLocaleList (§9.2.1).
pub fn canonicalize_locale_list(agent: &mut Agent, locales: &Value) -> JsResult<Vec<String>> {
    if locales.is_undefined() {
        return Ok(Vec::new());
    }
    let mut seen: Vec<String> = Vec::new();
    let o = match locales {
        Value::String(_) => agent.create_array_from_list(std::slice::from_ref(locales))?,
        Value::Object(o) if locale_object_tag(agent, *o).is_some() => {
            agent.create_array_from_list(std::slice::from_ref(locales))?
        }
        v => agent.to_object(v)?,
    };
    let len = agent.get_str(o, "length")?;
    let len = agent.to_length(&len)?;
    let mut k = 0.0;
    while k < len {
        let pk = PropertyKey::from_number(k);
        if agent.has_property(o, &pk)? {
            let kv = agent.get(o, &pk, Value::Object(o))?;
            let tag = match &kv {
                Value::String(s) => s.to_string_lossy(),
                Value::Object(lo) => match locale_object_tag(agent, *lo) {
                    Some(t) => t,
                    None => agent.to_string(&kv)?.to_string_lossy(),
                },
                _ => return agent.type_error("Language ID should be string or object."),
            };
            if !is_structurally_valid(&tag) {
                return agent.range_error(&format!("Incorrect locale information provided: {tag}"));
            }
            let canon = canonicalize(&tag);
            if !seen.contains(&canon) {
                seen.push(canon);
            }
        }
        k += 1.0;
    }
    Ok(seen)
}

/// La etiqueta de un objeto con `[[InitializedLocale]]` (Intl.Locale; sin implementar aún).
fn locale_object_tag(_: &Agent, _: Gc<Object>) -> Option<String> {
    None
}

/// La etiqueta sin la extensión `-u-` (RemoveUnicodeExtensions).
pub fn remove_unicode_extension(locale: &str) -> (String, Option<String>) {
    let Some(mut t) = parse(locale) else {
        return (locale.to_owned(), None);
    };
    let ext = t.unicode.take().map(|u| {
        let mut s = String::from("-u");
        for a in &u.attributes {
            s.push('-');
            s.push_str(a);
        }
        for (k, v) in &u.keywords {
            s.push('-');
            s.push_str(k);
            if !v.is_empty() {
                s.push('-');
                s.push_str(v);
            }
        }
        s
    });
    (t.to_string_canonical(), ext)
}

/// BestAvailableLocale (§9.2.2).
pub fn best_available_locale(locale: &str) -> Option<&'static str> {
    let mut candidate = locale.to_owned();
    loop {
        if let Some(a) = AVAILABLE_LOCALES
            .iter()
            .find(|a| a.eq_ignore_ascii_case(&candidate))
        {
            return Some(a);
        }
        let mut pos = candidate.rfind('-')?;
        if pos >= 2 && candidate.as_bytes()[pos - 2] == b'-' {
            pos -= 2;
        }
        candidate.truncate(pos);
    }
}

/// El resultado de LookupMatcher (§9.2.3).
pub struct MatcherResult {
    pub locale: String,
    pub extension: Option<UnicodeExtension>,
}

/// LookupMatcher (§9.2.3); BestFitMatcher es el mismo.
pub fn lookup_matcher(requested: &[String]) -> MatcherResult {
    for locale in requested {
        let (no_ext, _) = remove_unicode_extension(locale);
        if let Some(a) = best_available_locale(&no_ext) {
            let extension = parse(locale).and_then(|t| t.unicode);
            return MatcherResult {
                locale: a.to_owned(),
                extension,
            };
        }
    }
    MatcherResult {
        locale: default_locale().to_owned(),
        extension: None,
    }
}

/// El resultado de ResolveLocale (§9.2.7): el locale y el valor de cada clave relevante.
pub struct Resolved {
    pub locale: String,
    pub data_locale: String,
    pub values: Vec<(&'static str, Option<String>)>,
}

impl Resolved {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v.as_deref())
    }
}

/// ResolveLocale (§9.2.7). `options` da el valor de cada clave pedido por opción (o
/// `None`); `supported(key, locale)` la lista de valores de la clave (el primero es el
/// valor por defecto; vacía si la clave no tiene valor por defecto, como `hc`).
pub fn resolve_locale(
    requested: &[String],
    options: &[(&'static str, Option<String>)],
    relevant_keys: &[&'static str],
    supported: &dyn Fn(&str, &str) -> Vec<String>,
) -> Resolved {
    let r = lookup_matcher(requested);
    let found = r.locale.clone();
    let keywords = r.extension.map(|e| e.keywords).unwrap_or_default();
    let mut values = Vec::new();
    let mut supported_keywords: Vec<(String, String)> = Vec::new();
    for &key in relevant_keys {
        let key_data = supported(key, &found);
        let mut value: Option<String> = key_data.first().cloned();
        let mut supported_kw: Option<(String, String)> = None;
        if let Some((_, requested_value)) = keywords.iter().find(|(k, _)| k == key) {
            if !requested_value.is_empty() {
                if key_data.contains(requested_value) {
                    value = Some(requested_value.clone());
                    supported_kw = Some((key.to_owned(), requested_value.clone()));
                }
            } else if key_data.iter().any(|v| v == "true") {
                value = Some("true".into());
                supported_kw = Some((key.to_owned(), String::new()));
            }
        }
        if let Some((_, Some(ov))) = options.iter().find(|(k, _)| *k == key) {
            if ov == NULL_OPTION {
                // null: vale si los datos de la clave lo admiten (el primero, vacío).
                if key_data.first().is_some_and(String::is_empty) {
                    value = key_data.first().cloned();
                    supported_kw = None;
                }
                if let Some(kw) = supported_kw.take() {
                    let _ = kw;
                }
                values.push((key, value));
                continue;
            }
            let mut ov = canonicalize_keyword_value(key, &ov.to_ascii_lowercase());
            if ov.is_empty() {
                ov = "true".into();
            }
            if value.as_deref() != Some(ov.as_str()) && key_data.contains(&ov) {
                value = Some(ov);
                supported_kw = None;
            }
        }
        if let Some(kw) = supported_kw {
            supported_keywords.push(kw);
        }
        values.push((key, value));
    }
    let mut locale = found.clone();
    if !supported_keywords.is_empty() {
        // InsertUnicodeExtensionAndCanonicalize (§6.2.4).
        if let Some(mut t) = parse(&found) {
            t.unicode = Some(UnicodeExtension {
                attributes: Vec::new(),
                keywords: supported_keywords,
            });
            canonicalize_tag(&mut t);
            locale = t.to_string_canonical();
        }
    }
    Resolved {
        locale,
        data_locale: found,
        values,
    }
}

/// SupportedLocales (§9.2.11): los pedidos que tienen un locale disponible.
pub fn supported_locales(
    agent: &mut Agent,
    requested: &[String],
    options: &Value,
) -> JsResult<Value> {
    let o = crate::builtins::intl::coerce_options_to_object(agent, options)?;
    crate::builtins::intl::get_string_option(
        agent,
        o,
        "localeMatcher",
        &["lookup", "best fit"],
        Some("best fit"),
    )?;
    let mut out = Vec::new();
    for l in requested {
        let (no_ext, _) = remove_unicode_extension(l);
        if best_available_locale(&no_ext).is_some() {
            out.push(crate::builtins::web::str_value(l));
        }
    }
    Ok(Value::Object(agent.create_array_from_list(&out)?))
}

/// Los datos de locale para un locale disponible (los regionales usan los del idioma).
pub fn data_for(locale: &str) -> &'static cldr::LocaleData {
    let lang = locale.split('-').next().unwrap_or("en");
    cldr::LOCALE_DATA
        .iter()
        .find(|(l, _)| *l == lang)
        .map_or(cldr::LOCALE_DATA[0].1, |(_, d)| d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_forms() {
        assert_eq!(canonicalize("EN-us"), "en-US");
        assert_eq!(canonicalize("zh-hant-tw"), "zh-Hant-TW");
        assert_eq!(canonicalize("art-lojban"), "jbo");
        assert_eq!(canonicalize("iw"), "he");
        assert_eq!(canonicalize("en-u-ca-islamicc"), "en-u-ca-islamic-civil");
        assert_eq!(canonicalize("en-u-kn-true"), "en-u-kn");
        assert_eq!(canonicalize("en-u-ks-primary"), "en-u-ks-level1");
        assert_eq!(
            canonicalize("de-u-ka-shifted-co-phonebk"),
            "de-u-co-phonebk-ka-shifted"
        );
        assert_eq!(canonicalize("hy-su"), "hy-AM");
        assert_eq!(canonicalize("sl-rozaj-biske-1994"), "sl-1994-biske-rozaj");
        assert!(!is_structurally_valid("en-us-u"));
        assert!(!is_structurally_valid("de-gregory-gregory"));
        assert!(!is_structurally_valid("x-private"));
        assert!(is_structurally_valid("en-x-private"));
    }

    #[test]
    fn matching() {
        assert_eq!(best_available_locale("en-US-posix"), Some("en-US"));
        assert_eq!(best_available_locale("es-AR"), Some("es"));
        assert_eq!(best_available_locale("fr"), None);
        let r = lookup_matcher(&["fr".into(), "es-MX-u-nu-arab".into()]);
        assert_eq!(r.locale, "es-MX");
        assert!(r.extension.is_some());
    }
}
