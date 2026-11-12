//! Referrer Policy (W3C, 2017-01-26 + el *Editor's Draft*): la política de una petición y la
//! cabecera `Referer` que resulta.

use bf_url::Url;

/// §3: las políticas. `Default` es la cadena vacía: la de por defecto de Fetch, que es
/// `strict-origin-when-cross-origin` (§3, Fetch §2.2.5 "request's referrer policy").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReferrerPolicy {
    #[default]
    Default,
    NoReferrer,
    NoReferrerWhenDowngrade,
    SameOrigin,
    Origin,
    StrictOrigin,
    OriginWhenCrossOrigin,
    StrictOriginWhenCrossOrigin,
    UnsafeUrl,
}

impl ReferrerPolicy {
    /// Un *token* de política (§3), sin distinguir mayúsculas; `""` es [`ReferrerPolicy::Default`].
    pub fn parse(s: &str) -> Option<ReferrerPolicy> {
        use ReferrerPolicy::*;
        Some(match s.to_ascii_lowercase().as_str() {
            "" => Default,
            "no-referrer" => NoReferrer,
            "no-referrer-when-downgrade" => NoReferrerWhenDowngrade,
            "same-origin" => SameOrigin,
            "origin" => Origin,
            "strict-origin" => StrictOrigin,
            "origin-when-cross-origin" => OriginWhenCrossOrigin,
            "strict-origin-when-cross-origin" => StrictOriginWhenCrossOrigin,
            "unsafe-url" => UnsafeUrl,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        use ReferrerPolicy::*;
        match self {
            Default => "",
            NoReferrer => "no-referrer",
            NoReferrerWhenDowngrade => "no-referrer-when-downgrade",
            SameOrigin => "same-origin",
            Origin => "origin",
            StrictOrigin => "strict-origin",
            OriginWhenCrossOrigin => "origin-when-cross-origin",
            StrictOriginWhenCrossOrigin => "strict-origin-when-cross-origin",
            UnsafeUrl => "unsafe-url",
        }
    }

    /// §8.1 "Parse a referrer policy from a `Referrer-Policy` header": la cabecera (todas sus
    /// líneas unidas con comas) se parte por comas y gana el último *token* válido; `None` si
    /// no hay ninguno.
    pub fn from_header(value: &str) -> Option<ReferrerPolicy> {
        value
            .split(',')
            .map(|t| t.trim_matches([' ', '\t']))
            .filter(|t| !t.is_empty())
            .filter_map(ReferrerPolicy::parse)
            .next_back()
    }
}

/// §8.4 "Strip url for use as a referrer": sin credenciales ni fragmento y, con `origin_only`,
/// sin ruta ni consulta. Los esquemas que no son `http(s)` no dan referente (paso 2: *local
/// scheme*; Fetch solo manda `Referer` en `http(s)`).
pub fn strip(url: &Url, origin_only: bool) -> Option<Url> {
    if !matches!(url.scheme.as_str(), "http" | "https") {
        return None;
    }
    let mut r = url.clone();
    r.username.clear();
    r.password.clear();
    r.fragment = None;
    if origin_only {
        r.path = bf_url::Path::List(vec![String::new()]);
        r.query = None;
    }
    Some(r)
}

/// §8.3 "Determine request's referrer": la URL que va en `Referer` para una petición a
/// `current` desde `referrer` con `policy`. Pasos 7-8: si la URL completa pasa de 4096 bytes se
/// usa el origen.
pub fn determine(referrer: &Url, current: &Url, policy: ReferrerPolicy) -> Option<Url> {
    use ReferrerPolicy::*;
    let mut full = strip(referrer, false)?;
    let origin = strip(referrer, true)?;
    if full.href().len() > 4096 {
        full = origin.clone();
    }
    let same = referrer.origin() == current.origin();
    // §3: "potentially trustworthy" se reduce aquí a `https:`; un `http:` de destino desde
    // `https:` es una degradación.
    let downgrade = referrer.scheme == "https" && current.scheme != "https";
    match policy {
        NoReferrer => None,
        Origin => Some(origin),
        UnsafeUrl => Some(full),
        StrictOrigin => (!downgrade).then_some(origin),
        SameOrigin => same.then_some(full),
        OriginWhenCrossOrigin => Some(if same { full } else { origin }),
        NoReferrerWhenDowngrade => (!downgrade).then_some(full),
        Default | StrictOriginWhenCrossOrigin if same => Some(full),
        Default | StrictOriginWhenCrossOrigin => (!downgrade).then_some(origin),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s, None).unwrap()
    }

    fn referer(from: &str, to: &str, policy: &str) -> Option<String> {
        let p = ReferrerPolicy::parse(policy).unwrap();
        determine(&url(from), &url(to), p).map(|u| u.href())
    }

    #[test]
    fn header_takes_the_last_valid_token() {
        use ReferrerPolicy::*;
        assert_eq!(
            ReferrerPolicy::from_header("no-referrer, unsafe-url"),
            Some(UnsafeUrl)
        );
        assert_eq!(
            ReferrerPolicy::from_header("unsafe-url, bogus, "),
            Some(UnsafeUrl)
        );
        assert_eq!(ReferrerPolicy::from_header("Origin"), Some(Origin));
        assert_eq!(ReferrerPolicy::from_header("bogus"), None);
        assert_eq!(ReferrerPolicy::from_header(""), None);
    }

    #[test]
    fn every_policy() {
        let a = "https://user:pw@a.example/p?q#f";
        let same = "https://a.example/other";
        let cross = "https://b.example/";
        let down = "http://a.example/";
        let full = Some("https://a.example/p?q".to_owned());
        let origin = Some("https://a.example/".to_owned());
        let cases: [(&str, [Option<String>; 3]); 9] = [
            ("", [full.clone(), origin.clone(), None]),
            (
                "strict-origin-when-cross-origin",
                [full.clone(), origin.clone(), None],
            ),
            ("no-referrer", [None, None, None]),
            (
                "no-referrer-when-downgrade",
                [full.clone(), full.clone(), None],
            ),
            ("same-origin", [full.clone(), None, None]),
            ("origin", [origin.clone(), origin.clone(), origin.clone()]),
            ("strict-origin", [origin.clone(), origin.clone(), None]),
            (
                "origin-when-cross-origin",
                [full.clone(), origin.clone(), origin.clone()],
            ),
            ("unsafe-url", [full.clone(), full.clone(), full.clone()]),
        ];
        for (policy, [s, c, d]) in cases {
            assert_eq!(referer(a, same, policy), s, "{policy} same");
            assert_eq!(referer(a, cross, policy), c, "{policy} cross");
            assert_eq!(referer(a, down, policy), d, "{policy} downgrade");
        }
        assert_eq!(referer("about:blank", cross, "unsafe-url"), None);
    }

    #[test]
    fn long_urls_fall_back_to_the_origin() {
        let long = format!("https://a.example/{}", "x".repeat(5000));
        assert_eq!(
            referer(&long, "https://a.example/", "unsafe-url").as_deref(),
            Some("https://a.example/")
        );
    }
}
