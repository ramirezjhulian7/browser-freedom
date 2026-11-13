//! Páginas internas (`about:` y `freedom://`), generadas como HTML y maquetadas por el mismo
//! motor que las páginas de la web: `about:home`, `about:error`, `about:engine`, `about:wpt`,
//! `about:` (el índice) y las del uso diario, `freedom://history`. Cada una responde con los
//! dos esquemas (`about:history` es `freedom://history`). También genera el reporte del motor
//! que muestra el panel lateral de la ventana.
//!
//! Las plantillas viven en `crates/bf-shell/pages/` y se embeben con `include_str!`; los
//! `{{nombre}}` se reemplazan con [`fill`]. Los colores llegan del [`Theme`] como custom
//! properties.

use std::fmt::Write as _;
use std::time::Duration;

use bf_dom::QuirksMode;
use bf_infra::Url;
use bf_paint::{Page, PageContext, prepare_page};

use crate::load::{ErrorKind, LoadContext, LoadError, Loaded, Meta, fetch_document};
use crate::theme::Theme;
use crate::window::keys;

pub const STYLE: &str = include_str!("../pages/style.css");
/// La marca sola, ámbar sobre transparente (256 px), para las páginas internas de los dos temas.
pub const MARK_256: &[u8] = include_bytes!("../assets/mark-256.png");

/// Una URL `data:image/png;base64,…` con `png`, para incrustar el logo en las páginas internas.
pub fn logo_data_url(png: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(png.len() / 3 * 4 + 30);
    out.push_str("data:image/png;base64,");
    for chunk in png.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, &b)| acc | (u32::from(b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}
const HOME: &str = include_str!("../pages/home.html");
const ERROR: &str = include_str!("../pages/error.html");
const ENGINE: &str = include_str!("../pages/engine.html");
pub const PANEL: &str = include_str!("../pages/panel.html");
const WPT: &str = include_str!("../pages/wpt.html");
const ABOUT: &str = include_str!("../pages/about.html");
const HISTORY: &str = include_str!("../pages/history.html");
const BOOKMARKS: &str = include_str!("../pages/bookmarks.html");
const DOWNLOADS: &str = include_str!("../pages/downloads.html");
const DOWNLOADED: &str = include_str!("../pages/downloaded.html");
const SETTINGS: &str = include_str!("../pages/settings.html");
pub const DLBAR: &str = include_str!("../pages/dlbar.html");
pub const BAR: &str = include_str!("../pages/bar.html");
pub const MENU: &str = include_str!("../pages/menu.html");
pub const SUGGEST: &str = include_str!("../pages/suggest.html");
pub const FIND: &str = include_str!("../pages/find.html");
pub const DIALOG: &str = include_str!("../pages/dialog.html");
pub const SOURCE: &str = include_str!("../pages/source.html");
pub const STATUS: &str = include_str!("../pages/status.html");
/// El estado de compatibilidad al compilar (lo regenera la corrida de reftests).
const WPT_STATUS: &str = include_str!("../../../docs/wpt-status.md");

/// Los atajos de la ventana para el uso diario (se muestran en `about:home`): cómo se escriben
/// en Windows y Linux, en macOS si cambian más que el modificador (`""`: igual), y qué hacen.
/// `Mod` es el modificador primario (`Ctrl`, `Cmd` en macOS); la tabla que los ejecuta está en
/// `window::keys`.
pub const SHORTCUTS: &[(&str, &str, &str)] = &[
    ("Mod+L", "", "Ir a la barra de dirección"),
    ("Mod+T · Mod+W", "", "Pestaña nueva y cerrar pestaña"),
    ("Mod+N", "", "Ventana nueva"),
    ("Ctrl+Tab · Mod+1…9", "", "Cambiar de pestaña"),
    (
        "Clic central · Mod+clic",
        "",
        "Abrir un enlace en otra pestaña",
    ),
    ("Alt+← · Alt+→", "Mod+[ · Mod+]", "Atrás y adelante"),
    ("Mod+R · F5", "", "Recargar"),
    ("Esc", "", "Detener la carga"),
    ("Alt+Inicio", "Mod+Shift+H", "Página de inicio"),
    ("Mod+H · Mod+J", "Mod+Y · Mod+J", "Historial y descargas"),
    (
        "Mod+D · Mod+Shift+B",
        "",
        "Guardar marcador y ver los marcadores",
    ),
    ("Mod+F", "", "Buscar en la página"),
    (
        "Mod++ · Mod+− · Mod+0",
        "",
        "Acercar, alejar y tamaño normal",
    ),
    ("Mod+A · Mod+C", "", "Seleccionar todo y copiar"),
    ("Espacio · Mayús+Espacio", "", "Página abajo y arriba"),
];

/// Las herramientas para quien desarrolla páginas o el navegador (`about:home`, aparte).
pub const DEV_SHORTCUTS: &[(&str, &str, &str)] = &[
    ("Mod+I", "", "Panel del motor"),
    ("F12", "", "Superposición de cajas"),
    ("Mod+S", "", "Guardar un PNG de la ventana"),
    ("Mod+Shift+D", "", "Volcar el layout"),
];

/// Cuántos sitios más visitados muestra la página de pestaña nueva (dos filas de cuatro).
const TOP_SITES: usize = 8;

/// Accesos de `about:home` cuando el historial está vacío.
const TILES: &[(&str, &str, &str)] = &[
    ("Google", "https://www.google.com/", "www.google.com"),
    (
        "Wikipedia",
        "https://es.wikipedia.org/wiki/Navegador_web",
        "es.wikipedia.org",
    ),
    (
        "Hacker News",
        "https://news.ycombinator.com/",
        "news.ycombinator.com",
    ),
    ("example.com", "https://example.com/", "example.com"),
];

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Reemplaza cada `{{key:Mod+D}}` de `template` por el atajo escrito como en esta plataforma
/// (`Ctrl+D`, `⌘D` en macOS; ver [`keys::label`]) y cada `{{clave}}` por su valor. Los atajos
/// se resuelven antes que los valores: el texto de una página nunca se toma por un atajo.
pub fn fill(template: &str, vars: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{key:") {
        let Some(len) = rest[start..].find("}}") else {
            break;
        };
        out.push_str(&rest[..start]);
        out.push_str(&escape(&shortcut(&rest[start + 6..start + len])));
        rest = &rest[start + len + 2..];
    }
    out.push_str(rest);
    for (key, value) in vars {
        out = out.replace(&format!("{{{{{key}}}}}"), value);
    }
    out
}

/// Un atajo (`Mod+D`) escrito como en esta plataforma.
pub fn shortcut(spec: &str) -> String {
    keys::label(spec, keys::MAC)
}

/// Una página interna con el tema y el estilo común.
fn themed(template: &str, theme: &Theme, vars: &[(&str, String)]) -> String {
    let mut all = vec![("theme", theme.css_vars()), ("style", STYLE.to_owned())];
    all.extend(vars.iter().cloned());
    fill(template, &all)
}

/// ¿`url` es una página interna (`about:` o `freedom:`)?
pub fn is_internal(url: &Url) -> bool {
    matches!(url.scheme.as_str(), "about" | "freedom")
}

/// El nombre de la página interna: el camino de `about:<nombre>` o el host de
/// `freedom://<nombre>`.
fn internal_name(url: &Url) -> String {
    let name = if url.scheme == "freedom" {
        let host = url.host_string();
        if host.is_empty() {
            url.path_string()
        } else {
            host
        }
    } else {
        url.path_string()
    };
    name.trim_matches('/').to_ascii_lowercase()
}

/// El valor del parámetro `name` de la query (`application/x-www-form-urlencoded`).
fn query_param(url: &Url, name: &str) -> Option<String> {
    let q = url.query.as_deref()?;
    bf_url::form::parse(q.as_bytes())
        .into_iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v)
}

