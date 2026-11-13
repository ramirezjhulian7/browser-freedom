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
/// E5: en la clave de una cara del sistema, el `opsz` con que se dan forma y se dibujan sus
/// glifos (bits 20 a 29; 0 = el valor por defecto del eje). Ver [`Fonts::sized_key`].
const OPSZ_SHIFT: u32 = 20;
const OPSZ_MASK: u32 = 0x3FF << OPSZ_SHIFT;

/// E5: la clave sin `opsz` y el `opsz` que lleva.
fn split_key(key: FontKey) -> (FontKey, Option<u16>) {
    if key == NO_FONT || key.0 & WEB_BIT != 0 {
        return (key, None);
    }
    let o = (key.0 & OPSZ_MASK) >> OPSZ_SHIFT;
    (FontKey(key.0 & !OPSZ_MASK), (o > 0).then_some(o as u16))
}

struct WebFace {
    rule: FontFaceRule,
    loaded: OnceLock<Option<Arc<LoadedFace>>>,
}

type FamilyKey = (String, u32, bool);
/// Familia, tamaño, texto y dirección (`None`: la que adivina el shaper; `Some(rtl)`: la del
/// run bidi).
type ShapeKey = (FamilyKey, u32, String, Option<bool>);

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
        let (key, _) = split_key(key);
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
                    if web.is_empty() && matching::system_font_alias(name) {
                        generic_candidates("system-ui")
                            .iter()
                            .find_map(|n| self.system_family(n, weight, italic))
                            .map(|k| vec![k])
                    } else if web.is_empty() {
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

    /// Shaping con caché por familia, tamaño, texto y dirección (ver [`ShapeKey`]).
    fn shape_impl(&self, text: &str, request: &FontRequest<'_>, rtl: Option<bool>) -> Vec<Glyph> {
        let fkey: FamilyKey = (
            format!("{:?}", request.family.0),
            request.weight.to_bits(),
            request.italic,
        );
        let key: ShapeKey = (fkey, request.size.to_bits(), text.to_owned(), rtl);
        if let Some(g) = lock(&self.shaped).get(&key) {
            return g.as_ref().clone();
        }
        let families = self.resolve(request);
        let fallback = self.primary(request).map_or(NO_FONT, |(k, _)| k);
        // Runs de la misma cara. Los controles (tab, salto de línea) se dan forma como
        // espacios: el layout decide su ancho.
        // E9: además, runs de un solo script (UAX #24), para darle al shaper el script del
        // segmento y elegir el fallback por script.
        let mut runs: Vec<(usize, usize, FontKey, bf_layout::unicode::Script)> = Vec::new();
        for (sstart, send, script) in bf_layout::unicode::script_runs(text) {
            let run_family = self.run_family(&families, &text[sstart..send], script.tag(), request);
            for (i, c) in text[sstart..send].char_indices() {
                let i = sstart + i;
                let end = i + c.len_utf8();
                let c = if c.is_control() { ' ' } else { c };
                if let Some(last) = runs.last_mut()
                    && last.3 == script
                    && continues_cluster(c)
                    && self.covers(last.2, c)
                {
                    last.1 = end;
                    continue;
                }
                // E5: seguido de U+FE0F (presentación emoji, UTS #51 §4) o U+20E3 (keycap),
                // el carácter se dibuja con la fuente de emoji si la tiene.
                let emoji_next =
                    matches!(text[end..].chars().next(), Some('\u{FE0F}' | '\u{20E3}'));
                let k = emoji_next
                    .then(|| self.emoji_font(c, request))
                    .flatten()
                    .or_else(|| self.font_for(&families, c, script.tag(), request, run_family))
                    .unwrap_or(fallback);
                match runs.last_mut() {
                    Some(last) if last.2 == k && last.3 == script => last.1 = end,
                    _ => runs.push((i, end, k, script)),
                }
            }
        }
        // Un run bidi RTL sale en orden visual: los runs de fuente/script, de atrás adelante.
        if rtl == Some(true) {
            runs.reverse();
        }
        let mut glyphs = Vec::with_capacity(text.len());
        for (start, end, k, script) in runs {
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
            self.shape_run(
                &run,
                self.sized_key(k, request.size),
                request.size,
                &map,
                Some(script.tag()),
                rtl,
                &mut glyphs,
            );
        }
        lock(&self.shaped).insert(key, Arc::new(glyphs.clone()));
        glyphs
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

    /// Cara para `c`: la primera de las familias pedidas que lo cubra; si no, E9: las
    /// familias de fallback del script de `script` (matching::script_fallback), y por último el
    /// fallback por carácter de la base de datos. E5: dentro de la familia de fallback, la cara
    /// de peso y estilo más cercanos a los pedidos que tenga `c` (CSS Fonts 4 §5.2), para que
    /// un texto CJK no mezcle los pesos de una familia como Hiragino (W0…W9).
    fn font_for(
        &self,
        families: &[Vec<FontKey>],
        c: char,
        script: &str,
        request: &FontRequest<'_>,
        run_family: Option<&str>,
    ) -> Option<FontKey> {
        families
            .iter()
            .flatten()
            .copied()
            .find(|&k| self.covers(k, c))
            .or_else(|| {
                run_family
                    .into_iter()
                    .chain(matching::script_fallback(script).iter().copied())
                    .find_map(|name| self.best_covering(self.db.family(name), c, request))
            })
            .or_else(|| {
                let i = self.db.fallback(c)?;
                // La familia más amplia de la cara (la tipográfica, no la "legacy" de un peso).
                let faces = self.db.faces()[i]
                    .families
                    .iter()
                    .map(|n| self.db.family(n))
                    .max_by_key(|f| f.len())
                    .unwrap_or(&[]);
                self.best_covering(faces, c, request)
                    .or(Some(FontKey(i as u32)))
            })
    }

    /// E5: la familia de fallback del script que cubre todos los caracteres de `segment` (un
    /// tramo de un solo script) que no tienen las familias pedidas, si hay alguna. Así un texto
    /// chino sin `lang` sale de una sola familia (Hiragino Sans GB) en vez de mezclarla con la
    /// japonesa (Hiragino Sans) carácter a carácter.
    fn run_family(
        &self,
        families: &[Vec<FontKey>],
        segment: &str,
        script: &str,
        request: &FontRequest<'_>,
    ) -> Option<&'static str> {
        let needed: Vec<char> = segment
            .chars()
            .filter(|&c| !c.is_control() && !c.is_whitespace())
            .filter(|&c| !families.iter().flatten().any(|&k| self.covers(k, c)))
            .collect();
        if needed.is_empty() {
            return None;
        }
        matching::script_fallback(script)
            .iter()
            .copied()
            .find(|name| {
                let faces = self.db.family(name);
                !faces.is_empty()
                    && needed
                        .iter()
                        .all(|&c| self.best_covering(faces, c, request).is_some())
            })
    }

    /// E5: la clave con que se dan forma y se dibujan los glifos de la cara `key` a `size` px:
    /// si la cara tiene eje `opsz`, con el tamaño óptico igual al tamaño de fuente en px
    /// dentro del rango del eje (`font-optical-sizing: auto`, CSS Fonts 4 §7.4), redondeado.
    fn sized_key(&self, key: FontKey, size: f32) -> FontKey {
        let (base, _) = split_key(key);
        if base == NO_FONT || base.0 & WEB_BIT != 0 || base.0 & OPSZ_MASK != 0 {
            return base;
        }
        let Some((min, max)) = self.db.faces().get(base.0 as usize).and_then(|f| f.opsz) else {
            return base;
        };
        let opsz = size.clamp(min, max).round().clamp(1.0, 1023.0) as u32;
        FontKey(base.0 | opsz << OPSZ_SHIFT)
    }

    /// E5: el índice en [`FontDb::faces`] de la cara del sistema que dibuja con `key` (`None`
    /// para las caras `@font-face` y sin fuente).
    pub fn system_face(&self, key: FontKey) -> Option<usize> {
        let (base, _) = split_key(key);
        (base != NO_FONT && base.0 & WEB_BIT == 0)
            .then_some(base.0 as usize)
            .filter(|&i| i < self.db.faces().len())
    }

    /// E5: la cara de la genérica `emoji` que tenga `c`.
    fn emoji_font(&self, c: char, request: &FontRequest<'_>) -> Option<FontKey> {
        generic_candidates("emoji")
            .iter()
            .find_map(|name| self.best_covering(self.db.family(name), c, request))
    }

    /// E5: la cara de `faces` (de una familia del sistema) que mejor encaja con el peso y estilo
    /// pedidos (CSS Fonts 4 §5.2) entre las que tienen `c`. Se prueban en orden de preferencia,
    /// así que normalmente solo se carga una.
    fn best_covering(
        &self,
        faces: &[usize],
        c: char,
        request: &FontRequest<'_>,
    ) -> Option<FontKey> {
        let mut rest: Vec<usize> = faces.to_vec();
        while !rest.is_empty() {
            let descriptors: Vec<Descriptors> = rest
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
            let b = best_match(&descriptors, request.weight, request.italic)?;
            let k = FontKey(rest[b] as u32);
            if self.covers(k, c) {
                return Some(k);
            }
            rest.remove(b);
        }
        None
    }

    /// `map[i]` es el offset en el texto original del byte `i` de `text` (ver `shape`).
    /// E9: `script` (etiqueta ISO 15924) y `rtl` fijan las propiedades del segmento; sin ellos
    /// las adivina rustybuzz.
    #[allow(clippy::too_many_arguments)]
    fn shape_run(
        &self,
        text: &str,
        key: FontKey,
        size: f32,
        map: &[usize],
        script: Option<&str>,
        rtl: Option<bool>,
        out: &mut Vec<Glyph>,
    ) {
        let face = self.face(key);
        let (_, opsz) = split_key(key);
        let shaped = face.as_ref().and_then(|f| {
            let mut rb = rustybuzz::Face::from_slice(&f.data, f.index)?;
            // E5: la instancia de una fuente variable (avances con `HVAR`, sustituciones) y su
            // tamaño óptico.
            let variations: Vec<rustybuzz::Variation> = f
                .variations(opsz.map(f32::from))
                .into_iter()
                .map(|(tag, value)| rustybuzz::Variation { tag, value })
                .collect();
            if !variations.is_empty() {
                rb.set_variations(&variations);
            }
            let mut buffer = rustybuzz::UnicodeBuffer::new();
            buffer.push_str(text);
            if let Some(script) = script
                .filter(|t| !matches!(*t, "Zyyy" | "Zinh" | "Zzzz"))
                .map(|t| rustybuzz::ttf_parser::Tag::from_bytes_lossy(t.as_bytes()))
                .and_then(rustybuzz::Script::from_iso15924_tag)
            {
                buffer.set_script(script);
            }
            if let Some(rtl) = rtl {
                buffer.set_direction(if rtl {
                    rustybuzz::Direction::RightToLeft
                } else {
                    rustybuzz::Direction::LeftToRight
                });
            }
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
        self.shape_impl(text, request, None)
    }

    fn shape_dir(&self, text: &str, request: &FontRequest<'_>, rtl: bool) -> Vec<Glyph> {
        self.shape_impl(text, request, Some(rtl))
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
            let (base, opsz) = split_key(font);
            return self.db.outline_at(base.0 as usize, glyph, opsz);
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
