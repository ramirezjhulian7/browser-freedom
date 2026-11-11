//! El historial global y persistente: cada página visitada con su título, la hora de la última
//! visita y cuántas veces se visitó. Vive en el archivo `history` del perfil
//! ([`crate::profile`]), un registro por línea:
//!
//! ```text
//! # Freedom: historial v1
//! <última visita, s Unix>\t<visitas>\t<URL>\t<título>
//! ```
//!
//! Cada visita agrega una línea (barato aunque el historial sea grande); al abrir, la última
//! línea de cada URL manda y, si sobran muchas, el archivo se compacta. Borrar reescribe el
//! archivo entero. El historial de sesión de cada pestaña (atrás/adelante) es aparte, en la
//! ventana.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::find;
use crate::profile;

const HEADER: &str = "# Freedom: historial v1";
/// Entradas como mucho; al pasarse se olvidan las más viejas.
pub const MAX_ENTRIES: usize = 20_000;

/// Una página del historial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Visit {
    pub url: String,
    pub title: String,
    /// Última visita, en segundos Unix.
    pub last: u64,
    pub count: u32,
}

impl Visit {
    fn line(&self) -> String {
        profile::join_record(&[
            &self.last.to_string(),
            &self.count.to_string(),
            &self.url,
            &self.title,
        ])
    }

    fn parse(line: &str) -> Option<Visit> {
        let f = profile::split_record(line)?;
        let [last, count, url, title] = &f[..] else {
            return None;
        };
        Some(Visit {
            url: url.clone(),
            title: title.clone(),
            last: last.parse().ok()?,
            count: count.parse().ok()?,
        })
    }

    /// El título, o la URL si la página no tenía.
    pub fn label(&self) -> &str {
        if self.title.trim().is_empty() {
            &self.url
        } else {
            &self.title
        }
    }
}

/// ¿Se guarda una visita a `url`? Solo páginas de la web y archivos: no las internas
/// (`about:`, `freedom:`), ni `data:`, `blob:` o `javascript:`.
pub fn recordable(url: &str) -> bool {
    let scheme = url.split(':').next().unwrap_or("").to_ascii_lowercase();
    matches!(scheme.as_str(), "http" | "https" | "file")
}

/// El historial. Sin `path` vive solo en memoria.
#[derive(Debug, Default)]
pub struct History {
    path: Option<PathBuf>,
    /// Las visitas, sin orden; `by_url` indexa `entries`.
    entries: Vec<Visit>,
    by_url: HashMap<String, usize>,
}

impl History {
    /// Un historial en memoria.
    pub fn memory() -> History {
        History::default()
    }

    /// Lee el historial de `path` (si no existe, empieza vacío) y lo guarda ahí.
    pub fn open(path: PathBuf) -> History {
        let mut h = History {
            path: Some(path),
            ..History::default()
        };
        let text = h
            .path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        let mut lines = 0;
        for visit in text.lines().filter_map(Visit::parse) {
            lines += 1;
            h.put(visit);
        }
        let over = h.entries.len() > MAX_ENTRIES;
        if over {
            h.trim();
        }
        if over || lines > 2 * h.entries.len().max(64) {
            h.save();
        }
        h
    }

    fn put(&mut self, visit: Visit) {
        match self.by_url.get(&visit.url) {
            Some(&i) => self.entries[i] = visit,
            None => {
                self.by_url.insert(visit.url.clone(), self.entries.len());
                self.entries.push(visit);
            }
        }
    }

