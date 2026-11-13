//! Las políticas de seguridad de un documento ([`DocumentPolicy`]): Mixed Content
//! ([`crate::mixed_content`]) y Content Security Policy ([`crate::csp`]). La comparten el
//! loader de los subrecursos (en cualquier hilo), los scripts del documento y `fetch`/XHR;
//! cada decisión que bloquea o solo informa deja un mensaje para la consola, que recoge quien
//! la tiene ([`DocumentPolicy::take_messages`]).

use std::sync::Mutex;

use bf_infra::{Destination, Url};

use crate::csp::{self, Disposition, Inline, Policy, Source};
use crate::mixed_content::{self, Decision};

/// Las políticas del documento de `url`.
#[derive(Debug)]
pub struct DocumentPolicy {
    url: Url,
    policies: Mutex<Vec<Policy>>,
    messages: Mutex<Vec<String>>,
}

fn destination_name(d: Destination) -> &'static str {
    match d {
        Destination::Script => "el script",
        Destination::Style => "la hoja de estilo",
        Destination::Image => "la imagen",
        Destination::Font => "la fuente",
        Destination::Frame => "el marco",
        Destination::Audio => "el audio",
        Destination::Video => "el vídeo",
        Destination::Connect => "la conexión",
        Destination::Object => "el objeto",
        Destination::Worker => "el worker",
        Destination::Other => "el recurso",
    }
}

fn inline_name(kind: Inline) -> &'static str {
    match kind {
        Inline::Script => "un script en línea",
        Inline::ScriptAttribute => "un manejador de eventos en línea",
        Inline::Style => "un estilo en línea",
        Inline::StyleAttribute => "un atributo style",
    }
}

impl DocumentPolicy {
    pub fn new(url: Url) -> DocumentPolicy {
        DocumentPolicy {
            url,
            policies: Mutex::default(),
            messages: Mutex::default(),
        }
    }

    /// La URL del documento (su origen es el `'self'` de CSP).
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Las cabeceras de la respuesta del documento: `Content-Security-Policy` (cada valor,
    /// §3.1) y `Content-Security-Policy-Report-Only` (§3.2).
    pub fn add_headers<'a>(&self, headers: impl IntoIterator<Item = (&'a str, &'a str)>) {
        let mut list = self.policies.lock().unwrap_or_else(|e| e.into_inner());
        for (name, value) in headers {
            let disposition = if name.eq_ignore_ascii_case("content-security-policy") {
                Disposition::Enforce
            } else if name.eq_ignore_ascii_case("content-security-policy-report-only") {
                Disposition::Report
            } else {
                continue;
            };
            list.extend(csp::parse_list(value, disposition));
        }
    }

    /// Una política de `<meta http-equiv="Content-Security-Policy">` (§3.3): siempre se
    /// aplica (no hay *report-only* en `<meta>`).
    pub fn add_meta(&self, content: &str) {
        let p = csp::parse_policy(content, Disposition::Enforce, Source::Meta);
        if !p.directives.is_empty() {
            self.policies
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(p);
        }
    }

    /// ¿Tiene alguna política CSP?
    pub fn has_csp(&self) -> bool {
        !self
            .policies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }

    fn note(&self, message: String) {
        self.messages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(message);
    }

