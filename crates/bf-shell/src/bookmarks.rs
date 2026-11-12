//! Los marcadores: páginas guardadas con `Ctrl+D` (o la estrella del campo de URL), en
//! carpetas simples (un nivel; las importadas con carpetas anidadas quedan como
//! `Padre / Hija`). Viven en el archivo `bookmarks` del perfil ([`crate::profile`]), un
//! registro por línea:
//!
//! ```text
//! # Freedom: marcadores v1
//! F\t<carpeta>
//! B\t<id>\t<agregado, s Unix>\t<carpeta>\t<URL>\t<título>
//! ```
//!
//! La carpeta vacía es la raíz ("Marcadores"). Cada cambio reescribe el archivo (son pocos).
//! [`Bookmarks::import_netscape`] lee el HTML de marcadores de Netscape
//! (`<!DOCTYPE NETSCAPE-Bookmark-file-1>`) que exportan todos los navegadores.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::find;
use crate::profile;

const HEADER: &str = "# Freedom: marcadores v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub id: u64,
    pub url: String,
    pub title: String,
    /// La carpeta (`""` es la raíz).
    pub folder: String,
    /// Cuándo se agregó, en segundos Unix.
    pub added: u64,
}

impl Bookmark {
    /// El título, o la URL si no tiene.
    pub fn label(&self) -> &str {
        if self.title.trim().is_empty() {
            &self.url
        } else {
            &self.title
        }
    }
}

/// Los marcadores. Sin `path` viven solo en memoria.
#[derive(Debug, Default)]
pub struct Bookmarks {
    path: Option<PathBuf>,
    /// En el orden en que se agregaron.
    items: Vec<Bookmark>,
    /// Las carpetas en orden de creación (sin la raíz).
    folders: Vec<String>,
    next_id: u64,
}

