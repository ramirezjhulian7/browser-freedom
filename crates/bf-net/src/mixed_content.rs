//! Mixed Content Level 2 (W3C): en un documento cargado por una conexión segura, las
//! peticiones inseguras (`http:`, `ws:`) de contenido activo se bloquean y las de imágenes,
//! audio y vídeo se pasan a `https:` (§4.1 "Upgrade a mixed content request to a potentially
//! trustworthy URL, if appropriate"; §4.4 "Should fetching request be blocked as mixed
//! content?").
//!
//! "Potentially trustworthy" (Secure Contexts §3.1): `https:`, `wss:`, `file:`, `data:`,
//! `blob:`, `about:blank`/`about:srcdoc`, y `localhost`, `*.localhost`, `127.0.0.0/8` y `::1`.

use bf_infra::{Destination, Url};

/// Qué hacer con una petición.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Se pide por `https:` en lugar de `http:` (imágenes, audio, vídeo).
    Upgrade(Url),
    Block,
}

/// ¿El host es de bucle local (`localhost`, `*.localhost`, `127.0.0.0/8`, `::1`)?
fn loopback(url: &Url) -> bool {
    let host = url.hostname().to_ascii_lowercase();
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host == "localhost"
        || host.ends_with(".localhost")
        || host == "::1"
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.octets()[0] == 127)
}

/// "Is url potentially trustworthy?" (Secure Contexts §3.1).
pub fn potentially_trustworthy(url: &Url) -> bool {
    match url.scheme.as_str() {
        "https" | "wss" | "file" | "data" | "blob" => true,
        "about" => matches!(url.path_string().as_str(), "blank" | "srcdoc"),
        _ => loopback(url),
    }
}

/// "Does settings prohibit mixed security contexts?" (§4.3): el documento llegó por una
/// conexión segura.
pub fn prohibits_mixed_content(document: &Url) -> bool {
    matches!(document.scheme.as_str(), "https" | "wss")
}

/// ¿Es contenido que se puede pasar a `https:` (§3: *upgradeable*)? Imágenes, audio y vídeo.
pub fn upgradeable(destination: Destination) -> bool {
    matches!(
        destination,
        Destination::Image | Destination::Audio | Destination::Video
    )
}

/// La decisión para una petición de `request` con `destination` desde `document`.
pub fn check(document: &Url, request: &Url, destination: Destination) -> Decision {
    if !prohibits_mixed_content(document) || potentially_trustworthy(request) {
        return Decision::Allow;
    }
    if upgradeable(destination) && request.scheme == "http" {
        let mut up = request.clone();
        up.scheme = "https".into();
        // §4.1 paso 2.2: el puerto 80 pasa a ser el de `https:`.
        if up.port == Some(80) {
            up.port = None;
        }
        return Decision::Upgrade(up);
    }
    Decision::Block
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s, None).unwrap()
    }

    #[test]
    fn decisions() {
        let secure = url("https://sitio.test/");
        let table: &[(&str, &str, Destination, Decision)] = &[
            (
                "https://sitio.test/",
                "http://x.test/a.js",
                Destination::Script,
                Decision::Block,
            ),
            (
                "https://sitio.test/",
                "http://x.test/a.css",
                Destination::Style,
                Decision::Block,
            ),
            (
                "https://sitio.test/",
                "http://x.test/f",
                Destination::Frame,
                Decision::Block,
            ),
            (
                "https://sitio.test/",
                "http://x.test/api",
                Destination::Connect,
                Decision::Block,
            ),
            (
                "https://sitio.test/",
                "http://x.test/f.woff",
                Destination::Font,
                Decision::Block,
            ),
            (
                "https://sitio.test/",
                "https://x.test/a.js",
                Destination::Script,
                Decision::Allow,
            ),
            (
                "https://sitio.test/",
                "http://localhost:8000/a.js",
                Destination::Script,
                Decision::Allow,
            ),
            (
                "https://sitio.test/",
                "http://127.0.0.1/a.js",
                Destination::Script,
                Decision::Allow,
            ),
            (
                "https://sitio.test/",
                "http://dev.localhost/a.js",
                Destination::Script,
                Decision::Allow,
            ),
            (
                "https://sitio.test/",
                "data:text/javascript,1",
                Destination::Script,
                Decision::Allow,
            ),
            (
                "http://sitio.test/",
                "http://x.test/a.js",
                Destination::Script,
                Decision::Allow,
            ),
            (
                "file:///tmp/a.html",
                "http://x.test/a.js",
                Destination::Script,
                Decision::Allow,
            ),
        ];
        for (doc, req, dest, want) in table {
            assert_eq!(
                check(&url(doc), &url(req), *dest),
                *want,
                "{doc} {req} {dest:?}"
            );
        }
        assert_eq!(
            check(&secure, &url("http://x.test/a.png"), Destination::Image),
            Decision::Upgrade(url("https://x.test/a.png"))
        );
        assert_eq!(
            check(&secure, &url("http://x.test:80/v.mp4"), Destination::Video),
            Decision::Upgrade(url("https://x.test/v.mp4"))
        );
        assert_eq!(
            check(
                &secure,
                &url("http://x.test:8080/a.png"),
                Destination::Image
            ),
            Decision::Upgrade(url("https://x.test:8080/a.png"))
        );
        assert!(potentially_trustworthy(&url("about:blank")));
        assert!(!potentially_trustworthy(&url("http://ejemplo.test/")));
    }
}
