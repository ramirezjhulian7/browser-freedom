//! El almacén de IndexedDB: un **log de operaciones por base de datos** con el estado completo
//! en memoria (lo reconstruye `idb.js` al abrir).
//!
//! - Archivo: `<dir>/<origen>/<nombre en hex>.idb`, líneas de texto (una operación por línea,
//!   JSON sin saltos); cada transacción que hace commit agrega sus operaciones y una línea de
//!   commit (`["commit"]`) y hace `fsync` (durabilidad "strict"). Al abrir se ignoran las
//!   operaciones posteriores al último commit (una transacción a medio escribir por un corte).
//! - Compactación: cuando el log crece mucho más que los datos vivos, `idb.js` manda una
//!   instantánea que se escribe en un temporal, se sincroniza y se renombra encima (atómico).
//! - Sin directorio (sin hook de almacenamiento), los "archivos" viven en memoria del agente.
//!
//! Concurrencia: un documento (un hilo) por base de datos abierta; dos pestañas del mismo
//! origen ven los cambios de la otra al reabrir.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Los archivos de un origen.
#[derive(Debug, Default)]
pub struct Backend {
    /// `None`: en memoria.
    pub dir: Option<PathBuf>,
    mem: HashMap<String, String>,
}

/// Un nombre de archivo seguro para el origen (`https://a.com:8080` → `https_a.com_8080`).
pub fn origin_dir_name(origin: &str) -> String {
    origin
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .replace("___", "_")
}

/// El nombre de archivo de una base de datos (su nombre en UTF-8 en hexadecimal).
fn file_name(name: &str) -> String {
    let mut s: String = name.bytes().map(|b| format!("{b:02x}")).collect();
    s.push_str(".idb");
    s
}

fn name_of_file(file: &str) -> Option<String> {
    let hex = file.strip_suffix(".idb")?;
    if hex.len() % 2 != 0 {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}

impl Backend {
    pub fn new(dir: Option<PathBuf>) -> Backend {
        Backend {
            dir,
            mem: HashMap::new(),
        }
    }

    fn path(&self, name: &str) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(file_name(name)))
    }

    /// El contenido del log de `name` (`None` si no existe).
    pub fn load(&self, name: &str) -> Option<String> {
        match self.path(name) {
            Some(p) => fs::read_to_string(p).ok(),
            None => self.mem.get(name).cloned(),
        }
    }

    /// Agrega `text` al log y sincroniza.
    pub fn append(&mut self, name: &str, text: &str) -> std::io::Result<()> {
        match self.path(name) {
            Some(p) => {
                if let Some(parent) = p.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut f = OpenOptions::new().create(true).append(true).open(&p)?;
                f.write_all(text.as_bytes())?;
                f.sync_data()
            }
            None => {
                self.mem.entry(name.to_owned()).or_default().push_str(text);
                Ok(())
            }
        }
    }

    /// Reemplaza el log por `text` (compactación): temporal, `fsync` y rename.
    pub fn compact(&mut self, name: &str, text: &str) -> std::io::Result<()> {
        match self.path(name) {
            Some(p) => {
                if let Some(parent) = p.parent() {
                    fs::create_dir_all(parent)?;
                }
                let tmp = p.with_extension("idb.tmp");
                {
                    let mut f = fs::File::create(&tmp)?;
                    f.write_all(text.as_bytes())?;
                    f.sync_all()?;
                }
                fs::rename(&tmp, &p)
            }
            None => {
                self.mem.insert(name.to_owned(), text.to_owned());
                Ok(())
            }
        }
    }

    /// Borra la base de datos.
    pub fn remove(&mut self, name: &str) {
        match self.path(name) {
            Some(p) => {
                let _ = fs::remove_file(p);
            }
            None => {
                self.mem.remove(name);
            }
        }
    }

    /// Los nombres de las bases de datos del origen.
    pub fn names(&self) -> Vec<String> {
        match &self.dir {
            Some(d) => {
                let mut v: Vec<String> = fs::read_dir(d)
                    .map(|rd| {
                        rd.flatten()
                            .filter_map(|e| name_of_file(&e.file_name().to_string_lossy()))
                            .collect()
                    })
                    .unwrap_or_default();
                v.sort();
                v
            }
            None => {
                let mut v: Vec<String> = self.mem.keys().cloned().collect();
                v.sort();
                v
            }
        }
    }

    /// Los bytes que ocupa el origen.
    pub fn usage(&self) -> u64 {
        match &self.dir {
            Some(d) => dir_size(d),
            None => self.mem.values().map(|s| s.len() as u64).sum(),
        }
    }
}

fn dir_size(d: &Path) -> u64 {
    fs::read_dir(d)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_files() {
        assert_eq!(
            name_of_file(&file_name("mi base ñ")).as_deref(),
            Some("mi base ñ")
        );
        assert_eq!(origin_dir_name("https://a.com:8080"), "https_a.com_8080");
        let mut b = Backend::new(None);
        b.append("x", "uno\n").expect("append");
        b.append("x", "dos\n").expect("append");
        assert_eq!(b.load("x").as_deref(), Some("uno\ndos\n"));
        b.compact("x", "tres\n").expect("compact");
        assert_eq!(b.load("x").as_deref(), Some("tres\n"));
        assert_eq!(b.names(), vec!["x".to_owned()]);
        b.remove("x");
        assert!(b.load("x").is_none());
    }

    #[test]
    fn on_disk() {
        let dir = std::env::temp_dir().join(format!("freedom-idb-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut b = Backend::new(Some(dir.clone()));
        b.append("db", "a\n").expect("append");
        let b2 = Backend::new(Some(dir.clone()));
        assert_eq!(b2.load("db").as_deref(), Some("a\n"));
        assert_eq!(b2.names(), vec!["db".to_owned()]);
        assert!(b2.usage() >= 2);
        let _ = fs::remove_dir_all(&dir);
    }
}
