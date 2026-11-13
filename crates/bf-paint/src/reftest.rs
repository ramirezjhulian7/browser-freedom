//! Harness de reftests de web-platform-tests.
//!
//! Un reftest es un archivo con `<link rel="match" href>` o `<link rel="mismatch" href>`: se
//! pinta el test y cada referencia con la misma tubería y se comparan píxel a píxel (con la
//! tolerancia de `<meta name="fuzzy">`). Pasa si coincide con todas las `match` y difiere de
//! todas las `mismatch`. Los archivos sin referencias son tests de testharness.js o manuales,
//! y los que tienen `<script>` o atributos manejadores de eventos `on*` dependen de JS: ambos
//! cuentan como `needs-js`. Un pánico o una referencia que no existe cuentan como `error`.
//!
//! Condiciones: viewport 800×600, DPR 1, fondo blanco, `/` resuelto contra la raíz de WPT
//! (para `/fonts/ahem.css` y `/css/support/*`), fuentes del sistema más las de `@font-face`.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use bf_dom::{Document, NodeData};
use bf_font::FontDb;
use bf_infra::encoding::Encoding;
use bf_infra::{FileLoader, ResourceLoader, Url, decode_text};
use bf_layout::Size;
use tiny_skia::Pixmap;

/// Origen con el que el harness sirve `tests/wpt` (como `wpt serve`).
pub fn wpt_origin() -> Url {
    Url::parse("http://web-platform.test/", None).expect("URL válida")
}

pub const VIEWPORT: Size = Size {
    width: 800.0,
    height: 600.0,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Pass,
    Fail,
    NeedsJs,
    Error,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Fail => "fail",
            Status::NeedsJs => "needs-js",
            Status::Error => "error",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Outcome {
    /// Ruta relativa a la raíz de WPT, con `/`.
    pub test: String,
    /// Directorio del informe al que pertenece.
    pub group: String,
    pub status: Status,
    /// Diferencia máxima por canal y píxeles distintos de la peor comparación, o el motivo
    /// del error.
    pub detail: String,
}

/// Tolerancia de `<meta name="fuzzy">`: rangos de diferencia máxima por canal y de cantidad de
/// píxeles distintos. Un número solo `N` se toma como `0-N`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fuzzy {
    pub max_difference: (u32, u32),
    pub total_pixels: (u32, u32),
}

fn parse_range(s: &str) -> Option<(u32, u32)> {
    let s = s.trim();
    match s.split_once('-') {
        Some((a, b)) => Some((a.trim().parse().ok()?, b.trim().parse().ok()?)),
        None => Some((0, s.parse().ok()?)),
    }
}

/// `content` de `<meta name=fuzzy>`: `[url:]maxDifference=a-b;totalPixels=c-d` (los nombres
/// son opcionales).
pub fn parse_fuzzy(content: &str) -> Option<(Option<String>, Fuzzy)> {
    let (reference, rest) = match content.split_once(':') {
        Some((url, rest)) if !url.contains('=') && !url.contains(';') => {
            (Some(url.trim().to_owned()), rest)
        }
        _ => (None, content),
    };
    let parts: Vec<&str> = rest
        .split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let [a, b] = parts.as_slice() else {
        return None;
    };
    let value = |p: &str, name: &str| -> Option<(u32, u32)> {
        match p.split_once('=') {
            Some((k, v)) if k.trim().eq_ignore_ascii_case(name) => parse_range(v),
            Some(_) => None,
            None => parse_range(p),
        }
    };
    Some((
        reference,
        Fuzzy {
            max_difference: value(a, "maxDifference")?,
            total_pixels: value(b, "totalPixels")?,
        },
    ))
}

/// (diferencia máxima por canal, píxeles distintos).
pub fn compare(a: &Pixmap, b: &Pixmap) -> (u32, u32) {
    let mut max = 0u32;
    let mut count = 0u32;
    for (pa, pb) in a
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.data().as_chunks::<4>().0)
    {
        if pa != pb {
            count += 1;
            for i in 0..4 {
                max = max.max(u32::from(pa[i].abs_diff(pb[i])));
            }
        }
    }
    (max, count)
}

fn matches(diff: (u32, u32), fuzzy: Option<Fuzzy>) -> bool {
    if diff.1 == 0 {
        return true;
    }
    fuzzy.is_some_and(|f| {
        (f.max_difference.0..=f.max_difference.1).contains(&diff.0)
            && (f.total_pixels.0..=f.total_pixels.1).contains(&diff.1)
    })
}

