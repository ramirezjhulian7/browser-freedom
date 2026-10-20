//! Fuentes: base de datos de fuentes del sistema, `@font-face`, matching (CSS Fonts 4 §5),
//! fallback por carácter, shaping con `rustybuzz` y contornos de glifos.
//!
//! [`Fonts`] implementa [`bf_layout::FontProvider`] para un documento: combina la base de
//! datos compartida del sistema ([`FontDb`]) con las caras `@font-face` del documento.

#![forbid(unsafe_code)]

pub mod db;
pub mod face;
pub mod fontface;
pub mod matching;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use bf_css::syntax::Declaration;
use bf_infra::ResourceLoader;
use bf_layout::{FontKey, FontMetrics, FontProvider, FontRequest, Glyph, Outline};
use bf_style::properties::types::FamilyName;

pub use db::FontDb;
pub use face::{FaceStyle, LoadedFace};
pub use fontface::{FontFaceRule, Source, parse_font_face};
use matching::{Descriptors, best_match, generic_candidates};

/// Las claves con este bit son caras `@font-face` del documento; el resto, del sistema.
const WEB_BIT: u32 = 1 << 31;
/// Sin ninguna fuente disponible: glifos `.notdef` sin contorno.
const NO_FONT: FontKey = FontKey(u32::MAX);

struct WebFace {
    rule: FontFaceRule,
    loaded: OnceLock<Option<Arc<LoadedFace>>>,
}

type FamilyKey = (String, u32, bool);
type ShapeKey = (FamilyKey, u32, String);

/// Fuentes de un documento.
pub struct Fonts {
    db: Arc<FontDb>,
    web: Vec<WebFace>,
    loader: Option<Arc<dyn ResourceLoader>>,
    families: Mutex<HashMap<FamilyKey, Arc<Vec<Vec<FontKey>>>>>,
    shaped: Mutex<HashMap<ShapeKey, Arc<Vec<Glyph>>>>,
    web_outlines: db::OutlineCache,
    coverage: Mutex<HashMap<(FontKey, char), bool>>,
    vertical_fit: bool,
    /// `font-family` pedido → nombre de la cara elegida (para el reporte del motor).
    used: Mutex<std::collections::BTreeMap<String, String>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// ¿El contenido de `data` es un formato que sabemos leer (TrueType, OpenType, colección)?
fn is_sfnt(data: &[u8]) -> bool {
    matches!(
        data.get(0..4),
        Some([0, 1, 0, 0] | b"OTTO" | b"true" | b"ttcf")
    )
}

/// Caracteres que siguen al anterior si su cara los tiene (marcas combinantes, ZWJ,
/// selectores de variación), para no partir clusters entre fuentes.
fn continues_cluster(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F
        | 0x200C | 0x200D | 0xFE00..=0xFE0F | 0xE0100..=0xE01EF)
}

impl Fonts {
    pub fn new(db: Arc<FontDb>) -> Fonts {
        Fonts {
            db,
            web: Vec::new(),
            loader: None,
            families: Mutex::default(),
            shaped: Mutex::default(),
            web_outlines: Mutex::default(),
            coverage: Mutex::default(),
            vertical_fit: true,
            used: Mutex::default(),
        }
    }

    /// Activa o desactiva el ajuste vertical de contornos (ver
    /// [`FontProvider::vertical_fit`]); está activo por defecto.
    pub fn set_vertical_fit(&mut self, enabled: bool) {
        self.vertical_fit = enabled;
    }

    /// Fuentes del sistema (escaneadas una vez por proceso).
    pub fn system() -> Fonts {
        Fonts::new(FontDb::system())
    }

    pub fn db(&self) -> &FontDb {
        &self.db
    }

    /// Registra las reglas `@font-face` del documento. Las fuentes se cargan con `loader` la
    /// primera vez que se usan.
    pub fn add_font_faces(&mut self, rules: &[&[Declaration]], loader: Arc<dyn ResourceLoader>) {
        for decls in rules {
            if let Some(rule) = parse_font_face(decls) {
                self.web.push(WebFace {
                    rule,
                    loaded: OnceLock::new(),
                });
            }
        }
        self.loader = Some(loader);
        lock(&self.families).clear();
        lock(&self.used).clear();
        lock(&self.shaped).clear();
    }

