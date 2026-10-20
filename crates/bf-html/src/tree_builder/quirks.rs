//! Determinación de quirks mode a partir del DOCTYPE (13.2.6.4.1, "initial" insertion mode).

use bf_dom::QuirksMode;

use crate::tokenizer::Doctype;

const QUIRKS_PUBLIC_EXACT: &[&str] = &[
    "-//W3O//DTD W3 HTML Strict 3.0//EN//",
    "-/W3C/DTD HTML 4.0 Transitional/EN",
    "HTML",
];

const QUIRKS_SYSTEM_EXACT: &[&str] =
    &["http://www.ibm.com/data/dtd/v11/ibmxhtml1-transitional.dtd"];

const QUIRKS_PUBLIC_PREFIXES: &[&str] = &[
    "+//Silmaril//dtd html Pro v0r11 19970101//",
    "-//AS//DTD HTML 3.0 asWedit + extensions//",
    "-//AdvaSoft Ltd//DTD HTML 3.0 asWedit + extensions//",
    "-//IETF//DTD HTML 2.0 Level 1//",
    "-//IETF//DTD HTML 2.0 Level 2//",
    "-//IETF//DTD HTML 2.0 Strict Level 1//",
    "-//IETF//DTD HTML 2.0 Strict Level 2//",
    "-//IETF//DTD HTML 2.0 Strict//",
    "-//IETF//DTD HTML 2.0//",
    "-//IETF//DTD HTML 2.1E//",
    "-//IETF//DTD HTML 3.0//",
    "-//IETF//DTD HTML 3.2 Final//",
    "-//IETF//DTD HTML 3.2//",
    "-//IETF//DTD HTML 3//",
    "-//IETF//DTD HTML Level 0//",
    "-//IETF//DTD HTML Level 1//",
    "-//IETF//DTD HTML Level 2//",
    "-//IETF//DTD HTML Level 3//",
    "-//IETF//DTD HTML Strict Level 0//",
    "-//IETF//DTD HTML Strict Level 1//",
    "-//IETF//DTD HTML Strict Level 2//",
    "-//IETF//DTD HTML Strict Level 3//",
    "-//IETF//DTD HTML Strict//",
    "-//IETF//DTD HTML//",
    "-//Metrius//DTD Metrius Presentational//",
    "-//Microsoft//DTD Internet Explorer 2.0 HTML Strict//",
    "-//Microsoft//DTD Internet Explorer 2.0 HTML//",
    "-//Microsoft//DTD Internet Explorer 2.0 Tables//",
    "-//Microsoft//DTD Internet Explorer 3.0 HTML Strict//",
    "-//Microsoft//DTD Internet Explorer 3.0 HTML//",
    "-//Microsoft//DTD Internet Explorer 3.0 Tables//",
    "-//Netscape Comm. Corp.//DTD HTML//",
    "-//Netscape Comm. Corp.//DTD Strict HTML//",
    "-//O'Reilly and Associates//DTD HTML 2.0//",
    "-//O'Reilly and Associates//DTD HTML Extended 1.0//",
    "-//O'Reilly and Associates//DTD HTML Extended Relaxed 1.0//",
    "-//SQ//DTD HTML 2.0 HoTMetaL + extensions//",
    "-//SoftQuad Software//DTD HoTMetaL PRO 6.0::19990601::extensions to HTML 4.0//",
    "-//SoftQuad//DTD HoTMetaL PRO 4.0::19971010::extensions to HTML 4.0//",
    "-//Spyglass//DTD HTML 2.0 Extended//",
    "-//Sun Microsystems Corp.//DTD HotJava HTML//",
    "-//Sun Microsystems Corp.//DTD HotJava Strict HTML//",
    "-//W3C//DTD HTML 3 1995-03-24//",
    "-//W3C//DTD HTML 3.2 Draft//",
    "-//W3C//DTD HTML 3.2 Final//",
    "-//W3C//DTD HTML 3.2//",
    "-//W3C//DTD HTML 3.2S Draft//",
    "-//W3C//DTD HTML 4.0 Frameset//",
    "-//W3C//DTD HTML 4.0 Transitional//",
    "-//W3C//DTD HTML Experimental 19960712//",
    "-//W3C//DTD HTML Experimental 970421//",
    "-//W3C//DTD W3 HTML//",
    "-//W3O//DTD W3 HTML 3.0//",
    "-//WebTechs//DTD Mozilla HTML 2.0//",
    "-//WebTechs//DTD Mozilla HTML//",
];

/// Prefijos que dan quirks si falta el system identifier (o es vacío) y limited-quirks si no.
const HTML401_PREFIXES: &[&str] = &[
    "-//W3C//DTD HTML 4.01 Frameset//",
    "-//W3C//DTD HTML 4.01 Transitional//",
];

const LIMITED_QUIRKS_PUBLIC_PREFIXES: &[&str] = &[
    "-//W3C//DTD XHTML 1.0 Frameset//",
    "-//W3C//DTD XHTML 1.0 Transitional//",
];

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len()
        && s.is_char_boundary(prefix.len())
        && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// Modo del documento según el DOCTYPE, cuando no es un iframe srcdoc document y el parser
/// puede cambiar el modo.
pub(super) fn mode_for_doctype(d: &Doctype) -> QuirksMode {
    let public = d.public_id.as_deref();
    let system = d.system_id.as_deref();
    let public_is =
        |list: &[&str]| public.is_some_and(|p| list.iter().any(|x| p.eq_ignore_ascii_case(x)));
    let public_starts =
        |list: &[&str]| public.is_some_and(|p| list.iter().any(|x| starts_with_ci(p, x)));
    let system_missing_or_empty = system.is_none_or(str::is_empty);

    if d.force_quirks
        || d.name.as_deref() != Some("html")
        || public_is(QUIRKS_PUBLIC_EXACT)
        || system.is_some_and(|s| {
            QUIRKS_SYSTEM_EXACT
                .iter()
                .any(|x| s.eq_ignore_ascii_case(x))
        })
        || public_starts(QUIRKS_PUBLIC_PREFIXES)
        || (system_missing_or_empty && public_starts(HTML401_PREFIXES))
    {
        QuirksMode::Quirks
    } else if public_starts(LIMITED_QUIRKS_PUBLIC_PREFIXES)
        || (!system_missing_or_empty && public_starts(HTML401_PREFIXES))
    {
        QuirksMode::LimitedQuirks
    } else {
        QuirksMode::NoQuirks
    }
}