/// Por qué no se pudo pintar una referencia.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefError {
    /// La referencia tiene `<script>` (este harness no corre JavaScript).
    NeedsJs,
    Other(String),
}

/// El veredicto de un reftest ya pintado.
pub struct Verdict {
    pub status: Status,
    /// La peor comparación (`maxdiff=… pixels=…`) o el motivo.
    pub detail: String,
    /// La referencia `match` con la que no coincidió (para guardar las imágenes del fallo).
    pub failed_match: Option<Arc<Pixmap>>,
}

/// Compara `actual` con cada referencia de `m`: pasa si coincide con todas las `match` y
/// difiere de todas las `mismatch`, con la tolerancia de `<meta name=fuzzy>` (la de esa
/// referencia o la general). `reference` pinta la referencia de un `href`; `about:blank` es
/// una página en blanco. `Err` si una referencia no se pudo pintar.
pub fn judge(
    m: &Meta,
    actual: &Pixmap,
    reference: &mut dyn FnMut(&str) -> Result<Arc<Pixmap>, RefError>,
) -> Result<Verdict, String> {
    let mut worst = (0, 0);
    let mut status = Status::Pass;
    let mut failed_match = None;
    let refs = m
        .matches
        .iter()
        .map(|h| (h, true))
        .chain(m.mismatches.iter().map(|h| (h, false)));
    for (href, expect_match) in refs {
        // E7: `about:blank` es una página en blanco.
        let expected = if href.trim().eq_ignore_ascii_case("about:blank") {
            let mut blank = Pixmap::new(actual.width(), actual.height()).expect("tamaño válido");
            blank.fill(tiny_skia::Color::WHITE);
            Arc::new(blank)
        } else {
            match reference(href) {
                Ok(p) => p,
                Err(RefError::NeedsJs) => {
                    return Ok(Verdict {
                        status: Status::NeedsJs,
                        detail: "referencia con <script>".into(),
                        failed_match: None,
                    });
                }
                Err(RefError::Other(e)) => return Err(e),
            }
        };
        let file = href.rsplit('/').next().unwrap_or(href);
        let fuzzy = m
            .fuzzy
            .iter()
            .find(|(r, _)| r.as_deref().is_some_and(|r| r.ends_with(file)))
            .or_else(|| m.fuzzy.iter().find(|(r, _)| r.is_none()))
            .map(|(_, f)| *f);
        let diff = compare(actual, &expected);
        if matches(diff, fuzzy) != expect_match {
            status = Status::Fail;
            if expect_match {
                failed_match = Some(expected.clone());
            }
        }
        if diff.1 >= worst.1 {
            worst = diff;
        }
    }
    Ok(Verdict {
        status,
        detail: format!("maxdiff={} pixels={}", worst.0, worst.1),
        failed_match,
    })
}

/// Imagen de diferencias: rojo donde difieren, el test atenuado donde no.
pub fn diff_image(a: &Pixmap, b: &Pixmap) -> Pixmap {
    let mut out = Pixmap::new(a.width(), a.height()).expect("tamaño válido");
    for ((o, pa), pb) in out
        .data_mut()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(a.data().as_chunks::<4>().0)
        .zip(b.data().as_chunks::<4>().0)
    {
        if pa == pb {
            let sum = u16::from(pa[0]) + u16::from(pa[1]) + u16::from(pa[2]);
            let g = 160 + (sum / 9) as u8;
            *o = [g, g, g, 255];
        } else {
            *o = [255, 0, 0, 255];
        }
    }
    out
}

/// Lo que se necesita saber de un archivo antes de pintarlo.
#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub matches: Vec<String>,
    pub mismatches: Vec<String>,
    pub fuzzy: Vec<(Option<String>, Fuzzy)>,
    pub has_script: bool,
}

impl Meta {
    /// ¿Tiene referencias (`rel=match` o `rel=mismatch`)? Si no, es un test de testharness.js
    /// o manual.
    pub fn is_reftest(&self) -> bool {
        !self.matches.is_empty() || !self.mismatches.is_empty()
    }
}