    fn load_web(&self, i: usize) -> Option<Arc<LoadedFace>> {
        let web = &self.web[i];
        web.loaded
            .get_or_init(|| {
                web.rule.sources.iter().find_map(|src| match src {
                    Source::Url { url, format } => {
                        let supported = format.as_deref().is_none_or(|f| {
                            matches!(
                                f,
                                "truetype"
                                    | "opentype"
                                    | "collection"
                                    | "truetype-variations"
                                    | "opentype-variations"
                            )
                        });
                        if !supported {
                            return None;
                        }
                        let url = bf_infra::Url::parse(url, None).ok()?;
                        let data = self.loader.as_ref()?.load(&url)?.bytes;
                        if !is_sfnt(&data) {
                            // WOFF/WOFF2 necesitan descompresión (pendiente).
                            return None;
                        }
                        LoadedFace::new(Arc::new(data), 0).map(Arc::new)
                    }
                    Source::Local(name) => self.db.by_name(name).and_then(|i| self.db.load(i)),
                })
            })
            .clone()
    }

    fn face(&self, key: FontKey) -> Option<Arc<LoadedFace>> {
        if key == NO_FONT {
            None
        } else if key.0 & WEB_BIT != 0 {
            self.load_web((key.0 & !WEB_BIT) as usize)
        } else {
            self.db.load(key.0 as usize)
        }
    }

    fn covers(&self, key: FontKey, c: char) -> bool {
        if key == NO_FONT {
            return false;
        }
        if key.0 & WEB_BIT != 0 {
            let rule = &self.web[(key.0 & !WEB_BIT) as usize].rule;
            if !rule
                .ranges
                .iter()
                .any(|&(a, b)| (a..=b).contains(&(c as u32)))
            {
                return false;
            }
        }
        if let Some(&r) = lock(&self.coverage).get(&(key, c)) {
            return r;
        }
        let r = self
            .face(key)
            .and_then(|f| f.face().and_then(|face| face.glyph_index(c)))
            .is_some();
        lock(&self.coverage).insert((key, c), r);
        r
    }

    fn system_family(&self, name: &str, weight: f32, italic: bool) -> Option<FontKey> {
        let faces = self.db.family(name);
        let descriptors: Vec<Descriptors> = faces
            .iter()
            .map(|&i| {
                let f = &self.db.faces()[i];
                Descriptors {
                    weight: (f.weight, f.weight),
                    style: f.style,
                    stretch: f.stretch,
                }
            })
            .collect();
        best_match(&descriptors, weight, italic).map(|b| FontKey(faces[b] as u32))
    }

