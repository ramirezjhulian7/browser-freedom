//! Aislamiento entre orígenes (`crossOriginIsolated`) a partir de las cabeceras
//! `Cross-Origin-Opener-Policy` (HTML §7.1.3) y `Cross-Origin-Embedder-Policy` (HTML §7.1.4).
//!
//! - Documento: el embebedor llama a [`set_document_policies`] con la URL y las cabeceras de la
//!   respuesta de la navegación. Solo cuentan en un contexto seguro (§7.1.3.2 "obtain a
//!   cross-origin opener policy" paso 3 y §7.1.4.4 "obtain an embedder policy" paso 2). Con
//!   COOP `same-origin` y un COEP compatible con el aislamiento, la política del documento es
//!   `same-origin-plus-COEP` y su grupo de contextos de navegación queda aislado; de los dos
//!   modos que permite el estándar ("logical" o "concrete", a elección de la implementación),
//!   aquí siempre es "concrete", así que `crossOriginIsolated` es `true`.
//! - Workers (§10.2.4 "run a worker", pasos de *processResponseConsumeBody*): ver
//!   [`worker_policies`].

use bf_infra::Url;
use bf_js::vm::{Agent, RealmId};

use super::Isolation;

/// El valor de una política de apertura (HTML §7.1.3.1), sin los de informe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenerPolicy {
    #[default]
    UnsafeNone,
    SameOriginAllowPopups,
    SameOrigin,
    SameOriginPlusCoep,
    NoopenerAllowPopups,
}

/// El valor de una política de inserción (HTML §7.1.4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmbedderPolicy {
    #[default]
    UnsafeNone,
    RequireCorp,
    Credentialless,
}

impl EmbedderPolicy {
    /// "Compatible with cross-origin isolation" (§7.1.4.1): `credentialless` o `require-corp`.
    pub fn is_compatible_with_isolation(self) -> bool {
        matches!(
            self,
            EmbedderPolicy::RequireCorp | EmbedderPolicy::Credentialless
        )
    }
}

/// La política de inserción de un documento (dato del realm; sin él, `unsafe-none`).
#[derive(Debug, Clone, Copy, Default)]
pub struct DocumentEmbedderPolicy(pub EmbedderPolicy);

/// "Potentially trustworthy URL" (Secure Contexts §3.1), lo que usa `isSecureContext`.
pub fn is_potentially_trustworthy(url: &Url) -> bool {
    let host = url.hostname();
    matches!(url.scheme.as_str(), "https" | "wss" | "file" | "about")
        || host == "localhost"
        || host.ends_with(".localhost")
        || host == "127.0.0.1"
        || host == "[::1]"
}

/// Un token de Structured Fields (RFC 8941 §3.3.4): `( ALPHA / "*" ) *( tchar / ":" / "/" )`.
fn is_sf_token(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '*')
        && chars.all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~:/".contains(c))
}

/// "Get a structured field value" de tipo *item* (RFC 8941 §4.2): el token de la cabecera, sin
/// sus parámetros; `None` si no es un item con un token (incluidas las listas: dos valores
/// combinados con `,` no son un item).
fn sf_token_item(value: &str) -> Option<&str> {
    let value = value.trim_matches(' ');
    let (item, params) = match value.find(';') {
        Some(i) => (&value[..i], &value[i..]),
        None => (value, ""),
    };
    if !is_sf_token(item) {
        return None;
    }
    // Los parámetros: `;` OWS clave [ "=" valor ]. Basta con que no haya otro miembro.
    if params.contains(',') {
        return None;
    }
    for p in params.split(';').skip(1) {
        let key = p.trim_start_matches(' ').split('=').next().unwrap_or("");
        let ok = key
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '*')
            && key
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_-.*".contains(c));
        if !ok {
            return None;
        }
    }
    Some(item)
}

/// El valor de `Cross-Origin-Embedder-Policy` (§7.1.4.4 paso 3); sin cabecera o inválida,
/// `unsafe-none`.
pub fn parse_embedder_policy(header: Option<&str>) -> EmbedderPolicy {
    match header.and_then(sf_token_item) {
        Some("require-corp") => EmbedderPolicy::RequireCorp,
        Some("credentialless") => EmbedderPolicy::Credentialless,
        _ => EmbedderPolicy::UnsafeNone,
    }
}