/// La página interna para `url` (`about:...` o `freedom://...`).
pub fn page(url: &Url, ctx: &LoadContext) -> Loaded {
    let name = internal_name(url);
    let html = match name.as_str() {
        "blank" => String::new(),
        "" => themed(ABOUT, &ctx.theme, &[]),
        "home" | "newtab" => home(ctx),
        "history" => history(url, ctx),
        "bookmarks" => bookmarks(url, ctx),
        "downloads" => downloads_page(ctx),
        "settings" => settings_page(ctx),
        "wpt" => wpt(&ctx.theme),
        "engine" => engine(url, ctx),
        "source" => source(url, ctx),
        "error" => {
            // Vista previa: `about:error` o `about:error?dns|tls|timeout|404|...`.
            let err = sample_error(url.query.as_deref().unwrap_or(""));
            return error_page(url, &err, ctx);
        }
        _ => {
            let err = LoadError {
                kind: ErrorKind::UnknownInternal,
                message: format!("no existe la página interna {}", url.href()),
            };
            return error_page(url, &err, ctx);
        }
    };
    Loaded::generated(url, &html, ctx.net.clone(), Meta::default())
}

/// Lo que la ventana hace después de una acción del perfil.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// Volver a generar la página actual.
    pub reload: bool,
    /// Ir a otra página (en lugar de recargar).
    pub go: Option<Url>,
    /// Un aviso para la línea de estado.
    pub notice: Option<String>,
    /// Cambiaron las opciones ([`crate::settings`]): la ventana las aplica.
    pub settings: bool,
}

impl Outcome {
    fn reload() -> Outcome {
        Outcome {
            reload: true,
            ..Outcome::default()
        }
    }

    fn go(href: &str) -> Outcome {
        Outcome {
            go: Url::parse(href, None).ok(),
            ..Outcome::default()
        }
    }
}

/// Las acciones `bf:` de las páginas del perfil (historial, marcadores, descargas). Devuelve
/// `Some` si `url` es una de ellas; la ventana solo las acepta desde una página interna.
pub fn profile_action(url: &Url) -> Option<Outcome> {
    let now = crate::profile::now();
    let id = || query_param(url, "u").and_then(|v| v.parse::<u64>().ok());
    match url.path_string().as_str() {
        "history-remove" => {
            let target = query_param(url, "u")?;
            crate::history::global().remove(&target);
            Some(Outcome::reload())
        }
        "history-clear" => {
            let since = match url.query.as_deref().unwrap_or("") {
                "hour" => now.saturating_sub(3600),
                "day" => now - now % 86_400,
                "all" => 0,
                _ => return Some(Outcome::default()),
            };
            crate::history::global().clear_since(since);
            Some(Outcome::reload())
        }
        "bookmark-remove" => {
            crate::bookmarks::global().remove(id()?);
            Some(Outcome::reload())
        }
        "bookmark-save" => {
            let id = query_param(url, "id")?.parse().ok()?;
            let field = |n: &str| query_param(url, n).unwrap_or_default();
            crate::bookmarks::global().update(id, &field("title"), &field("url"), &field("folder"));
            Some(Outcome::go("freedom://bookmarks"))
        }
        "bookmark-folder" => {
            crate::bookmarks::global().add_folder(&query_param(url, "u")?);
            Some(Outcome::reload())
        }
        "bookmark-folder-remove" => {
            crate::bookmarks::global().remove_folder(&query_param(url, "u")?);
            Some(Outcome::reload())
        }
        "bookmark-import" => {
            let Some(path) = rfd::FileDialog::new()
                .set_title("Importar marcadores (HTML de Netscape)")
                .add_filter("Marcadores HTML", &["html", "htm"])
                .pick_file()
            else {
                return Some(Outcome::default());
            };
            let notice = match std::fs::read(&path) {
                Ok(bytes) => {
                    let text = bf_infra::decode_text(&bytes);
                    let n = crate::bookmarks::global().import_netscape(&text, "Importados", now);
                    format!("Importados {n} marcadores de {}", path.display())
                }
                Err(e) => format!("No se pudo leer {}: {e}", path.display()),
            };
            Some(Outcome {
                reload: true,
                notice: Some(notice),
                ..Outcome::default()
            })
        }
        "bookmark-export" => {
            let Some(path) = rfd::FileDialog::new()
                .set_title("Exportar marcadores")
                .set_file_name("marcadores-freedom.html")
                .save_file()
            else {
                return Some(Outcome::default());
            };
            let html = crate::bookmarks::global().export_netscape();
            let notice = match std::fs::write(&path, html) {
                Ok(()) => format!("Marcadores exportados a {}", path.display()),
                Err(e) => format!("No se pudo guardar {}: {e}", path.display()),
            };
            Some(Outcome {
                notice: Some(notice),
                ..Outcome::default()
            })
        }
        "download-show" => {
            let path = crate::downloads::global()
                .get(id()?)
                .map(|d| d.path.clone());
            if let Some(p) = path {
                crate::downloads::reveal(&p);
            }
            Some(Outcome::default())
        }
        "download-cancel" => {
            crate::downloads::global().cancel(id()?);
            Some(Outcome::reload())
        }
        "download-forget" => {
            crate::downloads::global().forget(id()?);
            Some(Outcome::reload())
        }
        "setting-third-party-cookies" => {
            let mut s = crate::settings::global();
            s.block_third_party_cookies = !s.block_third_party_cookies;
            s.save();
            Some(Outcome {
                reload: true,
                settings: true,
                ..Outcome::default()
            })
        }
        "setting-restore-session" => {
            let mut s = crate::settings::global();
            s.restore_session = !s.restore_session;
            s.save();
            Some(Outcome::reload())
        }
        "setting-text-zoom" => {
            let mut s = crate::settings::global();
            s.text_zoom = match url.query.as_deref().unwrap_or("") {
                "in" => s.text_zoom_step(true),
                "out" => s.text_zoom_step(false),
                "reset" => 1.0,
                _ => return Some(Outcome::default()),
            };
            s.save();
            Some(Outcome {
                reload: true,
                settings: true,
                ..Outcome::default()
            })
        }
        "download-clear" => {
            crate::downloads::global().clear_finished();
            Some(Outcome::reload())
        }
        _ => None,
    }
}

