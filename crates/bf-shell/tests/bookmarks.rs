//! La capa de datos de los marcadores (`src/bookmarks.rs`): persistencia en el perfil,
//! carpetas, edición e importación del HTML de marcadores de Netscape. Los módulos se incluyen
//! por ruta porque `bf-shell` es solo un binario.

#![forbid(unsafe_code)]
#![allow(dead_code)]

#[path = "../src/bookmarks.rs"]
mod bookmarks;
#[path = "../src/find.rs"]
mod find;
#[path = "../src/profile.rs"]
mod profile;

use std::path::PathBuf;

use bookmarks::Bookmarks;

fn temp_profile(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("freedom-e11-bm-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn survives_close_and_open_with_folders() {
    let path = temp_profile("persist").join("bookmarks");
    let (a, c) = {
        let mut b = Bookmarks::open(path.clone());
        let a = b.add("https://example.com/", "Example Domain", "", 10);
        b.add_folder("Lectura");
        let c = b.add(
            "https://es.wikipedia.org/wiki/Navegador_web",
            "Navegador\tweb",
            "Lectura",
            20,
        );
        b.add_folder("Vacía");
        (a, c)
    };
    let mut b = Bookmarks::open(path.clone());
    assert_eq!(b.len(), 2);
    assert_eq!(b.folders(), ["Lectura".to_owned(), "Vacía".to_owned()]);
    assert_eq!(b.get(c).unwrap().title, "Navegador\tweb");
    assert_eq!(b.in_folder("Lectura").count(), 1);
    // Los ids siguen después de los guardados.
    let d = b.add("https://d.test/", "D", "", 30);
    assert!(d > a && d > c);
    // Editar y borrar persisten.
    assert!(b.update(a, "Ejemplo", "https://example.org/", "Lectura"));
    assert!(b.remove(c));
    assert!(b.remove_folder("Vacía"));
    let b = Bookmarks::open(path);
    let e = b.get(a).unwrap();
    assert_eq!(
        (e.title.as_str(), e.url.as_str(), e.folder.as_str()),
        ("Ejemplo", "https://example.org/", "Lectura")
    );
    assert!(b.get(c).is_none());
    assert_eq!(b.folders(), ["Lectura".to_owned()]);
}

#[test]
fn toggle_adds_then_removes() {
    let mut b = Bookmarks::memory();
    assert!(b.toggle("https://a.test/", "A", 1));
    assert!(b.find_url("https://a.test/").is_some());
    assert!(!b.toggle("https://a.test/", "A", 2));
    assert!(b.find_url("https://a.test/").is_none());
}

#[test]
fn removing_a_folder_keeps_its_bookmarks_at_the_root() {
    let mut b = Bookmarks::memory();
    let id = b.add("https://a.test/", "A", "Trabajo", 1);
    assert!(b.remove_folder("Trabajo"));
    assert_eq!(b.get(id).unwrap().folder, "");
    assert!(b.folders().is_empty());
}

/// Un archivo como el que exportan los navegadores (estructura de Firefox y Chromium).
const EXPORT: &str = r#"<!DOCTYPE NETSCAPE-Bookmark-file-1>
<!-- This is an automatically generated file.
     It will be read and overwritten.
     DO NOT EDIT! -->
<META HTTP-EQUIV="Content-Type" CONTENT="text/html; charset=UTF-8">
<TITLE>Bookmarks</TITLE>
<H1>Bookmarks Menu</H1>

<DL><p>
    <DT><H3 ADD_DATE="1690000000" LAST_MODIFIED="1690000001" PERSONAL_TOOLBAR_FOLDER="true">Barra de marcadores</H3>
    <DL><p>
        <DT><A HREF="https://news.ycombinator.com/" ADD_DATE="1690000100" ICON="data:image/png;base64,AAAA">Hacker News</A>
        <DT><H3 ADD_DATE="1690000200">Wiki &amp; docs</H3>
        <DL><p>
            <DT><A HREF="https://es.wikipedia.org/wiki/Canci%C3%B3n" ADD_DATE="1690000300">Canción &#8212; Wikipedia</A>
            <DD>Una descripción que se ignora
        </DL><p>
    </DL><p>
    <HR>
    <DT><A HREF="https://example.com/" ADD_DATE="1690000400">Example Domain</A>
    <DT><A HREF="place:sort=8&maxResults=10">Más visitados</A>
    <DT><A HREF="javascript:void(0)">Bookmarklet</A>
</DL><p>
"#;

#[test]
fn imports_netscape_export() {
    let path = temp_profile("import").join("bookmarks");
    let mut b = Bookmarks::open(path.clone());
    assert_eq!(b.import_netscape(EXPORT, "Importados", 99), 3);
    let all: Vec<(String, String, String, u64)> = b
        .all()
        .iter()
        .map(|m| (m.url.clone(), m.title.clone(), m.folder.clone(), m.added))
        .collect();
    assert_eq!(
        all,
        vec![
            (
                "https://news.ycombinator.com/".to_owned(),
                "Hacker News".to_owned(),
                "Importados / Barra de marcadores".to_owned(),
                1_690_000_100
            ),
            (
                "https://es.wikipedia.org/wiki/Canci%C3%B3n".to_owned(),
                "Canción — Wikipedia".to_owned(),
                "Importados / Barra de marcadores / Wiki & docs".to_owned(),
                1_690_000_300
            ),
            (
                "https://example.com/".to_owned(),
                "Example Domain".to_owned(),
                "Importados".to_owned(),
                1_690_000_400
            ),
        ]
    );
    // Reimportar no duplica; se guardó en el perfil.
    assert_eq!(b.import_netscape(EXPORT, "Importados", 99), 0);
    assert_eq!(Bookmarks::open(path).len(), 3);
    // La búsqueda pliega mayúsculas y diacríticos no pedidos.
    assert_eq!(b.search("cancion").len(), 1);
    assert_eq!(b.search("WIKI & DOCS").len(), 1);
}

#[test]
fn export_round_trips_through_import() {
    let mut b = Bookmarks::memory();
    b.add("https://a.test/?x=1&y=<2>", "A \"uno\"", "", 5);
    b.add("https://b.test/", "B", "Carpeta", 6);
    let html = b.export_netscape();
    let mut c = Bookmarks::memory();
    assert_eq!(c.import_netscape(&html, "", 0), 2);
    assert_eq!(c.all()[1].url, "https://a.test/?x=1&y=<2>");
    assert_eq!(c.all()[1].title, "A \"uno\"");
    assert_eq!(c.all()[0].folder, "Carpeta");
    assert_eq!(c.all()[0].added, 6);
}
