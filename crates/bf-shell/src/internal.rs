//! Páginas internas (`about:`), generadas como HTML y maquetadas por el mismo motor que las
//! páginas de la web: `about:home`, `about:error`, `about:engine`, `about:wpt` y `about:` (el
//! índice). También genera el reporte del motor que muestra el panel lateral de la ventana.
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
pub const BAR: &str = include_str!("../pages/bar.html");
pub const MENU: &str = include_str!("../pages/menu.html");
pub const SOURCE: &str = include_str!("../pages/source.html");
pub const STATUS: &str = include_str!("../pages/status.html");
/// El estado de compatibilidad al compilar (lo regenera la corrida de reftests).
const WPT_STATUS: &str = include_str!("../../../docs/wpt-status.md");

/// Los atajos de la ventana (se muestran en `about:home`).
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl+L", "Ir a la barra de dirección"),
    ("Ctrl+I", "Panel del motor"),
    ("F12", "Superposición de cajas"),
    ("Ctrl+R · F5", "Recargar"),
    ("Esc", "Detener la carga o volver a la página"),
    ("Alt+← · Alt+→", "Atrás y adelante"),
    ("Ctrl++ · Ctrl+− · Ctrl+0", "Zoom"),
    ("Ctrl+S", "Guardar un PNG de la ventana"),
    ("Ctrl+Shift+D", "Volcar el layout"),
    ("Ctrl+H", "Inicio"),
    ("Ctrl+T · Ctrl+W", "Pestaña nueva y cerrar pestaña"),
    ("Ctrl+Tab · Ctrl+1…9", "Cambiar de pestaña"),
    (
        "Clic central · Ctrl+clic",
        "Abrir un enlace en otra pestaña",
    ),
    ("Espacio · Mayús+Espacio", "Página abajo y arriba"),
];

/// Accesos de `about:home`.
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

/// Reemplaza cada `{{clave}}` de `template` por su valor.
pub fn fill(template: &str, vars: &[(&str, String)]) -> String {
    let mut out = template.to_owned();
    for (key, value) in vars {
        out = out.replace(&format!("{{{{{key}}}}}"), value);
    }
    out
}

/// Una página interna con el tema y el estilo común.
fn themed(template: &str, theme: &Theme, vars: &[(&str, String)]) -> String {
    let mut all = vec![("theme", theme.css_vars()), ("style", STYLE.to_owned())];
    all.extend(vars.iter().cloned());
    fill(template, &all)
}

/// La página interna para `url` (`about:...`).
pub fn page(url: &Url, ctx: &LoadContext) -> Loaded {
    let name = url.path_string().to_ascii_lowercase();
    let html = match name.as_str() {
        "blank" => String::new(),
        "" => themed(ABOUT, &ctx.theme, &[]),
        "home" | "newtab" => home(ctx),
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
    let mut tiles = String::new();
    for (name, href, host) in TILES {
        let letter = name.chars().next().unwrap_or('?').to_uppercase();
        let _ = write!(
            tiles,
            "<a class=\"tile\" href=\"{href}\"><span class=\"frame\">{letter}<span class=\"gap\">\
             </span></span><span class=\"name\">{name}</span><span class=\"host\">{host}</span></a>"
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
    let mut keys = String::new();
    for (k, d) in SHORTCUTS {
        // Cada combinación en su tecla: "Ctrl+R · F5" son dos.
        let chips: String = k
            .split(" · ")
            .map(|c| format!("<span class=\"kbd\">{}</span>", escape(c)))
            .collect();
        let _ = write!(
            keys,
            "<div class=\"key\"><span class=\"k\">{chips}</span><span class=\"d\">{}</span></div>",
            escape(d)
        );
    }
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
            ("shortcuts", keys),
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
            "No se encontró el servidor".into(),
            "DNS".into(),
            "El nombre del host no existe o no se pudo resolver. Revisa la dirección o la conexión.",
        ),
        ErrorKind::Tls => (
            "La conexión segura falló".into(),
            "TLS".into(),
            "El certificado del servidor no es válido o el handshake TLS no se completó.",
        ),
        ErrorKind::Timeout => (
            "El servidor tardó demasiado".into(),
            "Timeout".into(),
            "Se agotó el tiempo de conexión (10 s) o de lectura (30 s).",
        ),
        ErrorKind::Connection => (
            "No se pudo conectar".into(),
            "Conexión".into(),
            "El servidor rechazó la conexión o la cortó antes de responder.",
        ),
        ErrorKind::Http(code) => (
            format!("El servidor respondió {code}"),
            format!("HTTP {code}"),
            if code < 500 {
                "La página no existe o no hay permiso para verla."
            } else {
                "El servidor tuvo un error al procesar la petición."
            },
        ),
        ErrorKind::NotFound => (
            "No se encontró el archivo".into(),
            "Archivo".into(),
            "La ruta no existe o no se puede leer.",
        ),
        ErrorKind::InvalidUrl => (
            "La dirección no es válida".into(),
            "URL".into(),
            "No es una URL que el URL Standard acepte.",
        ),
        ErrorKind::UnsupportedScheme => (
            "Esquema no soportado".into(),
            "Esquema".into(),
            "browser-freedom carga http, https, file, data y about.",
        ),
        ErrorKind::TooManyRedirects => (
            "Demasiadas redirecciones".into(),
            "Redirecciones".into(),
            "La página redirige más de 20 veces (Fetch §4.4).",
        ),
        ErrorKind::Engine => (
            "El motor falló con esta página".into(),
            "Motor".into(),
            "Es un error de browser-freedom, no de la página: el detalle sirve para reportarlo.",
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
        let body = "<div class=\"section\"><p>Abre <code>about:engine</code> desde una página (o pulsa \
                    <b>Ctrl+I</b>) para ver su reporte, o escribe <code>about:engine?https://…</code>.</p></div>"
            .to_owned();
        return themed(
            ENGINE,
            &ctx.theme,
            &[("subject", "ninguna página".into()), ("body", body)],
        );
    };
    let fetched = if target.scheme == "about" {
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
        for name in ["", "home", "wpt", "error?dns", "engine", "nope"] {
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
        for name in ["", "home", "wpt", "error?tls", "engine"] {
            let url = Url::parse(&format!("about:{name}"), None).unwrap();
            let loaded = page(&url, &ctx);
            let sheets = bf_style::document_stylesheets(&loaded.doc, &media, None);
            let report = bf_style::style_report(&loaded.doc, &sheets);
            assert_eq!(report.ignored(), 0, "about:{name}: {report:?}");
        }
        for template in [BAR, STATUS, PANEL] {
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