/// El desvío de `load.rs` (E6) para las descargas: si `response` no es para mostrar
/// ([`crate::downloads::should_download`]), su cuerpo se guarda en la carpeta de descargas y
/// el documento es una página interna que lo dice (con `Meta::download`); la ventana vuelve a
/// la página anterior si la había.
pub fn download_response(
    response: &bf_net::Response,
    net: &std::sync::Arc<bf_net::NetLoader>,
) -> Option<Loaded> {
    let content_type = response.headers.get("content-type");
    let disposition = response.headers.get("content-disposition");
    if !response.ok() || !crate::downloads::should_download(content_type, disposition) {
        return None;
    }
    let name = crate::downloads::filename_for(&response.url, disposition, content_type);
    let theme = Theme::for_system(None);
    let (title, detail, saved) =
        match crate::downloads::save_bytes(&response.url.href(), &name, &response.body) {
            Ok(path) => (
                "Descarga terminada".to_owned(),
                format!(
                    "Se guardó {} ({}).",
                    path.display(),
                    crate::downloads::human_size(response.body.len() as u64)
                ),
                path.display().to_string(),
            ),
            Err(e) => (
                "La descarga falló".to_owned(),
                format!("No se pudo guardar {name}: {e}"),
                name.clone(),
            ),
        };
    let html = themed(
        DOWNLOADED,
        &theme,
        &[("title", escape(&title)), ("detail", escape(&detail))],
    );
    let meta = Meta {
        download: Some(saved),
        content_type: content_type.map(str::to_owned),
        ..Meta::default()
    };
    Some(Loaded::generated(&response.url, &html, net.clone(), meta))
}

/// Una imagen abierta como documento (HTML §7.4.6, "Page load processing model for media"):
/// un documento HTML con un `<img>` cuyo `src` es la URL. SVG se trata como XML en `load.rs`.
pub fn image_document(
    response: &bf_net::Response,
    net: &std::sync::Arc<bf_net::NetLoader>,
) -> Option<Loaded> {
    let essence = response
        .headers
        .get("content-type")?
        .split(';')
        .next()?
        .trim()
        .to_ascii_lowercase();
    if !response.ok() || !essence.starts_with("image/") || essence == "image/svg+xml" {
        return None;
    }
    let href = response.url.href();
    let name = response
        .url
        .path_string()
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty())
        .map_or_else(|| response.url.host_string(), str::to_owned);
    let html = format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>{}</title></head>         <body style=\"margin:0\"><img src=\"{}\" alt=\"\"></body></html>",
        escape(&name),
        escape(&href)
    );
    let meta = Meta {
        status: Some((response.status, response.status_text.clone())),
        content_type: Some(essence),
        ..Meta::default()
    };
    let mut loaded = Loaded::generated(&response.url, &html, net.clone(), meta);
    // No es una página del navegador: no puede disparar acciones `bf:`.
    loaded.meta.internal = false;
    loaded.meta.bytes = response.body.len();
    Some(loaded)
}

/// `freedom://settings`: las opciones del navegador ([`crate::settings`]).
fn settings_page(ctx: &LoadContext) -> String {
    let s = crate::settings::global().clone();
    let profile = crate::profile::dir().map_or_else(
        || "en memoria (no se guarda)".to_owned(),
        |d| d.display().to_string(),
    );
    themed(
        SETTINGS,
        &ctx.theme,
        &[
            (
                "cookies_on",
                if s.block_third_party_cookies {
                    " on"
                } else {
                    ""
                }
                .to_owned(),
            ),
            (
                "cookies_state",
                if s.block_third_party_cookies {
                    "Activado"
                } else {
                    "Desactivado"
                }
                .to_owned(),
            ),
            (
                "restore_on",
                if s.restore_session { " on" } else { "" }.to_owned(),
            ),
            (
                "restore_state",
                if s.restore_session {
                    "Activado"
                } else {
                    "Desactivado"
                }
                .to_owned(),
            ),
            ("text_zoom", format!("{} %", (s.text_zoom * 100.0).round())),
            (
                "downloads_dir",
                escape(&crate::downloads::downloads_dir().display().to_string()),
            ),
            ("profile", escape(&profile)),
        ],
    )
}

/// `freedom://downloads`: las descargas con su estado, abrir la carpeta, cancelar y quitar.
fn downloads_page(ctx: &LoadContext) -> String {
    let g = crate::downloads::global();
    let list = g.list();
    let mut rows = String::new();
    for d in &list {
        let state = d.status();
        let bad = if matches!(d.state, crate::downloads::State::Failed(_)) {
            " bad"
        } else {
            ""
        };
        let track = match (&d.state, d.fraction()) {
            (crate::downloads::State::Running, Some(f)) => format!(
                "<span class=\"track\"><span class=\"fill\" style=\"width:{:.1}%\"></span></span>",
                f * 100.0
            ),
            _ => String::new(),
        };
        let id = d.id.to_string();
        let acts = match d.state {
            crate::downloads::State::Running => format!(
                "<a href=\"{}\">Cancelar</a>",
                action_href("download-cancel", &id)
            ),
            crate::downloads::State::Done => format!(
                "<a href=\"{}\">Mostrar en la carpeta</a><a href=\"{}\">Quitar</a>",
                action_href("download-show", &id),
                action_href("download-forget", &id)
            ),
            _ => format!(
                "<a href=\"{}\">Reintentar</a><a href=\"{}\">Quitar</a>",
                escape(&d.url),
                action_href("download-forget", &id)
            ),
        };
        let _ = write!(
            rows,
            "<div class=\"row\"><span class=\"main\"><span class=\"n\">{name}</span>\
             <span class=\"u\">{url}</span><span class=\"s{bad}\">{day} {clock} · {state}</span>{track}</span>\
             <span class=\"acts\">{acts}</span></div>",
            name = escape(&d.name()),
            url = escape(&d.url),
            day = crate::profile::format_day(d.started),
            clock = crate::profile::format_clock(d.started),
            state = escape(&state),
        );
    }
    if rows.is_empty() {
        rows = "<p class=\"empty\">Todavía no descargaste nada.</p>".to_owned();
    }
    let running = g.running();
    let summary = match (list.len(), running) {
        (0, _) => "Los archivos que descargues aparecen aquí.".to_owned(),
        (n, 0) => format!("{n} descargas."),
        (n, r) => format!("{n} descargas, {r} en curso."),
    };
    drop(g);
    themed(
        DOWNLOADS,
        &ctx.theme,
        &[
            ("summary", escape(&summary)),
            ("rows", rows),
            (
                "dir",
                escape(&crate::downloads::downloads_dir().display().to_string()),
            ),
        ],
    )
}