    /// Los mensajes para la consola desde la última vez.
    pub fn take_messages(&self) -> Vec<String> {
        std::mem::take(&mut *self.messages.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Anota las violaciones; `true` si alguna se aplica (bloquea).
    fn report(&self, violations: Vec<csp::Violation>, what: &str) -> bool {
        let mut blocked = false;
        for v in violations {
            match v.disposition {
                Disposition::Enforce => {
                    blocked = true;
                    self.note(format!(
                        "Content Security Policy: se bloqueó {what} porque no lo permite \
                         la directiva \"{}\".",
                        v.directive
                    ));
                }
                Disposition::Report => self.note(format!(
                    "Content Security Policy (solo informe): {what} no cumple la directiva \
                     \"{}\".",
                    v.directive
                )),
            }
        }
        blocked
    }

    /// ¿Se puede pedir `url` para `destination`? `Some` con la URL que se pide (pasada a
    /// `https:` si es contenido mixto que se puede subir), `None` si se bloquea. `nonce` es el
    /// del elemento que hace la petición, si tiene.
    pub fn check_request(
        &self,
        url: &Url,
        destination: Destination,
        nonce: Option<&str>,
    ) -> Option<Url> {
        self.check_request_by(url, destination, nonce, true)
    }

    /// Como [`DocumentPolicy::check_request`]; `parser_inserted` es falso para los scripts que
    /// inserta otro script (`'strict-dynamic'` los permite).
    pub fn check_request_by(
        &self,
        url: &Url,
        destination: Destination,
        nonce: Option<&str>,
        parser_inserted: bool,
    ) -> Option<Url> {
        let what = destination_name(destination);
        let url = match mixed_content::check(&self.url, url, destination) {
            Decision::Allow => url.clone(),
            Decision::Upgrade(up) => {
                self.note(format!(
                    "Contenido mixto: la página segura {} pidió {what} inseguro '{}'; se pide \
                     por https.",
                    self.url.href(),
                    url.href()
                ));
                up
            }
            Decision::Block => {
                self.note(format!(
                    "Contenido mixto: la página segura {} pidió {what} inseguro '{}'; se \
                     bloqueó.",
                    self.url.href(),
                    url.href()
                ));
                return None;
            }
        };
        let violations: Vec<_> = self
            .policies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter_map(|p| {
                p.request_violation_by(&url, destination, &self.url, nonce, parser_inserted)
            })
            .collect();
        let blocked = self.report(violations, &format!("{what} '{}'", url.href()));
        (!blocked).then_some(url)
    }

    /// ¿Se puede ejecutar o aplicar en línea `source` (con el `nonce` de su elemento)?
    pub fn allow_inline(&self, kind: Inline, nonce: Option<&str>, source: &str) -> bool {
        self.allow_inline_by(kind, nonce, source, true)
    }

    /// Como [`DocumentPolicy::allow_inline`]; `parser_inserted` es falso para un script en
    /// línea que insertó otro script (`'strict-dynamic'` lo permite).
    pub fn allow_inline_by(
        &self,
        kind: Inline,
        nonce: Option<&str>,
        source: &str,
        parser_inserted: bool,
    ) -> bool {
        let violations: Vec<_> = self
            .policies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter_map(|p| p.inline_violation_by(kind, nonce, source, parser_inserted))
            .collect();
        !self.report(violations, inline_name(kind))
    }

    /// §4.4 para `eval`/`new Function`: `Err` con el mensaje si alguna política lo prohíbe; las
    /// de solo informe dejan su mensaje y lo permiten.
    pub fn check_eval(&self) -> Result<(), String> {
        let violations: Vec<_> = self
            .policies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter_map(Policy::eval_violation)
            .collect();
        let enforced = violations
            .iter()
            .find(|v| v.disposition == Disposition::Enforce)
            .map(|v| v.directive.clone());
        self.report(violations, "evaluar texto como código (eval)");
        match enforced {
            Some(d) => Err(format!(
                "Content Security Policy: no se permite evaluar texto como código (\"{d}\" no \
                 tiene 'unsafe-eval')"
            )),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s, None).unwrap()
    }

    #[test]
    fn mixed_content_then_csp() {
        let p = DocumentPolicy::new(url("https://sitio.test/"));
        assert!(
            p.check_request(&url("http://x.test/a.js"), Destination::Script, None)
                .is_none()
        );
        let msgs = p.take_messages();
        assert!(
            msgs[0].contains("http://x.test/a.js") && msgs[0].contains("bloqueó"),
            "{msgs:?}"
        );
        assert_eq!(
            p.check_request(&url("http://x.test/a.png"), Destination::Image, None),
            Some(url("https://x.test/a.png"))
        );
        p.add_headers([("Content-Security-Policy", "img-src 'self'")]);
        // La subida a https no salva a la imagen de CSP.
        assert!(
            p.check_request(&url("http://x.test/a.png"), Destination::Image, None)
                .is_none()
        );
        assert!(
            p.check_request(&url("https://sitio.test/a.png"), Destination::Image, None)
                .is_some()
        );
    }

    #[test]
    fn report_only_and_meta() {
        let p = DocumentPolicy::new(url("http://sitio.test/"));
        p.add_headers([("content-security-policy-report-only", "script-src 'none'")]);
        assert!(p.allow_inline(Inline::Script, None, "x()"));
        assert!(p.check_eval().is_ok());
        let msgs = p.take_messages();
        assert_eq!(msgs.len(), 2);
        assert!(msgs.iter().all(|m| m.contains("solo informe")), "{msgs:?}");
        p.add_meta("script-src 'self' 'nonce-k'");
        assert!(!p.allow_inline(Inline::Script, None, "x()"));
        assert!(p.allow_inline(Inline::Script, Some("k"), "x()"));
        assert!(p.check_eval().is_err());
        assert!(p.has_csp());
    }
}