    /// Caras candidatas para cada familia de la lista: la mejor de cada familia, o la
    /// composición de caras `@font-face` con los mismos descriptores (las definidas después,
    /// primero).
    fn resolve(&self, request: &FontRequest<'_>) -> Arc<Vec<Vec<FontKey>>> {
        let key: FamilyKey = (
            format!("{:?}", request.family.0),
            request.weight.to_bits(),
            request.italic,
        );
        if let Some(r) = lock(&self.families).get(&key) {
            return r.clone();
        }
        let (weight, italic) = (request.weight, request.italic);
        let mut out: Vec<Vec<FontKey>> = Vec::new();
        for family in &request.family.0 {
            let found = match family {
                FamilyName::Named(name) => {
                    let lower = name.to_lowercase();
                    let web: Vec<usize> = (0..self.web.len())
                        .filter(|&i| self.web[i].rule.family == lower)
                        .collect();
                    if web.is_empty() {
                        self.system_family(name, weight, italic).map(|k| vec![k])
                    } else {
                        let descriptors: Vec<Descriptors> = web
                            .iter()
                            .map(|&i| Descriptors {
                                weight: self.web[i].rule.weight,
                                style: self.web[i].rule.style,
                                stretch: 5,
                            })
                            .collect();
                        best_match(&descriptors, weight, italic).map(|b| {
                            let best = descriptors[b];
                            web.iter()
                                .zip(&descriptors)
                                .rev()
                                .filter(|(_, d)| **d == best)
                                .map(|(&i, _)| FontKey(WEB_BIT | i as u32))
                                .collect()
                        })
                    }
                }
                FamilyName::Generic(g) => generic_candidates(g)
                    .iter()
                    .find_map(|name| self.system_family(name, weight, italic))
                    .map(|k| vec![k]),
            };
            if let Some(keys) = found
                && !out.contains(&keys)
            {
                out.push(keys);
            }
        }
        if out.is_empty() {
            // Fuente por defecto del UA.
            if let Some(k) = generic_candidates("serif")
                .iter()
                .find_map(|name| self.system_family(name, weight, italic))
            {
                out.push(vec![k]);
            }
        }
        if let Some(&first) = out.first().and_then(|k| k.first()) {
            let requested = request
                .family
                .0
                .iter()
                .map(|f| match f {
                    FamilyName::Named(n) => format!("\"{n}\""),
                    FamilyName::Generic(g) => g.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            lock(&self.used).insert(requested, self.face_name(first));
        }
        let out = Arc::new(out);
        lock(&self.families).insert(key, out.clone());
        out
    }

    /// Nombre de familia de una cara (las de `@font-face`, marcadas).
    fn face_name(&self, key: FontKey) -> String {
        if key.0 & WEB_BIT != 0 {
            let i = (key.0 & !WEB_BIT) as usize;
            return self
                .web
                .get(i)
                .map_or_else(String::new, |w| format!("{} (@font-face)", w.rule.family));
        }
        self.db
            .faces()
            .get(key.0 as usize)
            .and_then(|f| f.families.first().cloned())
            .unwrap_or_default()
    }

    /// Las familias pedidas por el documento y la cara que resolvió cada una (la primera
    /// disponible de la lista; el fallback por carácter puede usar otras).
    pub fn used_fonts(&self) -> Vec<(String, String)> {
        lock(&self.used)
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// La "first available font" (CSS Fonts 4 §5): la primera cuya cobertura incluye U+0020.
    fn primary(&self, request: &FontRequest<'_>) -> Option<(FontKey, Arc<LoadedFace>)> {
        let families = self.resolve(request);
        let all = families.iter().flatten().copied();
        all.clone()
            .find(|&k| self.covers(k, ' '))
            .or_else(|| all.clone().find(|&k| self.face(k).is_some()))
            .and_then(|k| self.face(k).map(|f| (k, f)))
    }

    fn font_for(&self, families: &[Vec<FontKey>], c: char) -> Option<FontKey> {
        families
            .iter()
            .flatten()
            .copied()
            .find(|&k| self.covers(k, c))
            .or_else(|| self.db.fallback(c).map(|i| FontKey(i as u32)))
    }

    /// `map[i]` es el offset en el texto original del byte `i` de `text` (ver `shape`).
    fn shape_run(&self, text: &str, key: FontKey, size: f32, map: &[usize], out: &mut Vec<Glyph>) {
        let face = self.face(key);
        let shaped = face.as_ref().and_then(|f| {
            let rb = rustybuzz::Face::from_slice(&f.data, f.index)?;
            let mut buffer = rustybuzz::UnicodeBuffer::new();
            buffer.push_str(text);
            buffer.guess_segment_properties();
            Some((rustybuzz::shape(&rb, &[], buffer), f.units_per_em))
        });
        let Some((glyphs, upem)) = shaped else {
            // Sin fuente: cajas vacías de medio em, como TestFont.
            out.extend(
                text.char_indices()
                    .map(|(i, _)| Glyph::simple(size * 0.5, map[i], 0, NO_FONT)),
            );
            return;
        };
        let scale = size / upem;
        for (info, pos) in glyphs.glyph_infos().iter().zip(glyphs.glyph_positions()) {
            out.push(Glyph {
                advance: pos.x_advance as f32 * scale,
                cluster: map[info.cluster as usize],
                id: u16::try_from(info.glyph_id).unwrap_or(0),
                font: key,
                x_offset: pos.x_offset as f32 * scale,
                y_offset: pos.y_offset as f32 * scale,
            });
        }
    }
}

impl FontProvider for Fonts {
    fn metrics(&self, request: &FontRequest<'_>) -> FontMetrics {
        let s = request.size;
        let Some((_, f)) = self.primary(request) else {
            return bf_layout::TestFont.metrics(request);
        };
        let k = s / f.units_per_em;
        FontMetrics {
            ascent: (f.ascender * k).round(),
            descent: (f.descender * k).round(),
            line_gap: (f.line_gap * k).round(),
            x_height: f.x_height * k,
            underline_offset: f.underline_offset * k,
            underline_thickness: f.underline_thickness * k,
            strikeout_offset: f.strikeout_offset * k,
            strikeout_thickness: f.strikeout_thickness * k,
        }
    }

    fn shape(&self, text: &str, request: &FontRequest<'_>) -> Vec<Glyph> {
        let fkey: FamilyKey = (
            format!("{:?}", request.family.0),
            request.weight.to_bits(),
            request.italic,
        );
        let key: ShapeKey = (fkey, request.size.to_bits(), text.to_owned());
        if let Some(g) = lock(&self.shaped).get(&key) {
            return g.as_ref().clone();
        }
        let families = self.resolve(request);
        let fallback = self.primary(request).map_or(NO_FONT, |(k, _)| k);
        // Runs de la misma cara. Los controles (tab, salto de línea) se dan forma como
        // espacios: el layout decide su ancho.
        let mut runs: Vec<(usize, usize, FontKey)> = Vec::new();
        for (i, c) in text.char_indices() {
            let end = i + c.len_utf8();
            let c = if c.is_control() { ' ' } else { c };
            if let Some(last) = runs.last_mut()
                && continues_cluster(c)
                && self.covers(last.2, c)
            {
                last.1 = end;
                continue;
            }
            let k = self.font_for(&families, c).unwrap_or(fallback);
            match runs.last_mut() {
                Some(last) if last.2 == k => last.1 = end,
                _ => runs.push((i, end, k)),
            }
        }
        let mut glyphs = Vec::with_capacity(text.len());
        for (start, end, k) in runs {
            // Los controles se sustituyen por espacios y eso puede cambiar la longitud en
            // bytes (U+0085, U+009D… ocupan 2). `map[offset_en_run] = offset_en_text`, para
            // que `Glyph::cluster` indexe `text` siempre en un límite de carácter.
            let mut run = String::with_capacity(end - start);
            let mut map = Vec::with_capacity(end - start + 1);
            for (i, c) in text[start..end].char_indices() {
                let c = if c.is_control() { ' ' } else { c };
                map.resize(run.len(), start + i);
                map.push(start + i);
                run.push(c);
            }
            map.resize(run.len() + 1, end);
            self.shape_run(&run, k, request.size, &map, &mut glyphs);
        }
        lock(&self.shaped).insert(key, Arc::new(glyphs.clone()));
        glyphs
    }

    fn vertical_fit_enabled(&self) -> bool {
        self.vertical_fit
    }

    fn vertical_fit(&self, font: FontKey, size: f32) -> (f32, f32) {
        let Some(f) = self.face(font) else {
            return (1.0, 1.0);
        };
        let k = size / f.units_per_em;
        let fit = |v: f32| {
            let px = v * k;
            if px > 0.0 { px.round() / px } else { 1.0 }
        };
        (fit(f.ascender), fit(f.descender))
    }

    fn outline(&self, font: FontKey, glyph: u16) -> Option<Arc<Outline>> {
        if font == NO_FONT {
            return None;
        }
        if font.0 & WEB_BIT == 0 {
            return self.db.outline(font.0 as usize, glyph);
        }
        let i = (font.0 & !WEB_BIT) as usize;
        if let Some(o) = lock(&self.web_outlines).get(&(i, glyph)) {
            return o.clone();
        }
        let o = self
            .load_web(i)
            .and_then(|f| f.outline(glyph))
            .map(Arc::new);
        lock(&self.web_outlines).insert((i, glyph), o.clone());
        o
    }
}
