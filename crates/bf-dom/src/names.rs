//! Validación de nombres (DOM §1.4) y la producción `Name` de XML 1.0 (para los targets de
//! processing instructions).

use bf_infra::{DomString, Namespace};

use crate::DomException;

fn is_ascii_whitespace(c: u32) -> bool {
    matches!(c, 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

fn is_ascii_alpha(c: u32) -> bool {
    (u32::from(b'a')..=u32::from(b'z')).contains(&c) || (u32::from(b'A')..=u32::from(b'Z')).contains(&c)
}

fn is_ascii_digit(c: u32) -> bool {
    (u32::from(b'0')..=u32::from(b'9')).contains(&c)
}

/// "valid namespace prefix".
pub(crate) fn valid_namespace_prefix(s: &DomString) -> bool {
    !s.is_empty()
        && !s
            .code_points()
            .any(|c| is_ascii_whitespace(c) || matches!(c, 0x00 | 0x2F | 0x3E))
}

/// "valid attribute local name".
pub(crate) fn valid_attribute_local_name(s: &DomString) -> bool {
    !s.is_empty()
        && !s
            .code_points()
            .any(|c| is_ascii_whitespace(c) || matches!(c, 0x00 | 0x2F | 0x3D | 0x3E))
}

/// "valid element local name".
pub(crate) fn valid_element_local_name(s: &DomString) -> bool {
    let mut cps = s.code_points();
    let Some(first) = cps.next() else {
        return false;
    };
    if is_ascii_alpha(first) {
        return !s
            .code_points()
            .any(|c| is_ascii_whitespace(c) || matches!(c, 0x00 | 0x2F | 0x3E));
    }
    if !(first == 0x3A || first == 0x5F || (0x80..=0x10FFFF).contains(&first)) {
        return false;
    }
    cps.all(|c| {
        is_ascii_alpha(c)
            || is_ascii_digit(c)
            || matches!(c, 0x2D | 0x2E | 0x3A | 0x5F)
            || (0x80..=0x10FFFF).contains(&c)
    })
}

/// "valid doctype name".
pub(crate) fn valid_doctype_name(s: &DomString) -> bool {
    !s.code_points()
        .any(|c| is_ascii_whitespace(c) || matches!(c, 0x00 | 0x3E))
}

fn is_name_start_char(c: u32) -> bool {
    matches!(c, 0x3A | 0x5F)
        || is_ascii_alpha(c)
        || (0xC0..=0xD6).contains(&c)
        || (0xD8..=0xF6).contains(&c)
        || (0xF8..=0x2FF).contains(&c)
        || (0x370..=0x37D).contains(&c)
        || (0x37F..=0x1FFF).contains(&c)
        || (0x200C..=0x200D).contains(&c)
        || (0x2070..=0x218F).contains(&c)
        || (0x2C00..=0x2FEF).contains(&c)
        || (0x3001..=0xD7FF).contains(&c)
        || (0xF900..=0xFDCF).contains(&c)
        || (0xFDF0..=0xFFFD).contains(&c)
        || (0x10000..=0xEFFFF).contains(&c)
}

/// La producción `Name` de XML 1.0 (5ª edición).
pub(crate) fn matches_xml_name(s: &DomString) -> bool {
    let mut cps = s.code_points();
    let Some(first) = cps.next() else {
        return false;
    };
    is_name_start_char(first)
        && cps.all(|c| {
            is_name_start_char(c)
                || matches!(c, 0x2D | 0x2E | 0xB7)
                || is_ascii_digit(c)
                || (0x300..=0x36F).contains(&c)
                || (0x203F..=0x2040).contains(&c)
        })
}

/// Contexto de *validate and extract*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameContext {
    Element,
    Attribute,
}

/// "validate and extract" (DOM §1.4): (namespace, prefix, local name).
pub(crate) fn validate_and_extract(
    namespace: Option<&str>,
    qualified_name: &DomString,
    context: NameContext,
) -> Result<(Option<Namespace>, Option<DomString>, DomString), DomException> {
    // 1. El string vacío es "sin namespace".
    let ns = namespace.and_then(Namespace::from_uri);
    let mut prefix = None;
    let mut local = qualified_name.clone();
    let units = qualified_name.to_utf16();
    if let Some(i) = units.iter().position(|&u| u == u16::from(b':')) {
        let p = DomString::from_utf16(&units[..i]);
        local = DomString::from_utf16(&units[i + 1..]);
        if !valid_namespace_prefix(&p) {
            return Err(DomException::InvalidCharacterError);
        }
        prefix = Some(p);
    }
    match context {
        NameContext::Attribute if !valid_attribute_local_name(&local) => {
            return Err(DomException::InvalidCharacterError);
        }
        NameContext::Element if !valid_element_local_name(&local) => {
            return Err(DomException::InvalidCharacterError);
        }
        _ => {}
    }
    if prefix.is_some() && ns.is_none() {
        return Err(DomException::NamespaceError);
    }
    if prefix.as_ref().is_some_and(|p| *p == "xml") && ns != Some(Namespace::Xml) {
        return Err(DomException::NamespaceError);
    }
    let is_xmlns = *qualified_name == "xmlns" || prefix.as_ref().is_some_and(|p| *p == "xmlns");
    if is_xmlns && ns != Some(Namespace::XmlNs) {
        return Err(DomException::NamespaceError);
    }
    if ns == Some(Namespace::XmlNs) && !is_xmlns {
        return Err(DomException::NamespaceError);
    }
    Ok((ns, prefix, local))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_names() {
        let ok = |s: &str| valid_element_local_name(&DomString::from(s));
        assert!(ok("div"));
        assert!(ok("a-b"));
        assert!(ok("é"));
        assert!(ok(":x"));
        assert!(!ok(""));
        assert!(!ok("1a"));
        assert!(!ok("a b"));
        assert!(!ok("-x"));
        assert!(ok("a<"));
    }

    #[test]
    fn extract() {
        let q = DomString::from("svg:rect");
        let (ns, p, l) =
            validate_and_extract(Some(Namespace::Svg.uri()), &q, NameContext::Element).unwrap();
        assert_eq!(ns, Some(Namespace::Svg));
        assert_eq!(p.unwrap(), "svg");
        assert_eq!(l, "rect");
        assert_eq!(
            validate_and_extract(None, &q, NameContext::Element),
            Err(DomException::NamespaceError)
        );
        assert_eq!(
            validate_and_extract(Some("urn:x"), &DomString::from("xmlns"), NameContext::Attribute),
            Err(DomException::NamespaceError)
        );
    }
}