    fn reindex(&mut self) {
        self.by_url = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, v)| (v.url.clone(), i))
            .collect();
    }

    /// Deja las [`MAX_ENTRIES`] más recientes.
    fn trim(&mut self) {
        self.entries.sort_by_key(|v| std::cmp::Reverse(v.last));
        self.entries.truncate(MAX_ENTRIES);
        self.reindex();
    }

    /// Reescribe el archivo con el estado actual.
    pub fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let mut out = String::from(HEADER);
        out.push('\n');
        let mut sorted: Vec<&Visit> = self.entries.iter().collect();
        sorted.sort_by_key(|v| v.last);
        for v in sorted {
            out.push_str(&v.line());
            out.push('\n');
        }
        if let Err(e) = profile::write_atomic(path, &out) {
            eprintln!(
                "aviso: no se pudo guardar el historial en {}: {e}",
                path.display()
            );
        }
    }

    /// Anota una visita a `url` con `title` en el instante `at`. Devuelve si se guardó (las
    /// URLs que no son [`recordable`] no).
    pub fn record(&mut self, url: &str, title: &str, at: u64) -> bool {
        if !recordable(url) {
            return false;
        }
        let visit = match self.by_url.get(url) {
            Some(&i) => {
                let old = &self.entries[i];
                Visit {
                    url: url.to_owned(),
                    // Una recarga sin título no borra el que había.
                    title: if title.trim().is_empty() {
                        old.title.clone()
                    } else {
                        title.to_owned()
                    },
                    last: at.max(old.last),
                    count: old.count.saturating_add(1),
                }
            }
            None => Visit {
                url: url.to_owned(),
                title: title.to_owned(),
                last: at,
                count: 1,
            },
        };
        if let Some(path) = &self.path {
            let fresh = !path.exists();
            let line = if fresh {
                format!("{HEADER}\n{}", visit.line())
            } else {
                visit.line()
            };
            if let Err(e) = profile::append_line(path, &line) {
                eprintln!(
                    "aviso: no se pudo guardar el historial en {}: {e}",
                    path.display()
                );
            }
        }
        self.put(visit);
        if self.entries.len() > MAX_ENTRIES + MAX_ENTRIES / 10 {
            self.trim();
            self.save();
        }
        true
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Borra `url`; devuelve si estaba.
    pub fn remove(&mut self, url: &str) -> bool {
        let Some(i) = self.by_url.remove(url) else {
            return false;
        };
        self.entries.swap_remove(i);
        self.reindex();
        self.save();
        true
    }

    /// Borra las visitas desde el instante `since` (todo, con `0`).
    pub fn clear_since(&mut self, since: u64) {
        self.entries.retain(|v| v.last < since);
        self.reindex();
        self.save();
    }

    /// Las visitas cuyo título o URL contienen `query` ([`find::contains`]: sin distinguir
    /// mayúsculas ni, si la búsqueda no los lleva, diacríticos), la más reciente primero.
    pub fn search(&self, query: &str) -> Vec<&Visit> {
        let mut out: Vec<&Visit> = self
            .entries
            .iter()
            .filter(|v| find::contains(&v.title, query) || find::contains(&v.url, query))
            .collect();
        out.sort_by(|a, b| b.last.cmp(&a.last).then_with(|| a.url.cmp(&b.url)));
        out
    }

    /// Sugerencias para lo escrito en la barra de dirección: hasta `limit` visitas, primero las
    /// que empiezan con lo escrito (sin esquema ni `www.`), luego las que tienen una palabra
    /// del título o un trozo de la URL que empieza así, luego el resto que lo contiene; en cada
    /// grupo, por *frecencia* (visitas pesadas por lo reciente). `now` es el instante actual.
    pub fn suggest(&self, input: &str, limit: usize, now: u64) -> Vec<&Visit> {
        let q: String = find::fold_query(input).into_iter().collect();
        if q.is_empty() || limit == 0 {
            return Vec::new();
        }
        let q_bare = strip_url_prefix(&q);
        let mut scored: Vec<(u8, f64, &Visit)> = Vec::new();
        for v in &self.entries {
            let url: String = find::fold(&v.url, true).into_iter().collect();
            let title: String = find::fold(&v.title, find::has_marks(input))
                .into_iter()
                .collect();
            let bare = strip_url_prefix(&url);
            let rank = if url.starts_with(&q) || bare.starts_with(q_bare) {
                3
            } else if word_starts(&title, &q) || word_starts(bare, q_bare) {
                2
            } else if title.contains(&q) || url.contains(&q) {
                1
            } else {
                continue;
            };
            scored.push((rank, frecency(v, now), v));
        }
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(b.1.total_cmp(&a.1))
                .then_with(|| a.2.url.len().cmp(&b.2.url.len()))
        });
        scored.into_iter().take(limit).map(|(_, _, v)| v).collect()
    }

    /// Las visitas más recientes primero, hasta `limit`.
    pub fn recent(&self, limit: usize) -> Vec<&Visit> {
        let mut out: Vec<&Visit> = self.entries.iter().collect();
        out.sort_by(|a, b| b.last.cmp(&a.last).then_with(|| a.url.cmp(&b.url)));
        out.truncate(limit);
        out
    }

    /// Los sitios más visitados para la página de pestaña nueva (E11): hasta `limit` visitas
    /// por *frecencia* (las visitas pesadas por lo reciente), una por host para que un sitio
    /// con muchas páginas no ocupe todos los accesos (la más visitada de cada host).
    pub fn top_sites(&self, limit: usize, now: u64) -> Vec<&Visit> {
        let mut scored: Vec<(f64, &Visit)> =
            self.entries.iter().map(|v| (frecency(v, now), v)).collect();
        scored.sort_by(|a, b| {
            b.0.total_cmp(&a.0)
                .then(b.1.last.cmp(&a.1.last))
                .then_with(|| a.1.url.cmp(&b.1.url))
        });
        let mut hosts = std::collections::HashSet::new();
        let mut out = Vec::new();
        for (_, v) in scored {
            if out.len() >= limit {
                break;
            }
            if hosts.insert(site_key(&v.url)) {
                out.push(v);
            }
        }
        out
    }
}