/// `.xht`, `.xhtml` y `.svg` son XML (`application/xhtml+xml`, `image/svg+xml`); el resto,
/// HTML.
pub fn parse_text(path: &Path, text: &str) -> Document {
    let xml = path.extension().is_some_and(|e| {
        e.eq_ignore_ascii_case("xht") || e.eq_ignore_ascii_case("xhtml") || is_svg_ext(e)
    });
    if xml {
        // E10, cambio mínimo: el content type de un `.xht` (sus `createElement` son XHTML).
        let mut doc = bf_html::parse_xml_document(text);
        doc.content_type = "application/xhtml+xml".into();
        doc
    } else {
        bf_html::parse_document(text, &bf_html::ParseOptions::default()).0
    }
}

fn is_svg_ext(e: &std::ffi::OsStr) -> bool {
    e.eq_ignore_ascii_case("svg")
}

/// Lee y parsea un archivo de WPT (con [`parse_text`]): el documento y el texto.
pub fn parse_file(path: &Path) -> Option<(Document, String)> {
    let bytes = std::fs::read(path).ok()?;
    let text = decode_text(&bytes);
    Some((parse_text(path, &text), text))
}

/// Referencias, tolerancias y si hay script en `doc`.
pub fn meta(doc: &Document) -> Meta {
    let mut m = Meta {
        matches: Vec::new(),
        mismatches: Vec::new(),
        fuzzy: Vec::new(),
        has_script: false,
    };
    for node in doc.inclusive_descendants(doc.root()) {
        let NodeData::Element(el) = &doc.get(node).data else {
            continue;
        };
        // E12: los atributos manejadores de eventos (HTML §8.1.8.1, `onload="…"`) también son
        // script; muchos tests de `css/css-position/sticky` desplazan así sus contenedores.
        let handler = el.attrs.iter().any(|a| {
            a.ns.is_none()
                && a.name
                    .as_lossy_str()
                    .get(..2)
                    .is_some_and(|p| p.eq_ignore_ascii_case("on"))
        });
        if el.name.as_lossy_str() == "script" || handler {
            m.has_script = true;
        }
        if el.is_html("link") {
            let rel = el.attr("rel").unwrap_or("");
            let Some(href) = el.attr("href") else {
                continue;
            };
            for token in rel.split_ascii_whitespace() {
                if token.eq_ignore_ascii_case("match") {
                    m.matches.push(href.to_owned());
                } else if token.eq_ignore_ascii_case("mismatch") {
                    m.mismatches.push(href.to_owned());
                }
            }
        }
        if el.is_html("meta")
            && el
                .attr("name")
                .is_some_and(|n| n.eq_ignore_ascii_case("fuzzy"))
            && let Some(f) = el.attr("content").and_then(parse_fuzzy)
        {
            m.fuzzy.push(f);
        }
    }
    m
}

/// ¿El archivo es un test (y no una referencia, un recurso o un crashtest)?
pub fn is_test_file(path: &Path) -> bool {
    let Some(name) = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
    else {
        return false;
    };
    let ext_ok = [".html", ".htm", ".xht", ".xhtml", ".svg"]
        .iter()
        .any(|e| name.ends_with(e));
    let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
    // `-ref`, `-ref2`, `-ref-x`, `ref-x`, `-notref`, `-noref`.
    let reference = stem
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .ends_with("-ref")
        || stem.contains("-ref-")
        || stem.starts_with("ref-")
        || stem.contains("-notref")
        || stem.contains("-noref");
    let skipped_dir = path.components().any(|c| {
        matches!(
            c.as_os_str().to_string_lossy().as_ref(),
            "support" | "reference" | "resources" | "crashtests" | "tools" | "reftest"
        )
    });
    ext_ok
        && !reference
        && !skipped_dir
        && !stem.contains("-manual")
        && !stem.contains("-crash")
        && !stem.contains("-print")
}

/// Los tests (según [`is_test_file`]) bajo `dir`, recursivamente.
pub fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_files(&p, out);
        } else if is_test_file(&p) {
            out.push(p);
        }
    }
}

thread_local! {
    static LAST_PANIC: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// El mensaje del último pánico de este hilo (con [`install_panic_hook`]).
pub fn last_panic() -> String {
    LAST_PANIC.with(|p| p.borrow().clone())
}

/// Silencia los pánicos (se registran como `error` con su mensaje).
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_owned()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "pánico".to_owned()
        };
        let location = info
            .location()
            .map(|l| format!(" ({}:{})", l.file(), l.line()))
            .unwrap_or_default();
        LAST_PANIC.with(|p| *p.borrow_mut() = format!("{msg}{location}"));
    }));
}