/// `freedom://bookmarks[?q=…|?edit=<id>]`: los marcadores por carpeta (o lo que coincide con
/// la búsqueda), con editar, borrar, carpetas e importar/exportar.
fn bookmarks(url: &Url, ctx: &LoadContext) -> String {
    let query = query_param(url, "q").unwrap_or_default();
    let searching = !query.trim().is_empty();
    let b = crate::bookmarks::global();
    let row = |out: &mut String, m: &crate::bookmarks::Bookmark| {
        let _ = write!(
            out,
            "<div class=\"row\"><a class=\"t\" href=\"{href}\">{title}</a><span class=\"h\">{host}</span>\
             <a class=\"e\" href=\"freedom://bookmarks?edit={id}\">Editar</a>\
             <a class=\"x\" href=\"{remove}\" title=\"Borrar el marcador\">×</a></div>",
            href = escape(&m.url),
            title = escape(m.label()),
            host = escape(&host_of(&m.url)),
            id = m.id,
            remove = action_href("bookmark-remove", &m.id.to_string()),
        );
    };
    let mut folders = String::new();
    if searching {
        let found = b.search(&query);
        let _ = write!(folders, "<h2>Resultados</h2><div class=\"card day\">");
        if found.is_empty() {
            let _ = write!(
                folders,
                "<p class=\"empty\">Nada coincide con «{}».</p>",
                escape(query.trim())
            );
        }
        for m in found {
            row(&mut folders, m);
        }
        folders.push_str("</div>");
    } else {
        let names = std::iter::once(String::new()).chain(b.folders().iter().cloned());
        for name in names {
            let items: Vec<_> = b.in_folder(&name).collect();
            if name.is_empty() && items.is_empty() && !b.is_empty() {
                continue;
            }
            let remove = if name.is_empty() {
                String::new()
            } else {
                format!(
                    "<a href=\"{}\">Borrar carpeta</a>",
                    action_href("bookmark-folder-remove", &name)
                )
            };
            let label = if name.is_empty() { "Marcadores" } else { &name };
            let _ = write!(
                folders,
                "<div class=\"folder\"><h2>{}</h2>{remove}</div><div class=\"card day\">",
                escape(label)
            );
            if items.is_empty() {
                let empty = if name.is_empty() {
                    format!(
                        "Todavía no hay marcadores. Pulsa {} en una página para guardarla.",
                        shortcut("Mod+D")
                    )
                } else {
                    "Carpeta vacía.".to_owned()
                };
                let _ = write!(folders, "<p class=\"empty\">{empty}</p>");
            }
            for m in items {
                row(&mut folders, m);
            }
            folders.push_str("</div>");
        }
    }
    let edit = query_param(url, "edit")
        .and_then(|v| v.parse::<u64>().ok())
        .and_then(|id| b.get(id))
        .map(|m| {
            let chips: String = b
                .folders()
                .iter()
                .map(|f| format!("<span>{}</span>", escape(f)))
                .collect();
            format!(
                "<form class=\"card edit\" action=\"bf:bookmark-save\" method=\"get\">\
                 <input type=\"hidden\" name=\"id\" value=\"{id}\">\
                 <label class=\"field\"><span>Nombre</span><input name=\"title\" value=\"{title}\"></label>\
                 <label class=\"field\"><span>Dirección</span><input name=\"url\" value=\"{href}\"></label>\
                 <label class=\"field\"><span>Carpeta</span><input name=\"folder\" value=\"{folder}\" placeholder=\"(Marcadores)\"></label>\
                 <div class=\"chips\">{chips}</div>\
                 <div class=\"actions\"><a href=\"freedom://bookmarks\">Cancelar</a><button type=\"submit\">Guardar</button></div></form>",
                id = m.id,
                title = escape(&m.title),
                href = escape(&m.url),
                folder = escape(&m.folder),
            )
        })
        .unwrap_or_default();
    let summary = match b.len() {
        0 => "Las páginas que guardes aparecen aquí.".to_owned(),
        1 => "1 marcador.".to_owned(),
        n => format!("{n} marcadores en {} carpetas.", b.folders().len() + 1),
    };
    drop(b);
    let path = crate::profile::file("bookmarks").map_or_else(
        || "memoria (sin perfil)".to_owned(),
        |p| p.display().to_string(),
    );
    themed(
        BOOKMARKS,
        &ctx.theme,
        &[
            ("summary", escape(&summary)),
            ("query", escape(&query)),
            ("edit", edit),
            ("folders", folders),
            ("path", escape(&path)),
        ],
    )
}

/// Un enlace `bf:<accion>?u=<valor>`, ya escapado para un atributo.
fn action_href(action: &str, value: &str) -> String {
    let q = bf_url::form::serialize(&[("u".to_owned(), value.to_owned())]);
    escape(&format!("bf:{action}?{q}"))
}

/// El host de una URL para mostrar junto al título (o el esquema, si no tiene).
fn host_of(href: &str) -> String {
    Url::parse(href, None).map_or_else(
        |_| String::new(),
        |u| {
            let h = u.host_string();
            if h.is_empty() { u.scheme.clone() } else { h }
        },
    )
}

/// Visitas que muestra `freedom://history`.
const HISTORY_SHOWN: usize = 400;