/// El host de `url` sin `www.` (o la URL entera si no tiene host, como los `file:`).
fn site_key(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    match lower.split_once("://") {
        Some((scheme, rest)) if scheme != "file" => {
            let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
            host.strip_prefix("www.").unwrap_or(host).to_owned()
        }
        _ => lower,
    }
}

/// La URL sin esquema ni `www.` (para que "wiki" sugiera `https://es.wikipedia.org/`).
fn strip_url_prefix(s: &str) -> &str {
    let s = s.split_once("://").map_or(
        s,
        |(scheme, rest)| if scheme.contains('/') { s } else { rest },
    );
    s.strip_prefix("www.").unwrap_or(s)
}

/// ¿Alguna palabra de `s` (separadas por lo que no es letra ni número) empieza con `q`?
fn word_starts(s: &str, q: &str) -> bool {
    s.match_indices(q).any(|(i, _)| {
        s[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric())
    })
}

/// Visitas pesadas por lo reciente: pierden la mitad del peso cada 30 días.
fn frecency(v: &Visit, now: u64) -> f64 {
    let days = now.saturating_sub(v.last) as f64 / 86_400.0;
    f64::from(v.count) * 0.5f64.powf(days / 30.0)
}

/// El historial del navegador (el del perfil, o en memoria si no hay perfil).
pub fn global() -> MutexGuard<'static, History> {
    static GLOBAL: OnceLock<Mutex<History>> = OnceLock::new();
    GLOBAL
        .get_or_init(|| {
            Mutex::new(match profile::file("history") {
                Some(p) => History::open(p),
                None => History::memory(),
            })
        })
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggest_ranks_prefix_then_words() {
        let mut h = History::memory();
        h.record(
            "https://es.wikipedia.org/wiki/Navegador_web",
            "Navegador web",
            100,
        );
        h.record("https://example.com/wiki", "Un ejemplo", 100);
        h.record("https://news.ycombinator.com/", "Hacker News", 100);
        let urls = |q: &str| -> Vec<String> {
            h.suggest(q, 5, 100)
                .into_iter()
                .map(|v| v.url.clone())
                .collect()
        };
        assert_eq!(
            urls("es.wiki"),
            vec!["https://es.wikipedia.org/wiki/Navegador_web"]
        );
        // "wiki": la palabra del camino de example.com y la del host de Wikipedia empiezan así;
        // ninguna es prefijo de la URL sin esquema.
        assert_eq!(urls("wiki").len(), 2);
        assert_eq!(urls("NEWS"), vec!["https://news.ycombinator.com/"]);
        assert_eq!(urls("hacker"), vec!["https://news.ycombinator.com/"]);
        assert!(urls("").is_empty());
        assert!(!h.record("about:home", "Inicio", 1));
    }
}