/// El valor de `Cross-Origin-Opener-Policy` (§7.1.3.2 pasos 4–5) dada la política de
/// inserción de la misma respuesta.
pub fn parse_opener_policy(header: Option<&str>, coep: EmbedderPolicy) -> OpenerPolicy {
    match header.and_then(sf_token_item) {
        Some("same-origin") if coep.is_compatible_with_isolation() => {
            OpenerPolicy::SameOriginPlusCoep
        }
        Some("same-origin") => OpenerPolicy::SameOrigin,
        Some("same-origin-allow-popups") => OpenerPolicy::SameOriginAllowPopups,
        Some("noopener-allow-popups") => OpenerPolicy::NoopenerAllowPopups,
        _ => OpenerPolicy::UnsafeNone,
    }
}

/// Las políticas de un documento cargado de `url` con esas cabeceras, y si queda aislado.
/// Fuera de un contexto seguro, las cabeceras no cuentan.
pub fn document_policies(
    url: &Url,
    coop: Option<&str>,
    coep: Option<&str>,
) -> (OpenerPolicy, EmbedderPolicy, bool) {
    if !is_potentially_trustworthy(url) {
        return (OpenerPolicy::UnsafeNone, EmbedderPolicy::UnsafeNone, false);
    }
    let embedder = parse_embedder_policy(coep);
    let opener = parse_opener_policy(coop, embedder);
    (opener, embedder, opener == OpenerPolicy::SameOriginPlusCoep)
}

/// El embebedor, al crear el documento de una navegación: guarda en `realm` su política de
/// inserción y si es `crossOriginIsolated` ([`Isolation`]). `coop` y `coep` son los valores
/// de las cabeceras de la respuesta (`None` si no vinieron).
pub fn set_document_policies(
    agent: &mut Agent,
    realm: RealmId,
    url: &Url,
    coop: Option<&str>,
    coep: Option<&str>,
) -> bool {
    let (_, embedder, isolated) = document_policies(url, coop, coep);
    agent.set_realm_data(realm, DocumentEmbedderPolicy(embedder));
    agent.set_realm_data(realm, Isolation(isolated));
    isolated
}

/// ¿Es un esquema local (Fetch §2.1: `about`, `blob`, `data`)? Sus workers heredan la
/// política del dueño.
fn is_local_scheme(url: &Url) -> bool {
    matches!(url.scheme.as_str(), "about" | "blob" | "data")
}

/// Lo que resulta de la respuesta del script principal de un worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerPolicies {
    pub embedder: EmbedderPolicy,
    pub cross_origin_isolated: bool,
}

