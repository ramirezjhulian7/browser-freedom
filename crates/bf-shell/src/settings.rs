//! Las opciones del navegador (`freedom://settings`): se guardan en el archivo `settings` del
//! perfil ([`crate::profile`]), una por línea:
//!
//! ```text
//! # Freedom: opciones v1
//! <nombre>\t<valor>
//! ```
//!
//! Los nombres que no se conocen se conservan al guardar (una versión más nueva pudo
//! escribirlos); los valores inválidos se ignoran y queda el de omisión.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::profile;

const HEADER: &str = "# Freedom: opciones v1";

/// Los niveles del zoom de texto (`Ctrl+Shift` + rueda), como fracción.
pub const TEXT_ZOOM_LEVELS: [f32; 11] = [0.5, 0.67, 0.8, 0.9, 1.0, 1.1, 1.2, 1.33, 1.5, 1.7, 2.0];

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// No mandar ni guardar cookies en peticiones entre sitios (RFC 6265bis §7.1; E4).
    pub block_third_party_cookies: bool,
    /// Zoom de texto (1 = sin zoom): escala solo los tamaños de fuente, no la página.
    pub text_zoom: f32,
    /// Líneas con nombres que esta versión no conoce, tal cual.
    unknown: Vec<String>,
    path: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            block_third_party_cookies: false,
            text_zoom: 1.0,
            unknown: Vec::new(),
            path: None,
        }
    }
}

impl Settings {
    /// Lee las opciones de `path` (las de omisión si no existe).
    pub fn open(path: PathBuf) -> Settings {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let mut s = Settings::parse(&text);
        s.path = Some(path);
        s
    }

    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let Some(fields) = profile::split_record(line) else {
                continue;
            };
            let [name, value] = &fields[..] else {
                continue;
            };
            match name.as_str() {
                "block_third_party_cookies" => {
                    if let Some(b) = parse_bool(value) {
                        s.block_third_party_cookies = b;
                    }
                }
                "text_zoom" => {
                    if let Some(z) = value
                        .parse::<f32>()
                        .ok()
                        .filter(|z| z.is_finite())
                        .map(clamp_text_zoom)
                    {
                        s.text_zoom = z;
                    }
                }
                _ => s.unknown.push(line.to_owned()),
            }
        }
        s
    }

    /// El archivo: encabezado, las opciones conocidas y las desconocidas al final.
    pub fn serialize(&self) -> String {
        let mut out = format!("{HEADER}\n");
        let bool_text = if self.block_third_party_cookies {
            "true"
        } else {
            "false"
        };
        out.push_str(&profile::join_record(&[
            "block_third_party_cookies",
            bool_text,
        ]));
        out.push('\n');
        out.push_str(&profile::join_record(&[
            "text_zoom",
            &format!("{}", self.text_zoom),
        ]));
        out.push('\n');
        for line in &self.unknown {
            out.push_str(line);
            out.push('\n');
        }
        out
    }

    /// Guarda en el perfil (si hay).
    pub fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        if let Err(e) = profile::write_atomic(path, &self.serialize()) {
            eprintln!(
                "aviso: no se pudieron guardar las opciones en {}: {e}",
                path.display()
            );
        }
    }

    /// El nivel de zoom de texto siguiente (`up`) o anterior a `self.text_zoom`.
    pub fn text_zoom_step(&self, up: bool) -> f32 {
        let z = self.text_zoom;
        let found = if up {
            TEXT_ZOOM_LEVELS.iter().find(|&&l| l > z + 0.001)
        } else {
            TEXT_ZOOM_LEVELS.iter().rev().find(|&&l| l < z - 0.001)
        };
        found.copied().unwrap_or(z)
    }
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.trim() {
        "true" | "1" | "sí" | "si" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

/// El zoom de texto dentro de los niveles ofrecidos.
pub fn clamp_text_zoom(z: f32) -> f32 {
    z.clamp(TEXT_ZOOM_LEVELS[0], TEXT_ZOOM_LEVELS[TEXT_ZOOM_LEVELS.len() - 1])
}

/// Las opciones del navegador (del perfil, o en memoria sin él).
pub fn global() -> MutexGuard<'static, Settings> {
    static GLOBAL: OnceLock<Mutex<Settings>> = OnceLock::new();
    GLOBAL
        .get_or_init(|| {
            Mutex::new(match profile::file("settings") {
                Some(p) => Settings::open(p),
                None => Settings::default(),
            })
        })
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}