pub struct Harness {
    pub wpt: PathBuf,
    pub out_dir: Option<PathBuf>,
    loader: Arc<dyn ResourceLoader>,
    db: Arc<FontDb>,
    /// Referencias usadas por varios tests: se pintan una sola vez.
    cache: Mutex<HashMap<PathBuf, Arc<Pixmap>>>,
    shared_refs: HashMap<PathBuf, usize>,
}

impl Harness {
    pub fn new(wpt: PathBuf, out_dir: Option<PathBuf>) -> Harness {
        let wpt = std::path::absolute(&wpt).unwrap_or(wpt);
        // Los tests de CSS2 dan por hecho que Ahem está instalada en el sistema.
        let db = Arc::new(FontDb::scan_with(
            &bf_font::db::system_font_dirs(),
            &[wpt.join("fonts/Ahem.ttf")],
        ));
        Harness {
            loader: Arc::new(FileLoader::new().mount(&wpt_origin(), wpt.clone())),
            wpt,
            out_dir,
            db,
            cache: Mutex::default(),
            shared_refs: HashMap::new(),
        }
    }

    /// Las fuentes del harness: las del sistema más Ahem.
    pub fn fonts(&self) -> Arc<FontDb> {
        self.db.clone()
    }

    /// URL con la que se sirve un archivo de WPT (`http://web-platform.test/<ruta>`).
    fn base(&self, path: &Path) -> Url {
        let rel = path
            .strip_prefix(&self.wpt)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        Url::parse(&rel, Some(&wpt_origin())).unwrap_or_else(|_| wpt_origin())
    }

    /// Archivo de una referencia `href` del documento `from`.
    pub fn resolve(&self, href: &str, from: &Path) -> Option<PathBuf> {
        let url = Url::parse(href, Some(&self.base(from))).ok()?;
        let prefix = wpt_origin().serialize(true);
        let href = url.serialize(true);
        let rest = href.strip_prefix(&prefix)?;
        let rest = rest.split('?').next().unwrap_or(rest);
        let rel = String::from_utf8_lossy(&bf_url::percent::decode(rest.as_bytes())).into_owned();
        Some(self.wpt.join(rel))
    }

    fn render_doc(&self, doc: &Document, path: &Path) -> Pixmap {
        if path.extension().is_some_and(is_svg_ext) {
            return self.render_svg(doc, path);
        }
        let base = self.base(path);
        let ctx = crate::PageContext {
            base: &base,
            encoding: Encoding::Utf8,
            loader: self.loader.clone(),
            fonts: self.db.clone(),
            extra: &[],
            text_zoom: 1.0,
            device_pixel_ratio: 1.0,
        };
        crate::render_document(doc, &ctx, VIEWPORT, 1.0)
    }

    /// E9: volcado del árbol de fragmentos de un archivo de WPT (ruta relativa a `tests/wpt`),
    /// para depurar un fallo.
    pub fn dump(&self, rel: &str) -> Option<String> {
        let path = self.wpt.join(rel);
        let (doc, _) = parse_file(&path)?;
        let base = self.base(&path);
        let ctx = crate::PageContext {
            base: &base,
            encoding: Encoding::Utf8,
            loader: self.loader.clone(),
            fonts: self.db.clone(),
            extra: &[],
            text_zoom: 1.0,
            device_pixel_ratio: 1.0,
        };
        let page = crate::prepare_page(&doc, &ctx, VIEWPORT);
        Some(bf_layout::dump(&doc, &page.tree))
    }

