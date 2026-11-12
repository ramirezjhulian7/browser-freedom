//! La sesión de la última vez (`src/restore.rs`): el archivo `session` del perfil con las
//! ventanas, sus pestañas y el historial de cada una, que Freedom vuelve a abrir al arrancar.

#![forbid(unsafe_code)]
#![allow(dead_code)]

#[path = "../src/profile.rs"]
mod profile;
#[path = "../src/restore.rs"]
mod restore;

use restore::{MAX_ENTRIES, SavedEntry, SavedSession, SavedTab, SavedWindow};

fn entry(url: &str, title: &str) -> SavedEntry {
    SavedEntry {
        url: url.to_owned(),
        title: title.to_owned(),
    }
}

/// La entrada que se muestra en la pestaña.
fn current(t: &SavedTab) -> Option<&SavedEntry> {
    t.entries.get(t.index)
}

fn two_windows() -> SavedSession {
    SavedSession {
        windows: vec![
            SavedWindow {
                tabs: vec![
                    SavedTab {
                        entries: vec![
                            entry("about:home", "Inicio"),
                            entry(
                                "https://es.wikipedia.org/wiki/Canción",
                                "Canción - Wikipedia",
                            ),
                            entry("https://es.wikipedia.org/wiki/Árbol", "Árbol\tcon tab"),
                        ],
                        index: 1,
                    },
                    SavedTab {
                        entries: vec![entry("freedom://history", "Historial")],
                        index: 0,
                    },
                ],
                active: 1,
            },
            SavedWindow {
                tabs: vec![SavedTab {
                    entries: vec![entry("file:///C:/página%20local.html", "")],
                    index: 0,
                }],
                active: 0,
            },
        ],
    }
}

#[test]
fn round_trip() {
    let s = two_windows();
    let text = s.serialize();
    assert!(text.starts_with("# Freedom: sesión v1\n"));
    // Los campos con tabulador se escapan.
    assert!(text.contains("Árbol\\tcon tab"));
    assert_eq!(SavedSession::parse(&text), s);
}

#[test]
fn survives_close_and_open() {
    let dir = std::env::temp_dir().join(format!("freedom-e11-restore-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session");
    let s = two_windows();
    profile::write_atomic(&path, &s.serialize()).unwrap();
    let back = SavedSession::parse(&std::fs::read_to_string(&path).unwrap());
    assert_eq!(back, s);
    assert_eq!(
        current(&back.windows[0].tabs[0]).map(|e| e.url.as_str()),
        Some("https://es.wikipedia.org/wiki/Canción")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn broken_files_keep_what_makes_sense() {
    let text = "# Freedom: sesión v1\n\
                entry\thttps://huérfana.example/\tsin pestaña\n\
                tab\t0\n\
                window\t7\n\
                tab\t9\n\
                entry\thttps://a.example/\tA\n\
                entry\tdata:text/html,hola\tdatos\n\
                entry\thttps://b.example/\tB\n\
                tab\t0\n\
                entry\tjavascript:alert(1)\tx\n\
                raro\tcosa\n\
                window\tx\n\
                tab\t0\n";
    let s = SavedSession::parse(text);
    // Lo que va antes de la primera ventana, las pestañas sin entradas restaurables y las
    // ventanas sin pestañas se descartan; los índices fuera de rango se acotan.
    assert_eq!(s.windows.len(), 1);
    let w = &s.windows[0];
    assert_eq!(w.tabs.len(), 1);
    assert_eq!(w.active, 0);
    assert_eq!(
        w.tabs[0].entries,
        [
            entry("https://a.example/", "A"),
            entry("https://b.example/", "B")
        ]
    );
    assert_eq!(w.tabs[0].index, 1);
    assert!(SavedSession::parse("").is_empty());
    assert!(SavedSession::parse("basura sin formato").is_empty());
}

#[test]
fn tabs_drop_unrestorable_entries_and_keep_the_current_one() {
    let t = SavedTab::new(
        vec![
            entry("https://a.example/", "A"),
            entry("data:text/plain,x", ""),
            entry("blob:https://a.example/1", ""),
            entry("https://b.example/", "B"),
            entry("bf:history-clear?all", ""),
        ],
        3,
    )
    .unwrap();
    assert_eq!(t.entries.len(), 2);
    assert_eq!(current(&t), Some(&entry("https://b.example/", "B")));
    // La actual no restaurable: queda la anterior que sí.
    let t = SavedTab::new(
        vec![
            entry("https://a.example/", "A"),
            entry("data:text/plain,x", ""),
        ],
        1,
    )
    .unwrap();
    assert_eq!(current(&t), Some(&entry("https://a.example/", "A")));
    assert!(SavedTab::new(vec![entry("data:,", "")], 0).is_none());
    assert!(SavedTab::new(Vec::new(), 0).is_none());
}

#[test]
fn long_histories_are_trimmed_around_the_current_entry() {
    let entries: Vec<SavedEntry> = (0..200)
        .map(|i| entry(&format!("https://e.example/{i}"), ""))
        .collect();
    let t = SavedTab::new(entries.clone(), 120).unwrap();
    assert_eq!(t.entries.len(), MAX_ENTRIES);
    assert_eq!(current(&t).unwrap().url, "https://e.example/120");
    // Cerca del final: se guardan las últimas.
    let t = SavedTab::new(entries.clone(), 199).unwrap();
    assert_eq!(t.entries.len(), MAX_ENTRIES);
    assert_eq!(current(&t).unwrap().url, "https://e.example/199");
    assert_eq!(t.entries[0].url, "https://e.example/150");
    // Cerca del principio: las primeras.
    let t = SavedTab::new(entries, 3).unwrap();
    assert_eq!(t.entries[0].url, "https://e.example/0");
    assert_eq!(t.index, 3);
}

#[test]
fn restorable_schemes() {
    for ok in [
        "https://a.example/",
        "http://a.example/",
        "file:///C:/x.html",
        "about:home",
        "freedom://settings",
    ] {
        assert!(restore::restorable(ok), "{ok}");
    }
    for no in [
        "data:text/html,x",
        "DATA:,x",
        "blob:https://a/1",
        "javascript:void(0)",
        "bf:focus-url",
        "",
        "sin-esquema",
    ] {
        assert!(!restore::restorable(no), "{no}");
    }
}