/// HTML §10.2.4 "run a worker", *processResponseConsumeBody* pasos 4–10, para el script de
/// `url` con la cabecera COEP `coep`: la política del worker (§7.1.4.5 "initialize a worker
/// global scope's policy container": la del dueño con un esquema local), el chequeo de
/// §7.1.4.6 "check a global object's embedder policy" (`None`: error de red, solo para los
/// dedicados) y el *cross-origin isolated capability*.
///
/// - Dedicado: el del dueño, salvo con un script `data:` (paso 10).
/// - Compartido: su agent cluster es nuevo; queda aislado si su propia política es
///   compatible y el script viene de un contexto seguro (paso 6, modo "concrete").
pub fn worker_policies(
    url: &Url,
    coep: Option<&str>,
    shared: bool,
    owner: EmbedderPolicy,
    owner_isolated: bool,
) -> Option<WorkerPolicies> {
    let embedder = if is_local_scheme(url) {
        owner
    } else if is_potentially_trustworthy(url) {
        parse_embedder_policy(coep)
    } else {
        EmbedderPolicy::UnsafeNone
    };
    if !shared
        && !is_local_scheme(url)
        && owner.is_compatible_with_isolation()
        && !embedder.is_compatible_with_isolation()
    {
        return None;
    }
    let cross_origin_isolated = if shared {
        embedder.is_compatible_with_isolation() && is_potentially_trustworthy(url)
    } else {
        owner_isolated && url.scheme != "data"
    };
    Some(WorkerPolicies {
        embedder,
        cross_origin_isolated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s, None).expect("url")
    }

    #[test]
    fn parses_header_items() {
        use EmbedderPolicy::*;
        assert_eq!(parse_embedder_policy(Some("require-corp")), RequireCorp);
        assert_eq!(
            parse_embedder_policy(Some(" credentialless ")),
            Credentialless
        );
        assert_eq!(
            parse_embedder_policy(Some("require-corp; report-to=\"x\"")),
            RequireCorp
        );
        assert_eq!(parse_embedder_policy(Some("\"require-corp\"")), UnsafeNone);
        assert_eq!(
            parse_embedder_policy(Some("require-corp, credentialless")),
            UnsafeNone
        );
        assert_eq!(parse_embedder_policy(Some("Require-Corp")), UnsafeNone);
        assert_eq!(parse_embedder_policy(None), UnsafeNone);
        assert_eq!(
            parse_opener_policy(Some("same-origin"), RequireCorp),
            OpenerPolicy::SameOriginPlusCoep
        );
        assert_eq!(
            parse_opener_policy(Some("same-origin"), UnsafeNone),
            OpenerPolicy::SameOrigin
        );
        assert_eq!(
            parse_opener_policy(Some("same-origin-allow-popups"), RequireCorp),
            OpenerPolicy::SameOriginAllowPopups
        );
    }

    #[test]
    fn document_isolation_needs_both_headers_and_a_secure_context() {
        let https = url("https://a.test/");
        let iso = |coop, coep| document_policies(&https, coop, coep).2;
        assert!(iso(Some("same-origin"), Some("require-corp")));
        assert!(iso(Some("same-origin"), Some("credentialless")));
        assert!(!iso(Some("same-origin"), None));
        assert!(!iso(None, Some("require-corp")));
        assert!(!iso(Some("same-origin-allow-popups"), Some("require-corp")));
        let http = url("http://a.test/");
        assert!(!document_policies(&http, Some("same-origin"), Some("require-corp")).2);
        let local = url("http://localhost:8000/");
        assert!(document_policies(&local, Some("same-origin"), Some("require-corp")).2);
    }

    #[test]
    fn worker_isolation() {
        use EmbedderPolicy::*;
        let https = url("https://a.test/w.js");
        // Dedicado: hereda del dueño; con un dueño `require-corp`, el script debe serlo.
        assert_eq!(
            worker_policies(&https, Some("require-corp"), false, RequireCorp, true),
            Some(WorkerPolicies {
                embedder: RequireCorp,
                cross_origin_isolated: true
            })
        );
        assert_eq!(
            worker_policies(&https, None, false, RequireCorp, true),
            None
        );
        assert!(
            !worker_policies(&https, Some("require-corp"), false, UnsafeNone, false)
                .unwrap()
                .cross_origin_isolated
        );
        // `blob:` hereda la política; `data:` nunca está aislado.
        let blob = url("blob:https://a.test/0b1f");
        assert_eq!(
            worker_policies(&blob, None, false, RequireCorp, true),
            Some(WorkerPolicies {
                embedder: RequireCorp,
                cross_origin_isolated: true
            })
        );
        let data = url("data:text/javascript,1");
        assert!(
            !worker_policies(&data, None, false, RequireCorp, true)
                .unwrap()
                .cross_origin_isolated
        );
        // Compartido: según su propia respuesta, sin importar el dueño.
        assert!(
            worker_policies(&https, Some("require-corp"), true, UnsafeNone, false)
                .unwrap()
                .cross_origin_isolated
        );
        assert!(
            !worker_policies(&https, None, true, RequireCorp, true)
                .unwrap()
                .cross_origin_isolated
        );
        let http = url("http://a.test/w.js");
        assert!(
            !worker_policies(&http, Some("require-corp"), true, UnsafeNone, false)
                .unwrap()
                .cross_origin_isolated
        );
    }
}
