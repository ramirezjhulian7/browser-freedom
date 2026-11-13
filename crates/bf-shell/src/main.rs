//! Freedom, el navegador: ventana, entrada de usuario y orquestación de los subsistemas del
//! motor. El ejecutable se llama `freedom`.

#![forbid(unsafe_code)]
// Con `--features gui` (los instaladores), Windows abre Freedom sin la consola detrás.
#![cfg_attr(all(windows, feature = "gui"), windows_subsystem = "windows")]

use std::process::ExitCode;

mod bookmarks;
mod downloads;
mod find;
mod history;
mod internal;
mod load;
mod preload;
mod profile;
mod reftest_js;
mod restore;
mod script;
mod settings;
mod theme;
mod window;
mod xml_scripts;

use bf_css::media::MediaContext;
use bf_css::selectors::{MatchContext, NamespaceMap, matches, parse_selector_list};
use bf_dom::Document;
use bf_style::values::color::Rgba;

const USAGE: &str = "uso: freedom [<url|archivo>
                       | --version
                       | dump-dom <url|archivo>
                       | dump-css <archivo.css>
                       | dump-style <url|archivo> [--selector <css>]
                       | dump-layout <url|archivo> [--width <px>] [--height <px>] [--system-fonts]
                       | render <url|archivo> -o <salida.png> [--width <px>] [--height <px>] [--scale <n>] [--wait <ms>]
                       | open [<url|archivo>]
                       | session <url|archivo> <paso>... -o <prefijo>
                       | wpt-reftests-js [--wpt <dir>] [--out <dir>]]
(<url> puede ser http(s)://, file://, data: o about:; sin argumentos, o open sin argumento,
abre la página de inicio)";