    /// Un documento SVG suelto (E7): su raíz es el viewport, del tamaño de sus `width` y
    /// `height` absolutos o del viewport de la ventana, sobre fondo blanco.
    fn render_svg(&self, doc: &Document, path: &Path) -> Pixmap {
        let (w, h) = (VIEWPORT.width as u32, VIEWPORT.height as u32);
        let mut out = Pixmap::new(w, h).expect("tamaño válido");
        out.fill(tiny_skia::Color::WHITE);
        let Some(root) = doc.children(doc.root()).find(|&n| doc.element(n).is_some()) else {
            return out;
        };
        let Ok(mut svg) = bf_svg::Svg::from_element(doc, root, &[], bf_svg::Inherited::default())
        else {
            return out;
        };
        // Documento de primer nivel: sus referencias a otros documentos se cargan.
        crate::images::load_svg_externals(
            &mut svg,
            Some(&self.base(path)),
            Some(self.loader.as_ref()),
        );
        let i = svg.intrinsic();
        let sw = i.width.unwrap_or(VIEWPORT.width);
        let sh = i.height.unwrap_or(VIEWPORT.height);
        let fonts = bf_font::Fonts::new(self.db.clone());
        // Documento de primer nivel: sus `image` se cargan con el loader del harness.
        let mut res = crate::images::SvgResources::default();
        res.load(&svg, Some(&self.base(path)), Some(self.loader.as_ref()));
        if let Some(p) = crate::images::rasterize_svg_with(
            &svg.tree(sw, sh),
            sw.ceil() as u32,
            sh.ceil() as u32,
            Some(&fonts),
            &res,
        ) {
            out.draw_pixmap(
                0,
                0,
                p.as_ref(),
                &tiny_skia::PixmapPaint::default(),
                tiny_skia::Transform::identity(),
                None,
            );
        }
        out
    }

    /// Pinta una referencia (cacheada si la usan varios tests). `Err` si no existe o usa JS.
    fn reference(&self, path: &Path) -> Result<Arc<Pixmap>, String> {
        if let Some(p) = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(path)
        {
            return Ok(p.clone());
        }
        let (doc, _) =
            parse_file(path).ok_or_else(|| format!("falta la referencia {}", path.display()))?;
        if meta(&doc).has_script {
            return Err("js".into());
        }
        let pixmap = Arc::new(self.render_doc(&doc, path));
        if self.shared_refs.get(path).copied().unwrap_or(0) >= 3 {
            self.cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(path.to_path_buf(), pixmap.clone());
        }
        Ok(pixmap)
    }

    /// Guarda `<test>-{actual,expected,diff}.png` en la carpeta de salida (si hay).
    pub fn save_failure(&self, test: &str, actual: &Pixmap, expected: &Pixmap) {
        let Some(out) = &self.out_dir else {
            return;
        };
        let stem = test.trim_end_matches(|c| c != '.').trim_end_matches('.');
        let base = out.join(stem);
        if let Some(parent) = base.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file = |suffix: &str| {
            let name = format!(
                "{}-{suffix}.png",
                base.file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default()
            );
            base.with_file_name(name)
        };
        let _ = actual.save_png(file("actual"));
        let _ = expected.save_png(file("expected"));
        let _ = diff_image(actual, expected).save_png(file("diff"));
    }