/// Un nombre de carpeta limpio: sin espacios en los extremos ni saltos.
fn clean_folder(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Bookmarks {
    pub fn memory() -> Bookmarks {
        Bookmarks {
            next_id: 1,
            ..Bookmarks::default()
        }
    }

    /// Lee los marcadores de `path` (si no existe, empieza vacío) y los guarda ahí.
    pub fn open(path: PathBuf) -> Bookmarks {
        let mut b = Bookmarks {
            path: Some(path),
            next_id: 1,
            ..Bookmarks::default()
        };
        let text = b
            .path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        for fields in text.lines().filter_map(profile::split_record) {
            match &fields[..] {
                [kind, name] if kind == "F" => {
                    let name = clean_folder(name);
                    if !name.is_empty() && !b.folders.contains(&name) {
                        b.folders.push(name);
                    }
                }
                [kind, id, added, folder, url, title] if kind == "B" => {
                    let (Ok(id), Ok(added)) = (id.parse::<u64>(), added.parse::<u64>()) else {
                        continue;
                    };
                    if b.items.iter().any(|x| x.id == id) {
                        continue;
                    }
                    let folder = clean_folder(folder);
                    if !folder.is_empty() && !b.folders.contains(&folder) {
                        b.folders.push(folder.clone());
                    }
                    b.next_id = b.next_id.max(id + 1);
                    b.items.push(Bookmark {
                        id,
                        url: url.clone(),
                        title: title.clone(),
                        folder,
                        added,
                    });
                }
                _ => {}
            }
        }
        b
    }

    /// Reescribe el archivo con el estado actual.
    pub fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let mut out = String::from(HEADER);
        out.push('\n');
        for f in &self.folders {
            out.push_str(&profile::join_record(&["F", f]));
            out.push('\n');
        }
        for b in &self.items {
            out.push_str(&profile::join_record(&[
                "B",
                &b.id.to_string(),
                &b.added.to_string(),
                &b.folder,
                &b.url,
                &b.title,
            ]));
            out.push('\n');
        }
        if let Err(e) = profile::write_atomic(path, &out) {
            eprintln!(
                "aviso: no se pudieron guardar los marcadores en {}: {e}",
                path.display()
            );
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[allow(dead_code, reason = "lo usan las pruebas (tests/bookmarks.rs)")]
    pub fn all(&self) -> &[Bookmark] {
        &self.items
    }

    /// Las carpetas, sin la raíz, en orden de creación.
    pub fn folders(&self) -> &[String] {
        &self.folders
    }

    pub fn get(&self, id: u64) -> Option<&Bookmark> {
        self.items.iter().find(|b| b.id == id)
    }

    /// El primer marcador de `url`, si hay.
    pub fn find_url(&self, url: &str) -> Option<&Bookmark> {
        self.items.iter().find(|b| b.url == url)
    }

    /// Los marcadores de `folder`, en orden.
    pub fn in_folder<'a>(&'a self, folder: &'a str) -> impl Iterator<Item = &'a Bookmark> + 'a {
        self.items.iter().filter(move |b| b.folder == folder)
    }

    fn add_unsaved(&mut self, url: &str, title: &str, folder: &str, added: u64) -> u64 {
        let folder = clean_folder(folder);
        if !folder.is_empty() && !self.folders.contains(&folder) {
            self.folders.push(folder.clone());
        }
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        self.items.push(Bookmark {
            id,
            url: url.to_owned(),
            title: title.trim().to_owned(),
            folder,
            added,
        });
        id
    }

    /// Agrega un marcador y devuelve su id.
    pub fn add(&mut self, url: &str, title: &str, folder: &str, added: u64) -> u64 {
        let id = self.add_unsaved(url, title, folder, added);
        self.save();
        id
    }

    /// Borra el marcador `id`; devuelve si estaba.
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.items.len();
        self.items.retain(|b| b.id != id);
        let removed = self.items.len() != before;
        if removed {
            self.save();
        }
        removed
    }

    /// `Ctrl+D`: si `url` no está, la agrega a la raíz y devuelve `true`; si está, quita sus
    /// marcadores y devuelve `false`.
    pub fn toggle(&mut self, url: &str, title: &str, now: u64) -> bool {
        if self.find_url(url).is_some() {
            self.items.retain(|b| b.url != url);
            self.save();
            false
        } else {
            self.add(url, title, "", now);
            true
        }
    }

    /// Cambia título, URL y carpeta del marcador `id`; devuelve si existe.
    pub fn update(&mut self, id: u64, title: &str, url: &str, folder: &str) -> bool {
        let folder = clean_folder(folder);
        let Some(b) = self.items.iter_mut().find(|b| b.id == id) else {
            return false;
        };
        b.title = title.trim().to_owned();
        if !url.trim().is_empty() {
            b.url = url.trim().to_owned();
        }
        b.folder = folder.clone();
        if !folder.is_empty() && !self.folders.contains(&folder) {
            self.folders.push(folder);
        }
        self.save();
        true
    }

    /// Crea una carpeta vacía; devuelve si no existía.
    pub fn add_folder(&mut self, name: &str) -> bool {
        let name = clean_folder(name);
        if name.is_empty() || self.folders.contains(&name) {
            return false;
        }
        self.folders.push(name);
        self.save();
        true
    }

    /// Borra la carpeta `name`; sus marcadores pasan a la raíz. Devuelve si existía.
    pub fn remove_folder(&mut self, name: &str) -> bool {
        let before = self.folders.len();
        self.folders.retain(|f| f != name);
        if self.folders.len() == before {
            return false;
        }
        for b in &mut self.items {
            if b.folder == name {
                b.folder.clear();
            }
        }
        self.save();
        true
    }

    /// Los marcadores cuyo título, URL o carpeta contienen `query` ([`find::contains`]).
    pub fn search(&self, query: &str) -> Vec<&Bookmark> {
        self.items
            .iter()
            .filter(|b| {
                find::contains(&b.title, query)
                    || find::contains(&b.url, query)
                    || find::contains(&b.folder, query)
            })
            .collect()
    }

    /// Importa un archivo de marcadores de Netscape (el HTML que exportan los navegadores):
    /// cada `<A HREF>` con el título de su texto y la carpeta de sus `<H3>` (las anidadas
    /// unidas con ` / `), en la carpeta `into` (vacía: tal cual). Se saltan las URLs que ya
    /// están en la misma carpeta y las que no son `http`, `https`, `file` ni `ftp` (los
    /// `javascript:` y `place:`). Devuelve cuántos se agregaron.
    pub fn import_netscape(&mut self, html: &str, into: &str, now: u64) -> usize {
        let mut added = 0;
        for (url, title, folders, date) in parse_netscape(html) {
            let scheme = url.split(':').next().unwrap_or("").to_ascii_lowercase();
            if !matches!(scheme.as_str(), "http" | "https" | "file" | "ftp") {
                continue;
            }
            let mut path: Vec<String> = Vec::new();
            if !clean_folder(into).is_empty() {
                path.push(clean_folder(into));
            }
            path.extend(
                folders
                    .iter()
                    .map(|f| clean_folder(f))
                    .filter(|f| !f.is_empty()),
            );
            let folder = path.join(" / ");
            if self
                .items
                .iter()
                .any(|b| b.url == url && b.folder == folder)
            {
                continue;
            }
            self.add_unsaved(&url, &title, &folder, date.unwrap_or(now));
            added += 1;
        }
        if added > 0 {
            self.save();
        }
        added
    }

    /// Los marcadores como archivo de Netscape, para llevarlos a otro navegador.
    pub fn export_netscape(&self) -> String {
        fn esc(s: &str) -> String {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
        }
        let mut out = String::from(
            "<!DOCTYPE NETSCAPE-Bookmark-file-1>\n<META HTTP-EQUIV=\"Content-Type\" \
             CONTENT=\"text/html; charset=UTF-8\">\n<TITLE>Bookmarks</TITLE>\n<H1>Bookmarks</H1>\n<DL><p>\n",
        );
        let link = |out: &mut String, b: &Bookmark, indent: &str| {
            out.push_str(&format!(
                "{indent}<DT><A HREF=\"{}\" ADD_DATE=\"{}\">{}</A>\n",
                esc(&b.url),
                b.added,
                esc(&b.title)
            ));
        };
        for f in &self.folders {
            out.push_str(&format!("    <DT><H3>{}</H3>\n    <DL><p>\n", esc(f)));
            for b in self.in_folder(f) {
                link(&mut out, b, "        ");
            }
            out.push_str("    </DL><p>\n");
        }
        for b in self.in_folder("") {
            link(&mut out, b, "    ");
        }
        out.push_str("</DL><p>\n");
        out
    }
}