/// `freedom://history[?q=…]`: el historial por día (o lo que coincide con la búsqueda), con
/// borrar por entrada y por periodo, y las páginas de esta pestaña.
fn history(url: &Url, ctx: &LoadContext) -> String {
    let query = query_param(url, "q").unwrap_or_default();
    let searching = !query.trim().is_empty();
    let h = crate::history::global();
    let list = if searching {
        let mut l = h.search(&query);
        l.truncate(HISTORY_SHOWN);
        l
    } else {
        h.recent(HISTORY_SHOWN)
    };
    let summary = match (searching, list.len()) {
        (false, _) if h.is_empty() => "Todavía no hay nada en el historial.".to_owned(),
        (false, n) => format!(
            "{} páginas visitadas; se muestran las {n} más recientes.",
            h.len()
        ),
        (true, 0) => format!("Nada coincide con «{}».", query.trim()),
        (true, n) => format!("{n} páginas coinciden con «{}».", query.trim()),
    };
    let mut days = String::new();
    let mut current_day = String::new();
    for v in &list {
        let day = crate::profile::format_day(v.last);
        if day != current_day {
            if !current_day.is_empty() {
                days.push_str("</div>");
            }
            let _ = write!(days, "<h2>{day}</h2><div class=\"card day\">");
            current_day = day;
        }
        let _ = write!(
            days,
            "<div class=\"row\"><span class=\"time\">{time}</span><a class=\"t\" href=\"{href}\">{title}</a>\
             <span class=\"h\">{host}</span><span class=\"n\">{count}</span>\
             <a class=\"x\" href=\"{remove}\" title=\"Borrar del historial\">×</a></div>",
            time = crate::profile::format_clock(v.last),
            href = escape(&v.url),
            title = escape(v.label()),
            host = escape(&host_of(&v.url)),
            count = if v.count > 1 {
                format!("×{}", v.count)
            } else {
                String::new()
            },
            remove = action_href("history-remove", &v.url),
        );
    }
    if !current_day.is_empty() {
        days.push_str("</div>");
    }
    drop(h);
    let mut tab = String::new();
    if !searching && !ctx.recent.is_empty() {
        tab.push_str("<h2>En esta pestaña</h2><div class=\"card day\">");
        for (title, href) in ctx.recent.iter().take(12) {
            let _ = write!(
                tab,
                "<div class=\"row\"><a class=\"t\" href=\"{h}\">{t}</a><span class=\"h\">{host}</span></div>",
                h = escape(href),
                t = escape(if title.is_empty() { href } else { title }),
                host = escape(&host_of(href)),
            );
        }
        tab.push_str("</div>");
    }
    let path = crate::profile::file("history").map_or_else(
        || "memoria (sin perfil)".to_owned(),
        |p| p.display().to_string(),
    );
    themed(
        HISTORY,
        &ctx.theme,
        &[
            ("summary", escape(&summary)),
            ("query", escape(&query)),
            ("tab", tab),
            ("days", days),
            ("path", escape(&path)),
            (
                "links",
                " · <a href=\"freedom://bookmarks\">Marcadores</a>".to_owned(),
            ),
        ],
    )
}

/// `about:source?<url>`: el código fuente de esa URL, con números de línea.
fn source(url: &Url, ctx: &LoadContext) -> String {
    let target = url.query.as_deref().and_then(|q| Url::parse(q, None).ok());
    let (title, body) = match target {
        Some(t) => match bf_infra::ResourceLoader::load(&*ctx.net, &t) {
            Some(res) => {
                let (text, _) =
                    bf_infra::encoding::decode(&res.bytes, bf_infra::encoding::Encoding::Utf8);
                let mut lines = String::new();
                for (i, line) in text.lines().enumerate() {
                    let _ = writeln!(lines, "<span class=\"n\">{}</span>{}", i + 1, escape(line));
                }
                (format!("Código fuente de {}", t.href()), lines)
            }
            None => (
                format!("Código fuente de {}", t.href()),
                "No se pudo cargar.".to_owned(),
            ),
        },
        None => (
            "Código fuente".to_owned(),
            "Falta la URL: about:source?https://…".to_owned(),
        ),
    };
    themed(
        SOURCE,
        &ctx.theme,
        &[("title", escape(&title)), ("lines", body)],
    )
}

fn home(ctx: &LoadContext) -> String {
    // Los sitios más visitados del historial (E11); sin historial, unos accesos de ejemplo.
    let top: Vec<(String, String, String)> = crate::history::global()
        .top_sites(TOP_SITES, crate::profile::now())
        .into_iter()
        .map(|v| (v.label().to_owned(), v.url.clone(), host_of(&v.url)))
        .collect();
    let top = if top.is_empty() {
        TILES
            .iter()
            .map(|(n, h, o)| ((*n).to_owned(), (*h).to_owned(), (*o).to_owned()))
            .collect()
    } else {
        top
    };
    let mut tiles = String::new();
    for (name, href, host) in &top {
        let letter = host
            .strip_prefix("www.")
            .unwrap_or(host)
            .chars()
            .chain(name.chars())
            .find(|c| c.is_alphanumeric())
            .unwrap_or('?')
            .to_uppercase();
        let _ = write!(
            tiles,
            "<a class=\"tile\" href=\"{href}\" title=\"{name}\"><span class=\"frame\">{letter}\
             <span class=\"gap\"></span></span><span class=\"name\">{name}</span>\
             <span class=\"host\">{host}</span></a>",
            href = escape(href),
            name = escape(name),
            host = escape(host),
        );
    }
    let mut recent = String::new();
    for (title, href) in ctx.recent.iter().take(8) {
        let _ = write!(
            recent,
            "<a href=\"{h}\"><span class=\"t\">{t}</span><span class=\"u\">{h}</span></a>",
            h = escape(href),
            t = escape(title),
        );
    }
    if recent.is_empty() {
        recent =
            "<p class=\"empty\">Todavía no visitaste ninguna página en esta sesión.</p>".into();
    }
    let keys = |list: &[(&str, &str, &str)]| {
        let mut out = String::new();
        for (other, mac, d) in list {
            let spec = if keys::MAC && !mac.is_empty() {
                mac
            } else {
                other
            };
            // Cada combinación en su tecla: "Ctrl+R · F5" son dos.
            let chips: String = keys::label(spec, keys::MAC)
                .split(" · ")
                .map(|c| format!("<span class=\"kbd\">{}</span>", escape(c)))
                .collect();
            let _ = write!(
                out,
                "<div class=\"key\"><span class=\"k\">{chips}</span><span class=\"d\">{}</span></div>",
                escape(d)
            );
        }
        out
    };
    themed(
        HOME,
        &ctx.theme,
        &[
            (
                "mark",
                format!("<img src=\"{}\" alt=\"\">", logo_data_url(MARK_256)),
            ),
            ("tiles", tiles),
            ("recent", recent),
            ("shortcuts", keys(SHORTCUTS)),
            ("dev_shortcuts", keys(DEV_SHORTCUTS)),
        ],
    )
}

fn sample_error(query: &str) -> LoadError {
    let kind = match query {
        "dns" => ErrorKind::Dns,
        "tls" => ErrorKind::Tls,
        "timeout" => ErrorKind::Timeout,
        "conexion" | "connection" => ErrorKind::Connection,
        q if q.parse::<u16>().is_ok() => ErrorKind::Http(q.parse().unwrap_or(500)),
        _ => ErrorKind::Dns,
    };
    LoadError {
        kind,
        message: "vista previa de la página de error".into(),
    }
}

