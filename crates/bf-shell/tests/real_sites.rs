//! Sitios reales con su JavaScript (roadmap 2.3): Google, Wikipedia y Hacker News cargan sin
//! pánicos y el buscador de Google funciona escribiendo en la caja y pulsando Intro.
//!
//! Necesitan red, así que están ignorados por defecto:
//! `cargo test -p bf-shell --test real_sites -- --ignored --test-threads=1`.
//! Cada uno corre `freedom session` (la lógica de la ventana sin abrirla) y falla si hay un
//! pánico, si la pestaña o el motor fallaron, o si no llega la página esperada.

#![forbid(unsafe_code)]

use std::process::Command;

/// Lo que dejó la sesión: la salida completa y la última línea de estado.
struct Session {
    log: String,
    last: String,
}

impl Session {
    /// `título="…"` de la última línea de estado.
    fn title(&self) -> &str {
        field(&self.last, "título=\"").unwrap_or("")
    }

    /// `url=…` de la última línea de estado.
    fn url(&self) -> &str {
        self.last
            .split_once(" url=")
            .and_then(|(_, rest)| rest.split(' ').next())
            .unwrap_or("")
    }
}

fn field<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = line.split_once(prefix)?.1;
    rest.split_once('"').map(|(v, _)| v)
}

fn session(url: &str, steps: &[&str]) -> Session {
    let dir = std::env::temp_dir().join(format!("freedom-test-sitios-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_freedom"))
        .arg("session")
        .arg(url)
        .args(steps)
        .arg("-o")
        .arg(dir.join("paso"))
        .output()
        .expect("se pudo ejecutar freedom");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let log = format!("{stdout}{}", String::from_utf8_lossy(&out.stderr));
    let last = stdout
        .lines()
        .rfind(|l| l.contains(" url="))
        .unwrap_or("")
        .to_owned();
    assert!(
        out.status.success(),
        "freedom session falló con {url}:\n{log}"
    );
    let s = Session { log, last };
    assert_no_crash(&s, url);
    s
}

/// Ni pánicos (ni siquiera capturados), ni la pestaña caída, ni el motor fallando.
fn assert_no_crash(s: &Session, url: &str) {
    for bad in ["panicked", "el hilo falló", "error del motor"] {
        assert!(
            !s.log.contains(bad),
            "{url}: {bad:?} en la salida:\n{}",
            s.log
        );
    }
    for bad in ["Esta página falló", "El motor falló"] {
        assert!(
            !s.title().contains(bad),
            "{url}: página de error {bad:?}:\n{}",
            s.log
        );
    }
}

#[test]
#[ignore = "red: www.google.com"]
fn google_search() {
    let s = session(
        "https://www.google.com/",
        // Los dos `wait` dejan terminar los scripts de la portada (la versión con JavaScript
        // reemplaza los manejadores del formulario después de `load`; en debug tarda).
        &[
            "wait",
            "wait",
            "field:q",
            "type:navegador web",
            "key:enter",
            "wait",
            "wait",
        ],
    );
    let url = s.url();
    assert!(
        url.contains("/search?") && url.contains("q=navegador"),
        "no llegó la página de resultados (url={url}, título={:?}):\n{}",
        s.title(),
        s.log
    );
    assert!(
        s.title().contains("navegador web"),
        "título de resultados inesperado {:?}:\n{}",
        s.title(),
        s.log
    );
}

#[test]
#[ignore = "red: es.wikipedia.org"]
fn wikipedia_article() {
    let s = session(
        "https://es.wikipedia.org/wiki/Navegador_web",
        &["wait", "wait"],
    );
    assert!(
        s.title().contains("Navegador web") && s.title().contains("Wikipedia"),
        "título {:?}:\n{}",
        s.title(),
        s.log
    );
}

#[test]
#[ignore = "red: news.ycombinator.com"]
fn hacker_news() {
    let s = session("https://news.ycombinator.com/", &["wait", "wait"]);
    assert_eq!(s.title(), "Hacker News", "{}", s.log);
}

/// E6: `https://asimetrix.co/` (Next.js). Hoy muestra su propia pantalla de error ("This page
/// couldn't load") porque el motor no tiene `IntersectionObserver` (lo implementa E2); la lista
/// de tropiezos está en `docs/pendiente/mac/e6.md`. Comprueba que la consola no tiene errores
/// (`[pestaña N] …` en stderr) y, con la barra de búsqueda, que la pantalla de error no está.
#[test]
#[ignore = "red: asimetrix.co"]
fn asimetrix() {
    let s = session(
        "https://asimetrix.co/",
        &[
            "wait",
            "wait",
            "wait",
            "wait",
            "key:f3",
            "type:This page couldn",
            "wait",
        ],
    );
    let errors: Vec<&str> = s
        .log
        .lines()
        .filter(|l| l.starts_with("[pestaña"))
        .collect();
    assert!(
        errors.is_empty(),
        "errores de consola: {errors:#?}\n{}",
        s.log
    );
    assert!(
        s.last
            .contains("buscar=\"This page couldn\" \"Sin resultados\""),
        "la pantalla de error del sitio sigue a la vista:\n{}",
        s.log
    );
    assert!(s.title().contains("Asimetrix"), "{}", s.log);
}