/// Los enlaces de un archivo de marcadores de Netscape: `(URL, título, carpetas, ADD_DATE)`.
///
/// El formato no es HTML bien formado (`<DT>` y `<p>` sin cerrar), así que se lee con un
/// tokenizador propio y tolerante: `<H3>` abre el nombre de la próxima carpeta, `<DL>` entra en
/// ella y `</DL>` sale; `<A HREF>` hasta `</A>` es un marcador.
fn parse_netscape(html: &str) -> Vec<(String, String, Vec<String>, Option<u64>)> {
    let mut out = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    // Profundidad de `<DL>` sin carpeta (la lista raíz).
    let mut anonymous: Vec<bool> = Vec::new();
    let mut pending_folder: Option<String> = None;
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        rest = &rest[lt..];
        let Some(gt) = rest.find('>') else {
            break;
        };
        let tag = &rest[1..gt];
        let after = &rest[gt + 1..];
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        let closing = tag.starts_with('/');
        match (name.as_str(), closing) {
            ("h3", false) => {
                let end = find_ci(after, "</h3").unwrap_or(after.len());
                pending_folder = Some(decode_entities(&strip_tags(&after[..end])));
                rest = &after[end..];
                continue;
            }
            ("dl", false) => match pending_folder.take() {
                Some(f) => {
                    stack.push(f);
                    anonymous.push(false);
                }
                None => anonymous.push(true),
            },
            ("dl", true) => {
                if anonymous.pop() == Some(false) {
                    stack.pop();
                }
            }
            ("a", false) => {
                let end = find_ci(after, "</a").unwrap_or(after.len());
                let title = decode_entities(&strip_tags(&after[..end]));
                if let Some(href) = attr(tag, "href") {
                    let date = attr(tag, "add_date").and_then(|d| d.trim().parse().ok());
                    out.push((
                        decode_entities(&href),
                        title.trim().to_owned(),
                        stack.clone(),
                        date,
                    ));
                }
                rest = &after[end..];
                continue;
            }
            _ => {}
        }
        rest = after;
    }
    out
}