/// Título, etiqueta y explicación de un tipo de error.
fn describe(kind: ErrorKind) -> (String, String, &'static str) {
    match kind {
        ErrorKind::Dns => (
            "No se encontró el sitio".into(),
            "DNS".into(),
            "No se encontró ese sitio. Revisa que la dirección esté bien escrita y que tengas conexión a internet.",
        ),
        ErrorKind::Tls => (
            "La conexión segura falló".into(),
            "Conexión segura".into(),
            "No se pudo comprobar que el sitio sea quien dice ser: su certificado no es válido o la conexión segura no se completó.",
        ),
        ErrorKind::Timeout => (
            "El sitio tardó demasiado".into(),
            "Tiempo agotado".into(),
            "El sitio no respondió a tiempo. Puede estar caído o tu conexión ser lenta; prueba de nuevo en un momento.",
        ),
        ErrorKind::Connection => (
            "No se pudo conectar".into(),
            "Conexión".into(),
            "El sitio rechazó la conexión o la cortó antes de responder.",
        ),
        ErrorKind::Http(code) => (
            format!("El sitio respondió con un error ({code})"),
            format!("HTTP {code}"),
            if code < 500 {
                "La página no existe o no tienes permiso para verla."
            } else {
                "El sitio tuvo un problema al responder. No es un error de Freedom; prueba más tarde."
            },
        ),
        ErrorKind::NotFound => (
            "No se encontró el archivo".into(),
            "Archivo".into(),
            "Ese archivo no existe en tu equipo o no se puede leer.",
        ),
        ErrorKind::InvalidUrl => (
            "La dirección no es válida".into(),
            "URL".into(),
            "Lo escrito no tiene el formato de una dirección web.",
        ),
        ErrorKind::UnsupportedScheme => (
            "Esquema no soportado".into(),
            "Esquema".into(),
            "Freedom abre direcciones http, https, file, data y about.",
        ),
        ErrorKind::TooManyRedirects => (
            "Demasiadas redirecciones".into(),
            "Redirecciones".into(),
            "La página redirige a otra una y otra vez (más de 20 veces) y nunca termina de cargar.",
        ),
        ErrorKind::Engine => (
            "El motor falló con esta página".into(),
            "Motor".into(),
            "Es un error de Freedom, no de la página. El detalle de abajo sirve para reportarlo.",
        ),
        ErrorKind::UnknownInternal => (
            "Página interna desconocida".into(),
            "about:".into(),
            "Las páginas internas están listadas en about:.",
        ),
    }
}

/// `about:error` para `url` con el error `err`.
pub fn error_page(url: &Url, err: &LoadError, ctx: &LoadContext) -> Loaded {
    let (title, kind, explanation) = describe(err.kind);
    let html = themed(
        ERROR,
        &ctx.theme,
        &[
            ("title", escape(&title)),
            ("kind", escape(&kind)),
            ("explanation", escape(explanation)),
            ("url", escape(&url.href())),
            ("detail", escape(&err.message)),
        ],
    );
    let meta = Meta {
        error: Some(err.clone()),
        ..Meta::default()
    };
    Loaded::generated(url, &html, ctx.net.clone(), meta)
}

/// `about:engine?<url>`: carga y maqueta `<url>` y muestra su reporte.
fn engine(url: &Url, ctx: &LoadContext) -> String {
    let target = url.query.as_deref().and_then(|q| {
        Url::parse(
            &String::from_utf8_lossy(&bf_url::percent::decode(q.as_bytes())),
            None,
        )
        .ok()
    });
    let Some(target) = target else {
        let body = format!(
            "<div class=\"section\"><p>Abre <code>about:engine</code> desde una página (o pulsa \
             <b>{}</b>) para ver su reporte, o escribe <code>about:engine?https://…</code>.</p></div>",
            escape(&shortcut("Mod+I"))
        );
        return themed(
            ENGINE,
            &ctx.theme,
            &[("subject", "ninguna página".into()), ("body", body)],
        );
    };
    let fetched = if is_internal(&target) {
        Ok(page(&target, ctx))
    } else {
        fetch_document(&target, ctx.net.clone())
    };
    let loaded = match fetched {
        Ok(l) => l,
        Err(e) => {
            let body = format!(
                "<div class=\"section\"><p class=\"bad\">No se pudo cargar: {}</p></div>",
                escape(&e.message)
            );
            return themed(
                ENGINE,
                &ctx.theme,
                &[("subject", escape(&target.href())), ("body", body)],
            );
        }
    };
    let pctx = PageContext {
        base: &loaded.base,
        encoding: loaded.encoding,
        loader: loaded.loader.clone(),
        fonts: ctx.fonts.clone(),
        extra: &[],
        text_zoom: 1.0,
        device_pixel_ratio: 1.0,
    };
    let page = prepare_page(&loaded.doc, &pctx, ctx.viewport);
    themed(
        ENGINE,
        &ctx.theme,
        &[
            ("subject", escape(&loaded.url.href())),
            ("body", engine_body(&loaded, &page, None, false)),
        ],
    )
}

fn ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}

fn kv(out: &mut String, k: &str, v: &str) {
    let _ = write!(
        out,
        "<div class=\"kv\"><span class=\"k\">{}</span><span class=\"v\">{v}</span></div>",
        escape(k)
    );
}

fn chips<'a>(items: impl Iterator<Item = (&'a str, usize)>) -> String {
    let mut out = String::from("<div class=\"chips\">");
    for (name, n) in items {
        let _ = write!(out, "<span class=\"chip\">{} ×{n}</span>", escape(name));
    }
    out.push_str("</div>");
    out
}

/// El reporte del motor para un documento ya maquetado: tiempos por fase, datos del
/// documento, lo que se ignoró, las fuentes y (en el panel) las acciones.
/// Las entradas de la consola que se muestran en el panel.
const CONSOLE_SHOWN: usize = 60;

/// La sección "Consola" del panel del motor: cuántos mensajes hay de cada nivel y los últimos
/// (los más recientes arriba).
pub fn console_section(entries: &[crate::script::ConsoleEntry]) -> String {
    use bf_js::host::ConsoleLevel;
    let count = |l: ConsoleLevel| entries.iter().filter(|e| e.level == l).count();
    let mut out = String::from("<div class=\"section\"><h3>Consola</h3>");
    if entries.is_empty() {
        out.push_str("<p class=\"muted\">Sin mensajes.</p></div>");
        return out;
    }
    kv(
        &mut out,
        "Mensajes",
        &format!(
            "{} errores · {} avisos · {} otros",
            count(ConsoleLevel::Error),
            count(ConsoleLevel::Warn),
            entries.len() - count(ConsoleLevel::Error) - count(ConsoleLevel::Warn)
        ),
    );
    for e in entries.iter().rev().take(CONSOLE_SHOWN) {
        let class = match e.level {
            ConsoleLevel::Error => "bad",
            ConsoleLevel::Warn => "warn",
            _ => "muted",
        };
        let first = e.text.lines().next().unwrap_or_default();
        let _ = write!(
            out,
            "<p class=\"{class}\" style=\"font-family:monospace;margin:2px 0\">{}</p>",
            escape(first)
        );
    }
    out.push_str("</div>");
    out
}

