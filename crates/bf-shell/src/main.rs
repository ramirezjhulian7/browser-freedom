//! Shell del navegador: ventana, entrada de usuario y orquestación de los subsistemas del motor.

#![forbid(unsafe_code)]

use std::process::ExitCode;

mod internal;
mod load;
mod theme;
mod window;

use bf_css::media::MediaContext;
use bf_css::selectors::{MatchContext, NamespaceMap, matches, parse_selector_list};
use bf_dom::Document;
use bf_style::values::color::Rgba;

const USAGE: &str = "uso: browser-freedom [dump-dom <url|archivo>
                       | dump-css <archivo.css>
                       | dump-style <url|archivo> [--selector <css>]
                       | dump-layout <url|archivo> [--width <px>] [--height <px>] [--system-fonts]
                       | render <url|archivo> -o <salida.png> [--width <px>] [--height <px>] [--scale <n>]
                       | open [<url|archivo>]
                       | session <url|archivo> <paso>... -o <prefijo>]
(<url> puede ser http(s)://, file://, data: o about:; open sin argumento abre about:home)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [] => {
            println!("browser-freedom {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [cmd, path] if cmd == "dump-dom" => dump_dom(path),
        [cmd, path] if cmd == "dump-css" => dump_css(path),
        [cmd, path, rest @ ..] if cmd == "dump-style" => dump_style(path, rest),
        [cmd, path, rest @ ..] if cmd == "dump-layout" => dump_layout(path, rest),
        [cmd, path, rest @ ..] if cmd == "render" => render(path, rest),
        [cmd] if cmd == "open" => window::open(None),
        [cmd, path] if cmd == "open" => window::open(Some(path)),
        [cmd, path, rest @ ..] if cmd == "session" => session(path, rest),
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

/// Rasteriza el documento a PNG con las fuentes del sistema, `@font-face` y la red. Imprime en
/// stderr el tiempo de cada fase.
fn render(arg: &str, args: &[String]) -> Result<(), String> {
    let out = option(args, "-o")?.ok_or("falta -o <salida.png>")?;
    let viewport = bf_layout::Size {
        width: px_option(args, "--width", 800.0)?,
        height: px_option(args, "--height", 600.0)?,
    };
    let scale = px_option(args, "--scale", 1.0)?;
    let loaded = load::load(arg)?;
    let start = std::time::Instant::now();
    let fonts = bf_font::FontDb::system();
    let font_scan = start.elapsed();
    let ctx = bf_paint::PageContext {
        base: &loaded.base,
        encoding: loaded.encoding,
        loader: loaded.loader.clone(),
        fonts,
        extra: &[],
    };
    let page = bf_paint::prepare_page(&loaded.doc, &ctx, viewport);
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