/// La posición de `needle` (en minúsculas ASCII) en `hay`, sin distinguir mayúsculas ASCII.
fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    let lower = hay.to_ascii_lowercase();
    lower.find(needle)
}

/// El valor del atributo `name` (minúsculas) dentro del texto de una etiqueta.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        // Que sea el nombre entero: precedido por espacio y seguido de `=`.
        if !lower[..at].ends_with(|c: char| c.is_ascii_whitespace()) {
            continue;
        }
        let after = tag[at + name.len()..].trim_start();
        let Some(value) = after.strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        return Some(match value.chars().next() {
            Some(q @ ('"' | '\'')) => value[1..].split(q).next().unwrap_or("").to_owned(),
            _ => value
                .split(|c: char| c.is_ascii_whitespace())
                .next()
                .unwrap_or("")
                .to_owned(),
        });
    }
    None
}

/// El texto sin etiquetas.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut inside = false;
    for c in s.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            c if !inside => out.push(c),
            _ => {}
        }
    }
    out
}

/// Las referencias de carácter de los archivos de marcadores: las cinco de XML y las
/// numéricas.
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..semi];
        let decoded = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            _ => name.strip_prefix('#').and_then(|n| {
                let code = match n.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => n.parse().ok(),
                };
                code.and_then(char::from_u32)
            }),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `Ctrl+D` (`⌘D` en macOS) en la ventana: guarda `url` en los marcadores o la quita si ya
/// estaba. Devuelve el aviso para la línea de estado; `key` es el atajo tal como se escribe en
/// esta plataforma.
pub fn toggle_page(url: &str, title: &str, key: &str) -> String {
    if global().toggle(url, title, profile::now()) {
        format!("Guardado en marcadores: {title} · {key} lo quita")
    } else {
        "Quitado de los marcadores".to_owned()
    }
}

/// Los marcadores del navegador (los del perfil, o en memoria si no hay perfil).
pub fn global() -> MutexGuard<'static, Bookmarks> {
    static GLOBAL: OnceLock<Mutex<Bookmarks>> = OnceLock::new();
    GLOBAL
        .get_or_init(|| {
            Mutex::new(match profile::file("bookmarks") {
                Some(p) => Bookmarks::open(p),
                None => Bookmarks::memory(),
            })
        })
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netscape_nested_folders() {
        let html = r#"<!DOCTYPE NETSCAPE-Bookmark-file-1>
<DL><p>
    <DT><H3 ADD_DATE="1">Barra</H3>
    <DL><p>
        <DT><A HREF="https://a.test/?x=1&amp;y=2" ADD_DATE="1700000000">A &amp; B</A>
        <DT><H3>Sub</H3>
        <DL><p>
            <DT><a href='https://b.test/'>B</a>
        </DL><p>
    </DL><p>
    <DT><A HREF="https://c.test/">C &#233;</A>
    <DT><A HREF="javascript:alert(1)">mal</A>
</DL><p>"#;
        let links = parse_netscape(html);
        assert_eq!(links.len(), 4);
        assert_eq!(
            links[0],
            (
                "https://a.test/?x=1&y=2".to_owned(),
                "A & B".to_owned(),
                vec!["Barra".to_owned()],
                Some(1_700_000_000)
            )
        );
        assert_eq!(links[1].2, vec!["Barra".to_owned(), "Sub".to_owned()]);
        assert_eq!(links[2].1, "C é");
        assert!(links[2].2.is_empty());
        let mut b = Bookmarks::memory();
        assert_eq!(b.import_netscape(html, "", 5), 3);
        assert_eq!(b.folders(), ["Barra".to_owned(), "Barra / Sub".to_owned()]);
        // Importar de nuevo no duplica.
        assert_eq!(b.import_netscape(html, "", 5), 0);
    }
}