pub fn engine_body(
    loaded: &Loaded,
    page: &Page,
    raster: Option<Duration>,
    actions: bool,
) -> String {
    let t = page.timings;
    let r = &page.report;
    let mut out = String::new();

    // Tiempos
    let phases: Vec<(&str, Option<Duration>)> = vec![
        ("fetch", Some(loaded.fetch)),
        ("parse", Some(loaded.parse)),
        ("estilo", Some(t.style)),
        ("imágenes", Some(t.images)),
        ("layout", Some(t.layout)),
        ("pintura", Some(t.paint)),
        ("raster", raster),
    ];
    let max = phases
        .iter()
        .filter_map(|(_, d)| *d)
        .max()
        .unwrap_or_default()
        .as_secs_f64()
        .max(1e-6);
    out.push_str("<div class=\"section\"><h3>Tiempos</h3>");
    for (name, d) in &phases {
        let (pct, label) = match d {
            Some(d) => ((d.as_secs_f64() / max * 100.0).max(1.0), ms(*d)),
            None => (0.0, "—".to_owned()),
        };
        let _ = write!(
            out,
            "<div class=\"phase\"><span class=\"k\">{name}</span><span class=\"track\">\
             <span class=\"fill\" style=\"width:{pct:.1}%\"></span></span><span class=\"v\">{label}</span></div>"
        );
    }
    out.push_str("</div>");

    // Documento
    let m = &loaded.meta;
    out.push_str("<div class=\"section\"><h3>Documento</h3>");
    kv(&mut out, "URL final", &escape(&loaded.url.href()));
    if !m.internal {
        kv(&mut out, "Redirecciones", &m.redirects.to_string());
    }
    match &m.status {
        Some((code, text)) => {
            let class = if *code >= 400 { "bad" } else { "" };
            kv(
                &mut out,
                "HTTP",
                &format!("<span class=\"{class}\">{code} {}</span>", escape(text)),
            );
        }
        None if m.internal => kv(&mut out, "Origen", "página interna"),
        None => kv(&mut out, "Origen", &escape(&loaded.url.scheme)),
    }
    if let Some(ct) = &m.content_type {
        kv(&mut out, "Tipo", &escape(ct));
    }
    if let Some(err) = &m.error {
        kv(
            &mut out,
            "Error",
            &format!("<span class=\"bad\">{}</span>", escape(&err.message)),
        );
    }
    let source = m.encoding_source.map_or("generado", |s| s.name());
    kv(
        &mut out,
        "Encoding",
        &format!("{} · {}", loaded.encoding.name(), escape(source)),
    );
    let mode = match r.quirks {
        QuirksMode::NoQuirks => "no-quirks (estándar)",
        QuirksMode::Quirks => "quirks",
        QuirksMode::LimitedQuirks => "limited-quirks",
    };
    kv(&mut out, "Modo", mode);
    kv(
        &mut out,
        "HTML",
        &format!("{:.1} KB", m.bytes as f64 / 1024.0),
    );
    kv(
        &mut out,
        "DOM",
        &format!("{} nodos · {} elementos", r.dom_nodes, r.elements),
    );
    kv(
        &mut out,
        "Estilos",
        &format!(
            "{} hojas · {} reglas · {} declaraciones",
            r.style.sheets, r.style.rules, r.style.declarations
        ),
    );
    let failed = page.images.errors.len();
    let images = if failed > 0 {
        format!(
            "{} cargadas · <span class=\"bad\">{failed} fallidas</span>",
            page.images.by_url.len()
        )
    } else {
        format!("{} cargadas", page.images.by_url.len())
    };
    kv(&mut out, "Imágenes", &images);
    kv(&mut out, "Cookies", &loaded.cookies().to_string());
    out.push_str("</div>");

    // Soporte
    out.push_str("<div class=\"section\"><h3>Soporte</h3>");
    let warn = |n: usize, s: &str| {
        if n > 0 {
            format!("<span class=\"warn\">{n}</span> · {s}")
        } else {
            format!("0 · {s}")
        }
    };
    kv(&mut out, "<script>", &warn(r.scripts, "sin motor JS aún"));
    let unknown: usize = r.style.unknown_properties.values().sum();
    kv(
        &mut out,
        "Propiedades CSS",
        &warn(
            unknown,
            &format!("{} desconocidas", r.style.unknown_properties.len()),
        ),
    );
    if !r.style.unknown_properties.is_empty() {
        let mut list: Vec<_> = r.style.unknown_properties.iter().collect();
        list.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        out.push_str(&chips(list.iter().take(24).map(|(k, v)| (k.as_str(), **v))));
    }
    let values: usize = r.style.unsupported_values.values().sum();
    kv(
        &mut out,
        "Valores CSS",
        &warn(values, "no aceptados (gradientes, funciones nuevas…)"),
    );
    if !r.style.unsupported_values.is_empty() {
        let mut list: Vec<_> = r.style.unsupported_values.iter().collect();
        list.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        out.push_str(&chips(list.iter().take(16).map(|(k, v)| (k.as_str(), **v))));
    }
    let at: usize = r.style.unsupported_at_rules.values().sum();
    kv(&mut out, "At-rules", &warn(at, "sin efecto"));
    if at > 0 {
        out.push_str(&chips(
            r.style
                .unsupported_at_rules
                .iter()
                .map(|(k, v)| (k.as_str(), *v)),
        ));
    }
    let degraded: usize = r.layout.degraded_display.values().sum();
    kv(
        &mut out,
        "display",
        &warn(degraded, "degradados (tablas y grid → bloque)"),
    );
    if degraded > 0 {
        out.push_str(&chips(
            r.layout
                .degraded_display
                .iter()
                .map(|(k, v)| (k.as_str(), *v)),
        ));
    }
    let unsupported: usize = r.images.unsupported.values().sum();
    kv(
        &mut out,
        "Imágenes",
        &warn(unsupported, "en formatos sin decodificador"),
    );
    if unsupported > 0 {
        out.push_str(&chips(r.images.unsupported.iter().map(|(k, v)| (*k, *v))));
    }
    kv(
        &mut out,
        "Formularios",
        &format!(
            "{} controles · sin interactividad aún",
            r.layout.form_controls
        ),
    );
    out.push_str("</div>");

    // Fuentes
    out.push_str("<div class=\"section\"><h3>Fuentes</h3>");
    if r.fonts.is_empty() {
        out.push_str("<p class=\"muted\">Sin texto.</p>");
    }
    for (requested, resolved) in &r.fonts {
        kv(&mut out, requested, &escape(resolved));
    }
    out.push_str("</div>");

    // E4: privacidad (RFC 6265bis §7.1: el navegador decide sobre las cookies de terceros).
    if actions {
        let blocked = loaded.net().fetcher.cookies.block_third_party();
        out.push_str("<div class=\"section\"><h3>Privacidad</h3>");
        kv(
            &mut out,
            "Cookies de terceros",
            if blocked { "bloqueadas" } else { "permitidas" },
        );
        out.push_str(&format!(
            "<div class=\"actions\"><a class=\"action\" href=\"bf:toggle-third-party-cookies\">{}</a></div></div>",
            if blocked {
                "Permitir cookies de terceros"
            } else {
                "Bloquear cookies de terceros"
            }
        ));
    }

    if actions {
        out.push_str(
            "<div class=\"section\"><h3>Acciones</h3><div class=\"actions\">\
             <a class=\"action\" href=\"bf:save-png\">Guardar PNG</a>\
             <a class=\"action\" href=\"bf:dump-layout\">Volcar layout</a>\
             <a class=\"action\" href=\"bf:dump-dom\">Volcar DOM</a>\
             <a class=\"action\" href=\"bf:engine-page\">Abrir como página</a></div></div>",
        );
    }
    out
}