    /// Ruta del test relativa a la raíz de WPT, con `/`.
    pub fn test_name(&self, path: &Path) -> String {
        path.strip_prefix(&self.wpt)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn run_one(&self, path: &Path, group: &str) -> Outcome {
        catch_unwind(AssertUnwindSafe(|| self.run_one_inner(path, group))).unwrap_or_else(|_| {
            Outcome {
                test: self.test_name(path),
                group: group.to_owned(),
                status: Status::Error,
                detail: format!("pánico: {}", LAST_PANIC.with(|p| p.borrow().clone())),
            }
        })
    }

    fn run_one_inner(&self, path: &Path, group: &str) -> Outcome {
        let test = self.test_name(path);
        let outcome = |status: Status, detail: String| Outcome {
            test: test.clone(),
            group: group.to_owned(),
            status,
            detail,
        };
        let Some((doc, _)) = parse_file(path) else {
            return outcome(Status::Error, "no se pudo leer".into());
        };
        let m = meta(&doc);
        if m.matches.is_empty() && m.mismatches.is_empty() {
            return outcome(
                Status::NeedsJs,
                "sin referencia (testharness.js o manual)".into(),
            );
        }
        if m.has_script {
            return outcome(Status::NeedsJs, "<script>".into());
        }
        let result = catch_unwind(AssertUnwindSafe(|| -> Result<(Status, String), String> {
            let actual = self.render_doc(&doc, path);
            let v = judge(&m, &actual, &mut |href| {
                let ref_path = self
                    .resolve(href, path)
                    .ok_or_else(|| RefError::Other(format!("referencia no resoluble {href}")))?;
                self.reference(&ref_path).map_err(|e| {
                    if e == "js" {
                        RefError::NeedsJs
                    } else {
                        RefError::Other(e)
                    }
                })
            })?;
            if let Some(expected) = &v.failed_match {
                self.save_failure(&test, &actual, expected);
            }
            Ok((v.status, v.detail))
        }));
        match result {
            Ok(Ok((status, detail))) => outcome(status, detail),
            Ok(Err(e)) => outcome(Status::Error, e),
            Err(_) => outcome(
                Status::Error,
                format!("pánico: {}", LAST_PANIC.with(|p| p.borrow().clone())),
            ),
        }
    }

    /// Corre los tests de `groups` (directorios relativos a la raíz de WPT) en paralelo.
    /// `filter` restringe a las rutas que lo contienen. `progress` se llama cada tanto con
    /// (hechos, total).
    pub fn run(
        &mut self,
        groups: &[&str],
        filter: Option<&str>,
        progress: &(dyn Fn(usize, usize) + Sync),
    ) -> Vec<Outcome> {
        install_panic_hook();
        let mut tests: Vec<(PathBuf, String)> = Vec::new();
        for g in groups {
            // E12: con un filtro de ruta solo se recorren los grupos que pueden contenerlo
            // (recorrer todos tarda minutos con el disco cargado).
            if filter.is_some_and(|flt| !group_may_match(g, flt)) {
                continue;
            }
            let mut files = Vec::new();
            collect_files(&self.wpt.join(g), &mut files);
            files.sort();
            for f in files {
                let rel = f.to_string_lossy().replace('\\', "/");
                if filter.is_none_or(|flt| rel.contains(flt)) {
                    tests.push((f, (*g).to_owned()));
                }
            }
        }
        // Cuántos tests usa cada referencia (para cachear las compartidas).
        let mut counts: HashMap<PathBuf, usize> = HashMap::new();
        for (t, _) in &tests {
            let parsed = catch_unwind(AssertUnwindSafe(|| parse_file(t)))
                .ok()
                .flatten();
            if let Some((doc, _)) = parsed {
                let m = meta(&doc);
                for h in m.matches.iter().chain(&m.mismatches) {
                    if let Some(p) = self.resolve(h, t) {
                        *counts.entry(p).or_default() += 1;
                    }
                }
            }
        }
        self.shared_refs = counts;
        let next = AtomicUsize::new(0);
        let done = AtomicUsize::new(0);
        let results: Mutex<Vec<Outcome>> = Mutex::new(Vec::with_capacity(tests.len()));
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let this: &Harness = self;
        std::thread::scope(|scope| {
            for _ in 0..threads {
                std::thread::Builder::new()
                    .stack_size(256 << 20)
                    .spawn_scoped(scope, || {
                        loop {
                            let i = next.fetch_add(1, Ordering::SeqCst);
                            let Some((path, group)) = tests.get(i) else {
                                break;
                            };
                            let o = this.run_one(path, group);
                            results.lock().unwrap_or_else(|e| e.into_inner()).push(o);
                            let d = done.fetch_add(1, Ordering::SeqCst) + 1;
                            if d.is_multiple_of(200) || d == tests.len() {
                                progress(d, tests.len());
                            }
                        }
                    })
                    .expect("hilo del harness");
            }
        });
        let mut results = results.into_inner().unwrap_or_else(|e| e.into_inner());
        results.sort_by(|a, b| a.test.cmp(&b.test));
        results
    }
}

/// Directorios que mide el harness (relativos a la raíz de WPT).
pub const GROUPS: &[&str] = &[
    "css/CSS2/box-display",
    "css/CSS2/margin-padding-clear",
    "css/CSS2/normal-flow",
    "css/CSS2/floats",
    "css/CSS2/floats-clear",
    "css/CSS2/positioning",
    "css/CSS2/linebox",
    "css/CSS2/text",
    "css/CSS2/colors",
    "css/CSS2/borders",
    "css/css-backgrounds",
    "css/css-flexbox",
    "css/selectors",
    // E4: tablas, grid y outline.
    "css/CSS2/tables",
    "css/css-tables",
    "css/css-grid",
    "css/css-ui",
    // E4: transformaciones.
    "css/css-transforms",
    // E7: SVG (documentos `.svg` sueltos y SVG en línea en HTML).
    "svg",
    // E9: escritura vertical, bidi y texto.
    "css/css-writing-modes",
    "css/CSS2/bidi-text",
    "css/css-text",
    // E12: scroll, sticky y object-fit.
    "css/css-overflow",
    "css/css-position",
    "css/css-images",
    // E2: colores (CSS Color 4/5), valores y unidades, `env()`, `@property` y variables.
    "css/css-color",
    "css/css-values",
    "css/css-env",
    "css/css-properties-values-api",
    "css/css-variables",
];

/// ¿Puede haber tests del grupo `g` (`css/css-position`) cuya ruta contenga `flt`? Un filtro
/// sin `/` puede estar en cualquier grupo; uno con `/` tiene que contener el grupo, estar
/// dentro de él o empezar por su último segmento (`css-position/sticky`).
fn group_may_match(g: &str, flt: &str) -> bool {
    let last = g.rsplit('/').next().unwrap_or(g);
    !flt.contains('/') || flt.contains(g) || g.contains(flt) || flt.starts_with(last)
}

/// Tabla Markdown por directorio.
pub fn summary_table(results: &[Outcome], groups: &[&str]) -> String {
    let mut out = String::from(
        "| Directorio | pass | fail | needs-js | error | total | % pass (de los ejecutables) |\n|---|---:|---:|---:|---:|---:|---:|\n",
    );
    let mut totals = [0usize; 4];
    for g in groups {
        let mut c = [0usize; 4];
        for r in results.iter().filter(|r| r.group == *g) {
            c[r.status as usize] += 1;
        }
        for i in 0..4 {
            totals[i] += c[i];
        }
        out.push_str(&row(g, &c));
    }
    out.push_str(&row("**Total**", &totals));
    out
}

fn row(name: &str, c: &[usize; 4]) -> String {
    let runnable = c[0] + c[1] + c[3];
    let pct = if runnable > 0 {
        format!("{:.1}%", 100.0 * c[0] as f64 / runnable as f64)
    } else {
        "–".into()
    };
    format!(
        "| {name} | {} | {} | {} | {} | {} | {pct} |\n",
        c[0],
        c[1],
        c[2],
        c[3],
        c.iter().sum::<usize>()
    )
}

/// Corre los grupos, imprime la tabla, guarda `results.tsv` y las imágenes de los fallos en
/// `<workspace>/target/reftests/` y, si `docs` es verdadero, escribe `docs/wpt-status.md`.
pub fn run_and_report(workspace: &Path, filter: Option<&str>, docs: bool) -> Vec<Outcome> {
    let wpt = workspace.join("tests/wpt");
    let out = workspace.join("target/reftests");
    let _ = std::fs::remove_dir_all(&out);
    let _ = std::fs::create_dir_all(&out);
    let mut harness = Harness::new(wpt, Some(out.clone()));
    let start = std::time::Instant::now();
    let results = harness.run(GROUPS, filter, &|done, total| {
        eprintln!("reftests: {done}/{total} ({:.0?})", start.elapsed());
    });
    let table = summary_table(&results, GROUPS);
    println!("{table}");
    let mut tsv = String::from("test\tstatus\tdetail\n");
    for r in &results {
        tsv.push_str(&format!("{}\t{}\t{}\n", r.test, r.status.name(), r.detail));
    }
    let _ = std::fs::write(out.join("results.tsv"), tsv);
    if docs {
        let commit = std::fs::read_to_string(workspace.join("tools/wpt-commit.txt"))
            .unwrap_or_default()
            .trim()
            .to_owned();
        let date = today();
        let doc = [
            "# Estado de los reftests de WPT".to_owned(),
            String::new(),
            "Generado por `cargo test -p bf-paint --test wpt_reftests -- --ignored --nocapture`."
                .to_owned(),
            String::new(),
            format!("- Fecha: {date}"),
            format!("- Commit de WPT: `{commit}`"),
            "- Condiciones: viewport 800×600, DPR 1, fondo blanco, fuentes del sistema más Ahem \
             (`tests/wpt/fonts/Ahem.ttf`, como si estuviera instalada) y `@font-face`."
                .to_owned(),
            "- `.xht`/`.xhtml` se parsean como XML; el resto, como HTML.".to_owned(),
            "- `needs-js`: tests de testharness.js, manuales o con `<script>` o atributos \
             manejadores de eventos `on*` (en el test o en la referencia)."
                .to_owned(),
            "- `error`: pánico o recurso faltante.".to_owned(),
            "- El porcentaje se calcula sobre pass + fail + error.".to_owned(),
            String::new(),
            table,
            "Los fallos se guardan en `target/reftests/<ruta>-{actual,expected,diff}.png` y el \
             detalle por test en `target/reftests/results.tsv`."
                .to_owned(),
            String::new(),
            KNOWN_ISSUES.to_owned(),
        ]
        .join("\n");
        let _ = std::fs::create_dir_all(workspace.join("docs"));
        let _ = std::fs::write(workspace.join("docs/wpt-status.md"), doc);
    }
    results
}

/// Secciones fijas al final de `docs/wpt-status.md`.
const KNOWN_ISSUES: &str = "## Tests con expectativa dudosa

- `css/CSS2/text/letter-spacing-080.xht`: el test pide `letter-spacing: 6em` con `font: 20px`
  (120px), pero su referencia `letter-spacing-007-ref.xht` separa los cuadrados 96px (6em a
  16px). Falla por la referencia; no se toca.
- `css/CSS2/box-display/root-box-003.xht` (CSS 2.1: el fondo de `html` con `display: none`
  pinta el canvas) contradice `css/css-backgrounds/background-color-root-propagation-001.html`
  (CSS Backgrounds 3: no se propaga). Se sigue el spec vigente.
- `css/CSS2/margin-padding-clear/margin-collapse-038.xht` (CSS 2.2 §8.3.1 literal: el margen
  del último hijo atraviesa un padre recortado por `max-height`) contradice
  `css/CSS2/normal-flow/max-height-separates-margin.html`. Se sigue CSS 2.2.

## Pendientes conocidos

- Namespaces arbitrarios en el DOM (`Namespace::Other` con la URI): hoy el parser XML mapea
  los namespaces desconocidos o ausentes a HTML, así que `css/selectors/of-type-selectors.xhtml`
  no puede pasar.
- `css/css-grid`: 939 de los 1567 tests ejecutables son de `grid-lanes` (masonry) o `subgrid`,
  que no están implementados (roadmap). Sin ellos pasan 434 de 628 (69.1%).
- `border-image` no existe todavía (unos 50 fallos de `css/css-backgrounds`). `transform` se
  pinta en 2D: las 3D se aplanan, sin `perspective`, `transform-style` ni `backface-visibility`.
- El ajuste vertical de contornos (`FontProvider::vertical_fit`) es una aproximación propia al
  hinting, no comportamiento de spec; se desactiva con `Fonts::set_vertical_fit(false)`.
";

/// Fecha UTC de hoy (AAAA-MM-DD) sin dependencias: días desde la época civil.
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    // Algoritmo de Howard Hinnant (civil_from_days).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {

    #[test]
    fn group_filter() {
        assert!(group_may_match("css/css-position", "sticky"));
        assert!(group_may_match(
            "css/css-position",
            "css/css-position/sticky"
        ));
        assert!(group_may_match("css/css-position", "css-position/sticky"));
        assert!(group_may_match("css/CSS2/positioning", "css/CSS2"));
        assert!(!group_may_match("css/CSS2/positioning", "css/css-position"));
        assert!(!group_may_match("css/css-grid", "css/css-overflow/x.html"));
    }
    use super::*;

    #[test]
    fn fuzzy_syntax() {
        assert_eq!(
            parse_fuzzy("maxDifference=0-2;totalPixels=0-300"),
            Some((
                None,
                Fuzzy {
                    max_difference: (0, 2),
                    total_pixels: (0, 300)
                }
            ))
        );
        assert_eq!(
            parse_fuzzy("ref.html:0-1; 5-10").map(|(r, f)| (r, f.total_pixels)),
            Some((Some("ref.html".into()), (5, 10)))
        );
        assert_eq!(
            parse_fuzzy("maxDifference=1; totalPixels=0-948").map(|(_, f)| f.max_difference),
            Some((0, 1))
        );
    }

    #[test]
    fn test_files() {
        assert!(is_test_file(Path::new("css/CSS2/floats/floats-001.xht")));
        assert!(!is_test_file(Path::new(
            "css/CSS2/floats/floats-001-ref.xht"
        )));
        assert!(!is_test_file(Path::new(
            "css/CSS2/floats/float-nowrap-4-ref2.html"
        )));
        assert!(!is_test_file(Path::new(
            "css/CSS2/floats/float-nowrap-1-notref.html"
        )));
        assert!(!is_test_file(Path::new("css/CSS2/support/a.html")));
        assert!(!is_test_file(Path::new(
            "css/CSS2/floats/crashtests/x.html"
        )));
    }
}
