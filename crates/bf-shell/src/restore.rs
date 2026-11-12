//! La sesión de la última vez (E11): las ventanas, sus pestañas y el historial de atrás y
//! adelante de cada una, para abrirlas igual al volver a arrancar (opción `restore_session`
//! de [`crate::settings`]). Se guarda en el archivo `session` del perfil con el formato de
//! [`crate::profile`]:
//!
//! ```text
//! # Freedom: sesión v1
//! window\t<pestaña activa>
//! tab\t<entrada actual>
//! entry\t<url>\t<título>
//! ```
//!
//! Cada `tab` pertenece a la última `window` y cada `entry` a la última `tab`. Lo que no se
//! entiende se ignora; las pestañas sin entradas y las ventanas sin pestañas se descartan.

use crate::profile;

const HEADER: &str = "# Freedom: sesión v1";

/// Entradas de historial que se guardan por pestaña (alrededor de la actual).
pub const MAX_ENTRIES: usize = 50;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SavedEntry {
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SavedTab {
    pub entries: Vec<SavedEntry>,
    /// La entrada que se muestra.
    pub index: usize,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SavedWindow {
    pub tabs: Vec<SavedTab>,
    /// La pestaña visible.
    pub active: usize,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SavedSession {
    pub windows: Vec<SavedWindow>,
}

/// ¿Vale la pena volver a abrir `url`? Las URLs que llevan el contenido dentro (`data:`,
/// `blob:`, `javascript:`) o las acciones de la interfaz (`bf:`) no.
pub fn restorable(url: &str) -> bool {
    let scheme = url.split(':').next().unwrap_or("").to_ascii_lowercase();
    !matches!(scheme.as_str(), "data" | "blob" | "javascript" | "bf" | "") && url.contains(':')
}

impl SavedTab {
    /// Una pestaña con `entries` y la actual en `index`, sin las entradas que no se pueden
    /// restaurar y recortada a [`MAX_ENTRIES`] alrededor de la actual. `None` si no queda
    /// ninguna.
    pub fn new(entries: Vec<SavedEntry>, index: usize) -> Option<SavedTab> {
        let mut kept = Vec::with_capacity(entries.len());
        let mut new_index = 0;
        for (i, e) in entries.into_iter().enumerate() {
            if !restorable(&e.url) {
                continue;
            }
            if i <= index {
                new_index = kept.len();
            }
            kept.push(e);
        }
        if kept.is_empty() {
            return None;
        }
        if kept.len() > MAX_ENTRIES {
            let start = new_index
                .saturating_sub(MAX_ENTRIES / 2)
                .min(kept.len() - MAX_ENTRIES);
            kept.drain(..start);
            kept.truncate(MAX_ENTRIES);
            new_index -= start;
        }
        Some(SavedTab {
            entries: kept,
            index: new_index,
        })
    }
}

impl SavedSession {
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    pub fn parse(text: &str) -> SavedSession {
        let mut s = SavedSession::default();
        for line in text.lines() {
            let Some(fields) = profile::split_record(line) else {
                continue;
            };
            match fields.first().map(String::as_str) {
                Some("window") => s.windows.push(SavedWindow {
                    tabs: Vec::new(),
                    active: number(fields.get(1)),
                }),
                Some("tab") => {
                    if let Some(w) = s.windows.last_mut() {
                        w.tabs.push(SavedTab {
                            entries: Vec::new(),
                            index: number(fields.get(1)),
                        });
                    }
                }
                Some("entry") => {
                    if let (Some(t), Some(url)) = (
                        s.windows.last_mut().and_then(|w| w.tabs.last_mut()),
                        fields.get(1).filter(|u| restorable(u)),
                    ) {
                        t.entries.push(SavedEntry {
                            url: url.clone(),
                            title: fields.get(2).cloned().unwrap_or_default(),
                        });
                    }
                }
                _ => {}
            }
        }
        for w in &mut s.windows {
            w.tabs.retain(|t| !t.entries.is_empty());
            for t in &mut w.tabs {
                t.index = t.index.min(t.entries.len() - 1);
            }
            w.active = w.active.min(w.tabs.len().saturating_sub(1));
        }
        s.windows.retain(|w| !w.tabs.is_empty());
        s
    }

    pub fn serialize(&self) -> String {
        let mut out = format!("{HEADER}\n");
        for w in &self.windows {
            out.push_str(&profile::join_record(&["window", &w.active.to_string()]));
            out.push('\n');
            for t in &w.tabs {
                out.push_str(&profile::join_record(&["tab", &t.index.to_string()]));
                out.push('\n');
                for e in &t.entries {
                    out.push_str(&profile::join_record(&["entry", &e.url, &e.title]));
                    out.push('\n');
                }
            }
        }
        out
    }

    /// La sesión guardada en el perfil (vacía si no hay perfil o archivo).
    pub fn load() -> SavedSession {
        profile::file("session")
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| SavedSession::parse(&t))
            .unwrap_or_default()
    }

    /// La guarda en el perfil (si hay).
    pub fn save(&self) {
        let Some(path) = profile::file("session") else {
            return;
        };
        if let Err(e) = profile::write_atomic(&path, &self.serialize()) {
            eprintln!(
                "aviso: no se pudo guardar la sesión en {}: {e}",
                path.display()
            );
        }
    }
}

fn number(field: Option<&String>) -> usize {
    field.and_then(|v| v.trim().parse().ok()).unwrap_or(0)
}