/// Markdown en línea mínimo de `wpt-status.md`: `código` y escape.
fn inline_md(s: &str) -> String {
    let mut out = String::new();
    for (i, part) in s.split('`').enumerate() {
        if i % 2 == 1 {
            let _ = write!(out, "<code>{}</code>", escape(part));
        } else {
            out.push_str(&escape(part));
        }
    }
    out
}

/// `about:wpt`: `docs/wpt-status.md` como tabla con barras.
fn wpt(theme: &Theme) -> String {
    let mut date = String::new();
    let mut commit = String::new();
    let mut rows = String::from(
        "<div class=\"tr head\"><span class=\"name\">Directorio</span><span class=\"n\">pass</span>\
         <span class=\"n\">fail</span><span class=\"n\">needs-js</span><span class=\"pct\">% de los ejecutables</span></div>",
    );
    let (mut total, mut total_detail) = (String::new(), String::new());
    let mut notes = String::new();
    let mut in_list = false;
    for line in WPT_STATUS.lines() {
        if let Some(v) = line.strip_prefix("- Fecha: ") {
            date = escape(v);
        } else if let Some(v) = line.strip_prefix("- Commit de WPT: ") {
            commit = escape(v.trim_matches('`').get(..12).unwrap_or(v));
        } else if line.starts_with("| css/") || line.starts_with("| **Total") {
            let cells: Vec<&str> = line
                .trim_matches('|')
                .split('|')
                .map(|c| c.trim().trim_matches('*'))
                .collect();
            let [name, pass, fail, js, err, all, pct] = cells[..] else {
                continue;
            };
            let value: f32 = pct.trim_end_matches('%').parse().unwrap_or(0.0);
            let is_total = name == "Total";
            if is_total {
                total = escape(pct);
                total_detail = format!(
                    "{pass} de {} ejecutables · {all} tests en total · {js} necesitan JavaScript",
                    pass.parse::<u32>().unwrap_or(0)
                        + fail.parse::<u32>().unwrap_or(0)
                        + err.parse::<u32>().unwrap_or(0)
                );
            }
            let _ = write!(
                rows,
                "<div class=\"tr{}\"><span class=\"name\">{}</span><span class=\"n\">{pass}</span>\
                 <span class=\"n\">{fail}</span><span class=\"n\">{js}</span><span class=\"pct\"><span class=\"p\">{pct}</span>\
                 <span class=\"track\"><span class=\"fill\" style=\"width:{value:.1}%\"></span></span></span></div>",
                if is_total { " total" } else { "" },
                escape(name)
            );
        } else if let Some(h) = line.strip_prefix("## ") {
            if in_list {
                notes.push_str("</li></ul>");
                in_list = false;
            }
            let _ = write!(notes, "<h2>{}</h2>", escape(h));
        } else if let Some(item) = line.strip_prefix("- ") {
            if !notes.is_empty() {
                notes.push_str(if in_list {
                    "</li><li>"
                } else {
                    "<ul class=\"notes\"><li>"
                });
                in_list = true;
                notes.push_str(&inline_md(item));
            }
        } else if in_list && line.starts_with("  ") {
            notes.push(' ');
            notes.push_str(&inline_md(line.trim()));
        }
    }
    if in_list {
        notes.push_str("</li></ul>");
    }
    themed(
        WPT,
        theme,
        &[
            ("date", date),
            ("commit", commit),
            ("total", total),
            ("total_detail", total_detail),
            ("rows", rows),
            ("notes", notes),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_fill_every_placeholder() {
        let ctx = LoadContext::cli();
        for name in [
            "",
            "home",
            "wpt",
            "error?dns",
            "engine",
            "nope",
            "history",
            "bookmarks",
            "bookmarks?edit=1",
        ] {
            let url = Url::parse(&format!("about:{name}"), None).unwrap();
            let loaded = page(&url, &ctx);
            let html = bf_dom::test_serialize(&loaded.doc, loaded.doc.root());
            assert!(
                !html.contains("{{"),
                "about:{name} dejó un marcador sin reemplazar"
            );
            assert!(loaded.meta.internal);
        }
    }

    /// Las páginas internas sirven de test del motor: solo usan CSS que se soporta.
    #[test]
    fn internal_pages_use_only_supported_css() {
        let ctx = LoadContext::cli();
        let media = bf_css::media::MediaContext::default();
        for name in [
            "",
            "home",
            "wpt",
            "error?tls",
            "engine",
            "history",
            "bookmarks",
            "settings",
        ] {
            let url = Url::parse(&format!("about:{name}"), None).unwrap();
            let loaded = page(&url, &ctx);
            let sheets = bf_style::document_stylesheets(&loaded.doc, &media, None);
            let report = bf_style::style_report(&loaded.doc, &sheets);
            assert_eq!(report.ignored(), 0, "about:{name}: {report:?}");
        }
        for template in [BAR, STATUS, PANEL, SUGGEST, FIND] {
            let html = fill(
                template,
                &[
                    ("theme", Theme::DARK.css_vars()),
                    ("style", STYLE.to_owned()),
                ],
            );
            let (doc, _) = bf_html::parse_document(&html, &bf_html::ParseOptions::default());
            let sheets = bf_style::document_stylesheets(&doc, &media, None);
            assert_eq!(bf_style::style_report(&doc, &sheets).ignored(), 0);
        }
    }

    #[test]
    fn wpt_page_has_every_directory() {
        let html = wpt(&Theme::DARK);
        assert!(html.contains("css/css-flexbox"));
        assert!(html.contains("tr total"));
    }
}