/// Las órdenes de línea de comandos: un único argumento que no es una de ellas se abre.
const COMMANDS: &[&str] = &[
    "dump-dom",
    "dump-css",
    "dump-style",
    "dump-layout",
    "render",
    "open",
    "session",
    "wpt-reftests-js",
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        // Doble clic en el ejecutable: el navegador.
        [] => window::open(None),
        [v] if v == "--version" || v == "-V" => {
            println!("Freedom {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [cmd, _, ..]
            if matches!(
                cmd.as_str(),
                "dump-dom" | "dump-css" | "dump-style" | "dump-layout" | "render"
            ) =>
        {
            on_script_stack(args.clone())
        }
        [cmd] if cmd == "open" => window::open(None),
        [cmd, path] if cmd == "open" => window::open(Some(path)),
        [cmd, path, rest @ ..] if cmd == "session" => session(path, rest),
        // Reftests de WPT con JavaScript (`REFTEST_GROUPS`, `REFTEST_FILTER`).
        [cmd, rest @ ..] if cmd == "wpt-reftests-js" => reftest_js::command(rest),
        // «Abrir con…» o un archivo arrastrado sobre el ejecutable.
        [path] if !path.starts_with('-') && !COMMANDS.contains(&path.as_str()) => {
            window::open(Some(path))
        }
        _ => Err(USAGE.to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Los comandos sin ventana corren en un hilo con la pila del motor de JavaScript (los
/// scripts de la página se ejecutan al cargarla).
fn on_script_stack(args: Vec<String>) -> Result<(), String> {
    let run = move || match args.as_slice() {
        [cmd, path] if cmd == "dump-dom" => dump_dom(path),
        [cmd, path] if cmd == "dump-css" => dump_css(path),
        [cmd, path, rest @ ..] if cmd == "dump-style" => dump_style(path, rest),
        [cmd, path, rest @ ..] if cmd == "dump-layout" => dump_layout(path, rest),
        [cmd, path, rest @ ..] if cmd == "render" => render(path, rest),
        _ => Err(USAGE.to_owned()),
    };
    std::thread::Builder::new()
        .name("freedom-cli".into())
        .stack_size(script::THREAD_STACK)
        .spawn(run)
        .map_err(|e| format!("no se pudo crear el hilo: {e}"))?
        .join()
        .unwrap_or_else(|_| Err("error interno del motor".to_owned()))
}

fn read(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("no se pudo leer {path}: {e}"))
}

/// Valor de `--name <valor>` (o `-o <valor>`) en `args`.
fn option<'a>(args: &'a [String], name: &str) -> Result<Option<&'a str>, String> {
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        if a == name {
            return iter
                .next()
                .map(|v| Some(v.as_str()))
                .ok_or_else(|| format!("falta el valor de {name}"));
        }
        if !a.starts_with('-') {
            return Err(format!("argumento inesperado: {a}"));
        }
        // El valor de otra opción.
        iter.next();
    }
    Ok(None)
}

/// Sesión sin ventana: los pasos van antes (o después) de `-o <prefijo>`.
fn session(arg: &str, args: &[String]) -> Result<(), String> {
    let at = args
        .iter()
        .position(|a| a == "-o")
        .ok_or("falta -o <prefijo>")?;
    let out = args.get(at + 1).ok_or("falta el valor de -o")?;
    let steps: Vec<String> = args[..at].iter().chain(&args[at + 2..]).cloned().collect();
    window::session(arg, &steps, out)
}

fn parse_html(arg: &str) -> Result<Document, String> {
    Ok(load::load(arg)?.doc)
}

/// Imprime el DOM en el formato de las pruebas de tree construction.
fn dump_dom(path: &str) -> Result<(), String> {
    let doc = parse_html(path)?;
    println!("{}", bf_dom::test_serialize(&doc, doc.root()));
    Ok(())
}

/// Imprime el árbol de reglas de una hoja de estilos de autor.
fn dump_css(path: &str) -> Result<(), String> {
    let text = bf_style::loader::decode_css(&read(path)?, bf_infra::encoding::Encoding::Utf8);
    let sheet = bf_css::stylesheet::Stylesheet::parse(&text, bf_css::stylesheet::Origin::Author);
    print!("{}", sheet.dump());
    Ok(())
}

fn element_label(doc: &Document, node: bf_dom::NodeId) -> String {
    let el = doc.element(node).expect("elemento");
    let mut s = format!("<{}", el.name);
    if let Some(id) = el.attr("id") {
        s.push('#');
        s.push_str(id);
    }
    if let Some(class) = el.attr("class") {
        for c in class.split_ascii_whitespace() {
            s.push('.');
            s.push_str(c);
        }
    }
    s + ">"
}

/// Imprime las propiedades computadas: todas para los elementos que coinciden con
/// `--selector`, o solo las que difieren del valor inicial si no hay selector.
fn dump_style(path: &str, args: &[String]) -> Result<(), String> {
    let doc = parse_html(path)?;
    let selector = option(args, "--selector")?;
    let list = selector
        .map(|s| {
            parse_selector_list(s, &NamespaceMap::default())
                .map_err(|_| format!("selector inválido: {s}"))
        })
        .transpose()?;
    let ctx = MatchContext::for_document(&doc);
    let styles = bf_style::style_document(&doc, &[], &MediaContext::default(), &ctx);
    let initial = bf_style::ComputedStyle::initial();
    for node in doc.inclusive_descendants(doc.root()) {
        let Some(style) = styles.get(&(node, None)) else {
            continue;
        };
        if let Some(list) = &list
            && !list.0.iter().any(|s| matches(s, &doc, node, &ctx))
        {
            continue;
        }
        println!("{}", element_label(&doc, node));
        for def in bf_style::properties::PROPERTIES {
            let value = (def.get)(style);
            if list.is_none() && value == (def.get)(initial) {
                continue;
            }
            println!("  {}: {}", def.name, (def.to_css)(&value));
        }
        let mut custom: Vec<_> = style.custom.keys().collect();
        custom.sort();
        for name in custom {
            println!("  {name}: {}", style.get_css(name).unwrap_or_default());
        }
    }
    Ok(())
}

/// Imprime el árbol de fragmentos (con `TestFont` mientras no haya fuentes reales).
fn dump_layout(path: &str, args: &[String]) -> Result<(), String> {
    let doc = parse_html(path)?;
    let px = |name: &str, default: f32| -> Result<f32, String> {
        option(args, name)?.map_or(Ok(default), |v| {
            v.parse()
                .map_err(|_| format!("valor inválido para {name}: {v}"))
        })
    };
    let width = px("--width", 800.0)?;
    let height = px("--height", 600.0)?;
    let media = MediaContext {
        width: f64::from(width),
        height: f64::from(height),
        ..MediaContext::default()
    };
    let ctx = MatchContext::for_document(&doc);
    let viewport = bf_layout::Size { width, height };
    // `--system-fonts`: métricas de las fuentes del sistema en lugar de `TestFont`.
    let tree = if args.iter().any(|a| a == "--system-fonts") {
        let fonts = bf_font::Fonts::new(bf_font::FontDb::system());
        let metrics = bf_layout::StyleMetrics(&fonts);
        let sheets = bf_style::document_stylesheets(&doc, &media, None);
        let styles =
            bf_style::style_document_with_sheets(&doc, &sheets, &[], &media, &ctx, Some(&metrics));
        bf_layout::layout_document(&doc, &styles, viewport, &fonts)
    } else {
        let styles = bf_style::style_document(&doc, &[], &media, &ctx);
        bf_layout::layout_document(&doc, &styles, viewport, &bf_layout::TestFont)
    };
    print!("{}", bf_layout::dump(&doc, &tree));
    Ok(())
}

fn px_option(args: &[String], name: &str, default: f32) -> Result<f32, String> {
    option(args, name)?.map_or(Ok(default), |v| {
        v.parse()
            .ok()
            .filter(|n: &f32| n.is_finite() && *n > 0.0)
            .ok_or_else(|| format!("valor inválido para {name}: {v}"))
    })
}

fn ms(d: std::time::Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}

/// E4: declara `url` como el documento de nivel superior de este hilo para las cookies
/// `SameSite` (RFC 6265bis §5.2, "site for cookies").
fn set_cookie_site(url: &bf_infra::Url) {
    bf_net::cookies::set_thread_context(Some(bf_net::cookies::CookieContext {
        site_for_cookies: bf_net::cookies::site_of(url),
        top_level_navigation: false,
    }));
}

/// E4: como [`load::load`], pero conserva el motor: después de cargar corre el bucle de
/// eventos (temporizadores, tareas, red) hasta que no queda nada pendiente o pasa `wait`, y
/// devuelve el documento de ese momento con los de sus marcos.
///
/// Los scripts ven `viewport` como `innerWidth`/`innerHeight` y `scale` como
/// `devicePixelRatio` (CSSOM View §4: px de dispositivo por px CSS), lo mismo que la
/// ventana con esa escala de pantalla.
fn load_live(
    arg: &str,
    wait: std::time::Duration,
    viewport: bf_layout::Size,
    scale: f32,
) -> Result<(load::Loaded, Vec<bf_paint::FrameDoc>), String> {
    let url = bf_infra::url_from_arg(arg).ok_or_else(|| format!("URL o ruta inválida: {arg}"))?;
    if url.scheme == "about" {
        return load::load(arg).map(|l| (l, Vec::new()));
    }
    set_cookie_site(&url);
    let mut engine = script::Engine::stderr();
    bf_web::cssom::set_viewport(
        &mut engine.rt.agent,
        engine.realm,
        viewport.width,
        viewport.height,
        scale,
    );
    // E6: la caché HTTP, HSTS y las cookies persistentes en disco, como `load::load`.
    let net = std::sync::Arc::new(bf_net::NetLoader::with_disk_cache());
    let mut env = script::PageEnv::cli(net.clone());
    env.screen.device_pixel_ratio = f64::from(scale);
    let mut loaded = load::fetch_document_with(&url, net, None, Some((&mut engine, &env)))
        .map_err(|e| format!("no se pudo cargar {url}: {e}"))?;
    set_cookie_site(&loaded.url);
    let start = std::time::Instant::now();
    // E4: los cuadros de `requestAnimationFrame` (HTML §8.1.7.3), uno cada 16 ms.
    let frame = std::time::Duration::from_millis(16);
    let mut last_frame = start;
    loop {
        engine.run_due_tasks();
        if engine.wants_frame() && last_frame.elapsed() >= frame {
            last_frame = std::time::Instant::now();
            engine.run_animation_frames();
        }
        let to_frame = engine
            .wants_frame()
            .then(|| frame.saturating_sub(last_frame.elapsed()));
        let next = match (engine.next_wake(), to_frame) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let Some(next) = next else {
            break;
        };
        if start.elapsed() + next > wait {
            break;
        }
        std::thread::sleep(next);
    }
    let (executed, failed) = engine.counts();
    if executed > 0 {
        eprintln!("scripts: {executed} ejecutados, {failed} con error");
    }
    engine.sync_frames();
    if let Some(doc) = engine.render_copy() {
        loaded.doc = doc;
    }
    let frames = engine.frame_docs(&std::collections::HashMap::new());
    Ok((loaded, frames))
}

/// Rasteriza el documento a PNG con las fuentes del sistema, `@font-face` y la red. Imprime en
/// stderr el tiempo de cada fase. Antes, los scripts corren hasta `--wait` ms (5000 por
/// defecto; 0: solo el parseo).
fn render(arg: &str, args: &[String]) -> Result<(), String> {
    let out = option(args, "-o")?.ok_or("falta -o <salida.png>")?;
    let viewport = bf_layout::Size {
        width: px_option(args, "--width", 800.0)?,
        height: px_option(args, "--height", 600.0)?,
    };
    let scale = px_option(args, "--scale", 1.0)?;
    let wait = match option(args, "--wait")? {
        Some(v) => v
            .parse::<u64>()
            .map_err(|_| format!("valor inválido para --wait: {v}"))?,
        None => 5000,
    };
    let wait = std::time::Duration::from_millis(wait);
    let (loaded, frames) = load_live(arg, wait, viewport, scale)?;
    let start = std::time::Instant::now();
    let fonts = bf_font::FontDb::system();
    let font_scan = start.elapsed();
    let ctx = bf_paint::PageContext {
        base: &loaded.base,
        encoding: loaded.encoding,
        loader: loaded.loader.clone(),
        fonts,
        extra: &[],
        text_zoom: 1.0,
        // E2: `--scale` es la densidad de la salida: elige la fuente de `srcset`.
        device_pixel_ratio: scale,
    };
    let page = bf_paint::prepare_page_with_frames(&loaded.doc, &ctx, viewport, &[], &frames);
    let start = std::time::Instant::now();
    let pixmap = bf_paint::render(&page.display_list, viewport, scale, Rgba::WHITE);
    let raster = start.elapsed();
    let t = page.timings;
    eprintln!("url:     {} ({})", loaded.url, loaded.encoding.name());
    eprintln!("fetch:   {}", ms(loaded.fetch));
    eprintln!("parse:   {}", ms(loaded.parse));
    eprintln!("fuentes: {} (escaneo del sistema)", ms(font_scan));
    eprintln!("style:   {} (incluye hojas externas)", ms(t.style));
    eprintln!(
        "imágenes: {} ({} cargadas, {} con error)",
        ms(t.images),
        page.images.by_url.len(),
        page.images.errors.len()
    );
    eprintln!("layout:  {}", ms(t.layout));
    eprintln!("paint:   {} + raster {}", ms(t.paint), ms(raster));
    for (url, err) in loaded
        .net()
        .errors
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
    {
        eprintln!("aviso: {url}: {err}");
    }
    for (url, err) in &page.images.errors {
        eprintln!("aviso: imagen {url}: {err}");
    }
    pixmap
        .save_png(out)
        .map_err(|e| format!("no se pudo escribir {out}: {e}"))
}
