//! La capa de datos del historial persistente (`src/history.rs`): lo que se guarda en el perfil
//! sobrevive a cerrar y abrir el navegador. Los módulos se incluyen por ruta porque `bf-shell`
//! es solo un binario.

#![forbid(unsafe_code)]
#![allow(dead_code)]

#[path = "../src/find.rs"]
mod find;
#[path = "../src/history.rs"]
mod history;
#[path = "../src/profile.rs"]
mod profile;

use std::path::PathBuf;

use history::History;

/// Una carpeta de perfil vacía y propia de cada prueba.
fn temp_profile(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("freedom-e11-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn survives_close_and_open() {
    let path = temp_profile("persist").join("history");
    {
        let mut h = History::open(path.clone());
        assert!(h.is_empty());
        assert!(h.record("https://example.com/", "Example Domain", 1_000));
        assert!(h.record(
            "https://es.wikipedia.org/wiki/Navegador_web",
            "Navegador web - Wikipedia, la enciclopedia libre",
            2_000
        ));
        assert!(h.record("https://example.com/", "Example Domain", 3_000));
        assert!(!h.record("about:home", "Inicio", 3_500));
        assert!(!h.record("freedom://history", "Historial", 3_500));
    }
    // "Cerrar y abrir": un historial nuevo desde el mismo archivo.
    let h = History::open(path.clone());
    assert_eq!(h.len(), 2);
    let recent = h.recent(10);
    assert_eq!(recent[0].url, "https://example.com/");
    assert_eq!(recent[0].count, 2);
    assert_eq!(recent[0].last, 3_000);
    assert_eq!(
        recent[1].title,
        "Navegador web - Wikipedia, la enciclopedia libre"
    );
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# Freedom: historial v1\n"), "{text}");
}

#[test]
fn search_ignores_case_and_unasked_diacritics() {
    let mut h = History::memory();
    h.record(
        "https://es.wikipedia.org/wiki/Canci%C3%B3n",
        "Canción - Wikipedia",
        10,
    );
    h.record("https://example.com/", "Example Domain", 20);
    let found = |q: &str| -> Vec<String> { h.search(q).iter().map(|v| v.url.clone()).collect() };
    assert_eq!(found("CANCION").len(), 1);
    assert_eq!(found("canción").len(), 1);
    assert_eq!(found("example.COM"), vec!["https://example.com/"]);
    assert_eq!(found("").len(), 2);
    assert!(found("zzz").is_empty());
}

#[test]
fn titles_with_tabs_and_newlines_round_trip() {
    let path = temp_profile("escape").join("history");
    let title = "Uno\tdos\nTres \\ cuatro";
    History::open(path.clone()).record("https://a.test/x?y=1\t2", title, 5);
    let h = History::open(path);
    let v = &h.recent(1)[0];
    assert_eq!(v.title, title);
    assert_eq!(v.url, "https://a.test/x?y=1\t2");
}

#[test]
fn remove_and_clear_persist() {
    let path = temp_profile("remove").join("history");
    let mut h = History::open(path.clone());
    for (i, u) in ["https://a.test/", "https://b.test/", "https://c.test/"]
        .iter()
        .enumerate()
    {
        h.record(u, "", 100 * (i as u64 + 1));
    }
    assert!(h.remove("https://b.test/"));
    assert!(!h.remove("https://b.test/"));
    let urls = |h: &History| -> Vec<String> { h.recent(9).iter().map(|v| v.url.clone()).collect() };
    assert_eq!(
        urls(&History::open(path.clone())),
        vec!["https://c.test/", "https://a.test/"]
    );
    // Borrar desde el instante 250: se va c (300), queda a (100).
    h.clear_since(250);
    assert_eq!(urls(&History::open(path.clone())), vec!["https://a.test/"]);
    h.clear_since(0);
    assert!(History::open(path).is_empty());
}

#[test]
fn repeated_visits_compact_on_open() {
    let path = temp_profile("compact").join("history");
    {
        let mut h = History::open(path.clone());
        for i in 0..500 {
            h.record("https://a.test/", "A", i);
        }
        h.record("https://b.test/", "B", 600);
    }
    let lines = |p: &PathBuf| std::fs::read_to_string(p).unwrap().lines().count();
    assert!(lines(&path) > 500);
    let h = History::open(path.clone());
    assert_eq!(h.recent(1)[0].url, "https://b.test/");
    assert_eq!(h.search("a.test")[0].count, 500);
    // Al abrir se reescribió con una línea por URL (más la cabecera).
    assert_eq!(lines(&path), 3);
}

#[test]
fn broken_lines_are_skipped() {
    let path = temp_profile("broken").join("history");
    std::fs::write(
        &path,
        "# Freedom: historial v1\nbasura\n10\t1\thttps://ok.test/\tOK\nx\ty\tz\tw\n",
    )
    .unwrap();
    let h = History::open(path);
    assert_eq!(h.len(), 1);
    assert_eq!(h.recent(1)[0].title, "OK");
}

#[test]
fn suggestions_prefer_typed_prefix_and_frequent_pages() {
    let mut h = History::memory();
    let day = 86_400;
    h.record("https://www.example.com/", "Example Domain", 10 * day);
    h.record("https://news.ycombinator.com/", "Hacker News", 10 * day);
    for _ in 0..5 {
        h.record(
            "https://es.wikipedia.org/wiki/Rust",
            "Rust - Wikipedia",
            10 * day,
        );
    }
    h.record(
        "https://es.wikipedia.org/wiki/Navegador_web",
        "Navegador web",
        10 * day,
    );
    let urls = |q: &str| -> Vec<String> {
        h.suggest(q, 6, 10 * day)
            .iter()
            .map(|v| v.url.clone())
            .collect()
    };
    // Sin esquema ni `www.`: "exa" es prefijo de example.com.
    assert_eq!(urls("exa")[0], "https://www.example.com/");
    // Dos de Wikipedia: primero la más visitada.
    assert_eq!(
        urls("es.wikipedia"),
        vec![
            "https://es.wikipedia.org/wiki/Rust",
            "https://es.wikipedia.org/wiki/Navegador_web"
        ]
    );
    // Una palabra del título.
    assert_eq!(
        urls("navegador")[0],
        "https://es.wikipedia.org/wiki/Navegador_web"
    );
    assert!(urls("zzz").is_empty());
}

#[test]
fn top_sites_rank_by_frecency_one_per_site() {
    let mut h = History::memory();
    let day = 86_400;
    let now = 400 * day;
    // Muchas visitas recientes a un sitio, con dos páginas: un solo acceso (la más visitada).
    for i in 0..6 {
        h.record("https://es.wikipedia.org/wiki/Canción", "Canción", now - i);
    }
    h.record("https://es.wikipedia.org/wiki/Árbol", "Árbol", now);
    // `www.` no separa sitios.
    h.record("https://www.example.com/", "Example", now - day);
    h.record("https://example.com/otra", "Otra", now - day);
    h.record("https://example.com/otra", "Otra", now - day);
    // Muchas visitas, pero viejas: pesan menos que pocas recientes.
    for _ in 0..8 {
        h.record("https://viejo.example.org/", "Viejo", now - 300 * day);
    }
    h.record(
        "https://news.ycombinator.com/",
        "Hacker News",
        now - 2 * day,
    );
    // Los archivos locales cuentan cada uno por su ruta.
    h.record("file:///C:/a.html", "A", now);
    h.record("file:///C:/b.html", "B", now);

    let top: Vec<&str> = h.top_sites(8, now).iter().map(|v| v.url.as_str()).collect();
    assert_eq!(
        top,
        [
            "https://es.wikipedia.org/wiki/Canción",
            "https://example.com/otra",
            "file:///C:/a.html",
            "file:///C:/b.html",
            "https://news.ycombinator.com/",
            "https://viejo.example.org/",
        ]
    );
    assert_eq!(h.top_sites(2, now).len(), 2);
    assert!(History::memory().top_sites(8, now).is_empty());
}
