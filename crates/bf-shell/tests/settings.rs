//! Las opciones del navegador (`src/settings.rs`): formato del archivo del perfil, valores
//! inválidos, nombres desconocidos que se conservan y los pasos del zoom de texto.

#![forbid(unsafe_code)]
#![allow(dead_code)]

#[path = "../src/profile.rs"]
mod profile;
#[path = "../src/settings.rs"]
mod settings;

use settings::{Settings, TEXT_ZOOM_LEVELS};

#[test]
fn defaults() {
    let s = Settings::default();
    assert!(!s.block_third_party_cookies);
    assert_eq!(s.text_zoom, 1.0);
    assert_eq!(Settings::parse(""), s);
}

#[test]
fn round_trip_keeps_unknown_names() {
    let text = "# Freedom: opciones v1\n\
                block_third_party_cookies\ttrue\n\
                text_zoom\t1.2\n\
                tema\toscuro\n";
    let s = Settings::parse(text);
    assert!(s.block_third_party_cookies);
    assert_eq!(s.text_zoom, 1.2);
    let out = s.serialize();
    assert!(out.starts_with("# Freedom: opciones v1\n"));
    assert!(out.contains("block_third_party_cookies\ttrue\n"));
    assert!(out.contains("text_zoom\t1.2\n"));
    // Una opción de una versión más nueva sobrevive a guardar con esta.
    assert!(out.contains("tema\toscuro\n"));
    assert_eq!(Settings::parse(&out), s);
}

#[test]
fn invalid_values_keep_defaults() {
    let s = Settings::parse("block_third_party_cookies\tquizás\ntext_zoom\tNaN\nroto\n");
    assert!(!s.block_third_party_cookies);
    assert_eq!(s.text_zoom, 1.0);
    // Fuera de rango: al nivel más cercano.
    assert_eq!(Settings::parse("text_zoom\t9").text_zoom, 2.0);
    assert_eq!(Settings::parse("text_zoom\t0.1").text_zoom, 0.5);
}

#[test]
fn text_zoom_steps() {
    let mut s = Settings::default();
    assert_eq!(s.text_zoom_step(true), 1.1);
    assert_eq!(s.text_zoom_step(false), 0.9);
    // Desde un valor entre niveles, al vecino de cada lado.
    s.text_zoom = 1.15;
    assert_eq!(s.text_zoom_step(true), 1.2);
    assert_eq!(s.text_zoom_step(false), 1.1);
    // En los extremos se queda.
    s.text_zoom = TEXT_ZOOM_LEVELS[TEXT_ZOOM_LEVELS.len() - 1];
    assert_eq!(s.text_zoom_step(true), s.text_zoom);
    s.text_zoom = TEXT_ZOOM_LEVELS[0];
    assert_eq!(s.text_zoom_step(false), s.text_zoom);
}

#[test]
fn saved_to_the_profile_file() {
    let dir = std::env::temp_dir().join(format!("freedom-test-opciones-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings");
    let mut s = Settings::open(path.clone());
    assert_eq!(s.text_zoom, 1.0);
    s.text_zoom = 1.33;
    s.block_third_party_cookies = true;
    s.save();
    let again = Settings::open(path);
    assert_eq!(again.text_zoom, 1.33);
    assert!(again.block_third_party_cookies);
    let _ = std::fs::remove_dir_all(&dir);
}
