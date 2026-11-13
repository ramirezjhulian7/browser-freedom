//! Base de datos de fuentes del sistema: escaneo de los directorios de fuentes de cada
//! plataforma, metadatos de cada cara (familias, peso, estilo, ancho) y carga perezosa.
//!
//! El escaneo no lee archivos completos: solo la cabecera, el directorio de tablas y las tablas
//! `name` y `OS/2`, sobre un buffer del tamaño del archivo que el resto deja en cero. Los datos
//! completos se leen la primera vez que se usa la cara.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use bf_layout::Outline;
use ttf_parser::{RawFace, Tag, name, name_id, os2};

use crate::face::{FaceStyle, LoadedFace};

/// Metadatos de una cara del sistema.
#[derive(Debug, Clone)]
pub struct FaceInfo {
    pub path: PathBuf,
    /// Índice dentro de una colección (`.ttc`).
    pub index: u32,
    /// Nombres de familia (tipográfica y "legacy"), en minúsculas.
    pub families: Vec<String>,
    /// Nombre completo y nombre PostScript (para `local()`), en minúsculas.
    pub names: Vec<String>,
    pub weight: f32,
    pub style: FaceStyle,
    /// `usWidthClass` (1–9, 5 es normal).
    pub stretch: u16,
    /// `ulUnicodeRange` de OS/2: filtro previo del fallback por carácter.
    pub unicode_ranges: u128,
    /// E5: en una fuente variable, el valor del eje `wght` de esta instancia (ver
    /// [`variable_instances`]).
    pub wght: Option<f32>,
    /// E5: rango del eje `opsz` de una fuente variable (mínimo, máximo).
    pub opsz: Option<(f32, f32)>,
}

/// Contornos cacheados por (cara, glifo); `None` si el glifo no tiene contorno.
pub type OutlineCache = Mutex<HashMap<(usize, u16), Option<Arc<Outline>>>>;

/// E5: contornos de las caras del sistema por (cara, glifo, `opsz`; 0 = por defecto).
type SizedOutlineCache = Mutex<HashMap<(usize, u16, u16), Option<Arc<Outline>>>>;

#[derive(Debug, Default)]
pub struct FontDb {
    faces: Vec<FaceInfo>,
    by_family: HashMap<String, Vec<usize>>,
    by_name: HashMap<String, usize>,
    loaded: Vec<OnceLock<Option<Arc<LoadedFace>>>>,
    files: Mutex<HashMap<PathBuf, Option<Arc<Vec<u8>>>>>,
    /// E5: la tabla `cmap` de cada cara leída sola del archivo, para saber qué caracteres
    /// tiene sin leer el archivo entero (ver [`FontDb::covers`]).
    cmaps: Vec<OnceLock<Option<Vec<u8>>>>,
    fallback: Mutex<HashMap<char, Option<usize>>>,
    outlines: SizedOutlineCache,
}

/// E5: lee del archivo solo la tabla `cmap` de la cara `index` (sfnt o colección `ttcf`):
/// la cabecera, el directorio de tablas y la tabla. `None` si el archivo no tiene esa forma o
/// le falta alguna tabla sin la que `ttf_parser::Face::parse` falla (`head`, `hhea`, `maxp`);
/// entonces se carga entero como antes.
fn read_cmap(path: &Path, index: u32) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let mut read_at = |offset: u64, len: usize| -> Option<Vec<u8>> {
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut buf = vec![0; len];
        file.read_exact(&mut buf).ok()?;
        Some(buf)
    };
    let u16_at = |b: &[u8], i: usize| -> Option<u16> {
        Some(u16::from_be_bytes(b.get(i..i + 2)?.try_into().ok()?))
    };
    let u32_at = |b: &[u8], i: usize| -> Option<u32> {
        Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
    };
    let head = read_at(0, 12)?;
    let start = if head.get(0..4)? == b"ttcf" {
        let count = u32_at(&head, 8)?;
        if index >= count {
            return None;
        }
        let off = read_at(12 + 4 * u64::from(index), 4)?;
        u64::from(u32_at(&off, 0)?)
    } else if index == 0 && matches!(head.get(0..4)?, [0, 1, 0, 0] | b"OTTO" | b"true") {
        0
    } else {
        return None;
    };
    let header = if start == 0 {
        head
    } else {
        read_at(start, 12)?
    };
    let tables = usize::from(u16_at(&header, 4)?);
    let records = read_at(start + 12, 16 * tables)?;
    let mut cmap = None;
    let mut required = 0;
    for r in records.as_chunks::<16>().0 {
        match r.get(0..4)? {
            b"cmap" => cmap = Some((u32_at(r, 8)?, u32_at(r, 12)?)),
            b"head" | b"hhea" | b"maxp" => required += 1,
            _ => {}
        }
    }
    let (offset, len) = cmap.filter(|_| required == 3)?;
    read_at(u64::from(offset), usize::try_from(len).ok()?)
}

/// Directorios de fuentes de Windows, Linux y macOS (los que existan).
pub fn system_font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    if let Some(windir) = env("WINDIR").or_else(|| env("SystemRoot")) {
        dirs.push(windir.join("Fonts"));
    } else {
        dirs.push(PathBuf::from(r"C:\Windows\Fonts"));
    }
    if let Some(local) = env("LOCALAPPDATA") {
        dirs.push(local.join(r"Microsoft\Windows\Fonts"));
    }
    dirs.push(PathBuf::from("/usr/share/fonts"));
    dirs.push(PathBuf::from("/usr/local/share/fonts"));
    dirs.push(PathBuf::from("/Library/Fonts"));
    dirs.push(PathBuf::from("/System/Library/Fonts"));
    if let Some(home) = env("HOME") {
        dirs.push(home.join(".fonts"));
        dirs.push(home.join(".local/share/fonts"));
        dirs.push(home.join("Library/Fonts"));
    }
    dirs.retain(|d| d.is_dir());
    dirs
}

fn font_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            font_files(&path, out);
        } else if path.extension().is_some_and(|e| {
            let e = e.to_string_lossy().to_ascii_lowercase();
            matches!(e.as_str(), "ttf" | "otf" | "ttc" | "otc")
        }) {
            out.push(path);
        }
    }
}

fn read_at(file: &mut File, buf: &mut [u8], offset: usize, len: usize) -> Option<()> {
    let end = offset.checked_add(len)?;
    let dest = buf.get_mut(offset..end)?;
    file.seek(SeekFrom::Start(offset as u64)).ok()?;
    file.read_exact(dest).ok()
}

fn be_u32(buf: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(buf.get(at..at + 4)?.try_into().ok()?))
}

fn be_u16(buf: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(buf.get(at..at + 2)?.try_into().ok()?))
}

/// Metadatos de las caras de un archivo.
fn scan_file(path: &Path) -> Vec<FaceInfo> {
    scan_file_inner(path).unwrap_or_default()
}

fn scan_file_inner(path: &Path) -> Option<Vec<FaceInfo>> {
    let mut file = File::open(path).ok()?;
    let len = usize::try_from(file.metadata().ok()?.len()).ok()?;
    if !(12..=1 << 30).contains(&len) {
        return None;
    }
    // Las páginas que no se escriben no llegan a ocupar memoria.
    let mut buf = vec![0u8; len];
    read_at(&mut file, &mut buf, 0, 12)?;
    let offsets: Vec<usize> = if &buf[0..4] == b"ttcf" {
        let n = be_u32(&buf, 8)? as usize;
        read_at(&mut file, &mut buf, 12, n.checked_mul(4)?)?;
        (0..n)
            .map(|i| be_u32(&buf, 12 + 4 * i).map(|o| o as usize))
            .collect::<Option<_>>()?
    } else {
        vec![0]
    };
    let mut faces = Vec::new();
    for (index, &offset) in offsets.iter().enumerate() {
        if read_at(&mut file, &mut buf, offset, 12).is_none() {
            continue;
        }
        let Some(tables) = be_u16(&buf, offset + 4) else {
            continue;
        };
        if read_at(&mut file, &mut buf, offset + 12, 16 * usize::from(tables)).is_none() {
            continue;
        }
        let index = index as u32;
        let wanted: Vec<(usize, usize)> = {
            let Ok(raw) = RawFace::parse(&buf, index) else {
                continue;
            };
            raw.table_records
                .into_iter()
                .filter(|r| {
                    [b"name", b"OS/2", b"fvar"]
                        .iter()
                        .any(|t| r.tag == Tag::from_bytes(t))
                })
                .map(|r| (r.offset as usize, r.length as usize))
                .collect()
        };
        for (o, l) in wanted {
            read_at(&mut file, &mut buf, o, l);
        }
        let Ok(raw) = RawFace::parse(&buf, index) else {
            continue;
        };
        if let Some(info) = face_info(path, index, &raw) {
            faces.extend(variable_instances(info, &raw));
        }
    }
    Some(faces)
}

/// Los caracteres de Mac OS Roman de `0x80` a `0xFF` (Apple, `ROMAN.TXT`; `0xDB` es `€` desde
/// Mac OS 8.5 y `0xF0` el logotipo de Apple, `U+F8FF`). De `0x00` a `0x7F` es ASCII.
const MAC_ROMAN_HIGH: [char; 128] = [
    'Ä', 'Å', 'Ç', 'É', 'Ñ', 'Ö', 'Ü', 'á', 'à', 'â', 'ä', 'ã', 'å', 'ç', 'é', 'è', //
    'ê', 'ë', 'í', 'ì', 'î', 'ï', 'ñ', 'ó', 'ò', 'ô', 'ö', 'õ', 'ú', 'ù', 'û', 'ü', //
    '†', '°', '¢', '£', '§', '•', '¶', 'ß', '®', '©', '™', '´', '¨', '≠', 'Æ', 'Ø', //
    '∞', '±', '≤', '≥', '¥', 'µ', '∂', '∑', '∏', 'π', '∫', 'ª', 'º', 'Ω', 'æ', 'ø', //
    '¿', '¡', '¬', '√', 'ƒ', '≈', '∆', '«', '»', '…', '\u{a0}', 'À', 'Ã', 'Õ', 'Œ', 'œ', //
    '–', '—', '“', '”', '‘', '’', '÷', '◊', 'ÿ', 'Ÿ', '⁄', '€', '‹', '›', 'ﬁ', 'ﬂ', //
    '‡', '·', '‚', '„', '‰', 'Â', 'Ê', 'Á', 'Ë', 'È', 'Í', 'Î', 'Ï', 'Ì', 'Ó', 'Ô', //
    '\u{f8ff}', 'Ò', 'Ú', 'Û', 'Ù', 'ı', 'ˆ', '˜', '¯', '˘', '˙', '˚', '¸', '˝', '˛', 'ˇ', //
];

/// Un nombre de la plataforma Macintosh con codificación Roman (OpenType, tabla `name`:
/// "Macintosh platform-specific encoding IDs", 0 = Roman), que `ttf-parser` no decodifica.
/// Muchas fuentes de Apple (Helvetica, Menlo, Courier, Monaco, Geneva, Apple Color Emoji)
/// solo traen nombres de esa plataforma; sin esto no se encuentran por familia.
fn mac_roman_name(n: &name::Name<'_>) -> Option<String> {
    if n.platform_id != name::PlatformId::Macintosh || n.encoding_id != 0 {
        return None;
    }
    Some(
        n.name
            .iter()
            .map(|&b| match b {
                0..=0x7F => char::from(b),
                _ => MAC_ROMAN_HIGH[usize::from(b - 0x80)],
            })
            .collect(),
    )
}

fn face_info(path: &Path, index: u32, raw: &RawFace<'_>) -> Option<FaceInfo> {
    let names = name::Table::parse(raw.table(Tag::from_bytes(b"name"))?)?;
    let mut families = Vec::new();
    let mut full = Vec::new();
    let mut subfamily = String::new();
    for n in names.names {
        let Some(s) = n.to_string().or_else(|| mac_roman_name(&n)) else {
            continue;
        };
        let s = s.trim().to_lowercase();
        if s.is_empty() {
            continue;
        }
        let list = match n.name_id {
            name_id::TYPOGRAPHIC_FAMILY | name_id::FAMILY => &mut families,
            name_id::FULL_NAME | name_id::POST_SCRIPT_NAME => &mut full,
            name_id::SUBFAMILY => {
                subfamily = s;
                continue;
            }
            _ => continue,
        };
        if !list.contains(&s) {
            list.push(s);
        }
    }
    if families.is_empty() {
        return None;
    }
    let (weight, style, stretch, unicode_ranges) = match raw
        .table(Tag::from_bytes(b"OS/2"))
        .and_then(os2::Table::parse)
    {
        Some(os2) => (
            f32::from(os2.weight().to_number()),
            match os2.style() {
                os2::Style::Normal => FaceStyle::Normal,
                os2::Style::Italic => FaceStyle::Italic,
                os2::Style::Oblique => FaceStyle::Oblique,
            },
            os2.width().to_number(),
            os2.unicode_ranges().0,
        ),
        // Sin OS/2 (fuentes Mac antiguas): lo que diga el nombre de la subfamilia.
        None => (
            if subfamily.contains("bold") {
                700.0
            } else {
                400.0
            },
            if subfamily.contains("italic") {
                FaceStyle::Italic
            } else if subfamily.contains("oblique") {
                FaceStyle::Oblique
            } else {
                FaceStyle::Normal
            },
            5,
            u128::MAX,
        ),
    };
    Some(FaceInfo {
        path: path.to_path_buf(),
        index,
        families,
        names: full,
        weight,
        style,
        stretch,
        unicode_ranges,
        wght: None,
        opsz: None,
    })
}

/// E5: una fuente variable con eje `wght` (como `SFNS.ttf`, la del sistema en macOS) se
/// registra como una cara por cada peso CSS de 100 a 900 dentro del rango del eje (CSS Fonts 4
/// §5.2 elige entre ellas como entre caras estáticas). Los demás ejes quedan en su valor por
/// defecto. Sin ese eje, la cara tal cual.
fn variable_instances(mut info: FaceInfo, raw: &RawFace<'_>) -> Vec<FaceInfo> {
    let fvar = raw
        .table(Tag::from_bytes(b"fvar"))
        .and_then(ttf_parser::fvar::Table::parse);
    let axis = |tag| fvar.and_then(|f| f.axes.into_iter().find(|a| a.tag == tag));
    info.opsz = axis(crate::face::OPSZ).map(|a| (a.min_value, a.max_value));
    let Some(axis) = axis(crate::face::WGHT) else {
        return vec![info];
    };
    let instances: Vec<FaceInfo> = (1..=9)
        .map(|w| w as f32 * 100.0)
        .filter(|w| (axis.min_value..=axis.max_value).contains(w))
        .map(|w| FaceInfo {
            weight: w,
            wght: Some(w),
            ..info.clone()
        })
        .collect();
    if instances.is_empty() {
        vec![info]
    } else {
        instances
    }
}

impl FontDb {
    /// Base de datos de las fuentes del sistema, escaneada una sola vez por proceso.
    pub fn system() -> Arc<FontDb> {
        static DB: OnceLock<Arc<FontDb>> = OnceLock::new();
        DB.get_or_init(|| Arc::new(FontDb::scan(&system_font_dirs())))
            .clone()
    }

    /// Escanea los directorios dados (en paralelo).
    pub fn scan(dirs: &[PathBuf]) -> FontDb {
        FontDb::scan_with(dirs, &[])
    }

    /// Escanea los directorios y además los archivos sueltos `extra` (el harness de WPT agrega
    /// Ahem como si estuviera instalada).
    pub fn scan_with(dirs: &[PathBuf], extra: &[PathBuf]) -> FontDb {
        let mut files = extra.to_vec();
        for d in dirs {
            font_files(d, &mut files);
        }
        files.sort();
        files.dedup();
        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .clamp(1, 16);
        let chunk = files.len().div_ceil(threads).max(1);
        let faces: Vec<FaceInfo> = std::thread::scope(|scope| {
            let handles: Vec<_> = files
                .chunks(chunk)
                .map(|paths| {
                    scope.spawn(move || paths.iter().flat_map(|p| scan_file(p)).collect::<Vec<_>>())
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap_or_default())
                .collect()
        });
        FontDb::from_faces(faces)
    }

    pub fn from_faces(faces: Vec<FaceInfo>) -> FontDb {
        let mut by_family: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_name = HashMap::new();
        for (i, f) in faces.iter().enumerate() {
            for fam in &f.families {
                by_family.entry(fam.clone()).or_default().push(i);
            }
            for n in &f.names {
                by_name.entry(n.clone()).or_insert(i);
            }
        }
        let loaded = (0..faces.len()).map(|_| OnceLock::new()).collect();
        let cmaps = (0..faces.len()).map(|_| OnceLock::new()).collect();
        FontDb {
            faces,
            by_family,
            by_name,
            loaded,
            cmaps,
            ..FontDb::default()
        }
    }

    pub fn faces(&self) -> &[FaceInfo] {
        &self.faces
    }

    /// Caras de la familia `name` (sin distinguir mayúsculas).
    pub fn family(&self, name: &str) -> &[usize] {
        self.by_family
            .get(&name.trim().to_lowercase())
            .map_or(&[], Vec::as_slice)
    }

    /// Cara por nombre completo o PostScript (`local()` de `@font-face`).
    pub fn by_name(&self, name: &str) -> Option<usize> {
        self.by_name.get(&name.trim().to_lowercase()).copied()
    }

    fn file(&self, path: &Path) -> Option<Arc<Vec<u8>>> {
        let mut files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        files
            .entry(path.to_path_buf())
            .or_insert_with(|| std::fs::read(path).ok().map(Arc::new))
            .clone()
    }

    /// Datos completos de la cara `i` (se leen del disco la primera vez).
    pub fn load(&self, i: usize) -> Option<Arc<LoadedFace>> {
        let info = self.faces.get(i)?;
        self.loaded[i]
            .get_or_init(|| {
                let data = self.file(&info.path)?;
                LoadedFace::new(data, info.index).map(|f| Arc::new(f.with_wght(info.wght)))
            })
            .clone()
    }

    ///
    /// E5: si la cara todavía no se cargó, se mira solo su tabla `cmap` (unos KB leídos del
    /// archivo): el fallback por carácter prueba muchas caras y algunos archivos son enormes
    /// (Apple Color Emoji ocupa 192 MB). Igual que `ttf_parser::Face::glyph_index`: el primer
    /// subtable Unicode que tenga el carácter.
    pub fn covers(&self, i: usize, c: char) -> bool {
        let loaded = self.loaded.get(i).and_then(OnceLock::get).is_some()
            || self.faces.get(i).is_some_and(|f| {
                let files = self.files.lock().unwrap_or_else(|e| e.into_inner());
                files.contains_key(&f.path)
            });
        if !loaded
            && let Some(Some(cmap)) = self.cmaps.get(i).map(|slot| {
                slot.get_or_init(|| {
                    let f = self.faces.get(i)?;
                    read_cmap(&f.path, f.index)
                })
            })
        {
            return ttf_parser::cmap::Table::parse(cmap).is_some_and(|t| {
                t.subtables
                    .into_iter()
                    .filter(|s| s.is_unicode())
                    .any(|s| s.glyph_index(u32::from(c)).is_some())
            });
        }
        self.load(i)
            .and_then(|f| f.face().and_then(|face| face.glyph_index(c)))
            .is_some()
    }

    /// Contorno (cacheado) de un glifo de la cara `i`.
    pub fn outline(&self, i: usize, glyph: u16) -> Option<Arc<Outline>> {
        self.outline_at(i, glyph, None)
    }

    /// E5: [`FontDb::outline`] con el tamaño óptico `opsz` (entero, ver
    /// [`LoadedFace::face_at`]).
    pub fn outline_at(&self, i: usize, glyph: u16, opsz: Option<u16>) -> Option<Arc<Outline>> {
        let key = (i, glyph, opsz.unwrap_or(0));
        if let Some(o) = self
            .outlines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            return o.clone();
        }
        let outline = self
            .load(i)
            .and_then(|f| f.outline_at(glyph, opsz.map(f32::from)))
            .map(Arc::new);
        self.outlines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, outline.clone());
        outline
    }

    /// Una cara del sistema que tenga glifo para `c`: primero las familias de fallback de la
    /// plataforma y después cualquier cara cuyo `ulUnicodeRange` declare el bloque de `c`,
    /// prefiriendo las de peso y estilo normales.
    pub fn fallback(&self, c: char) -> Option<usize> {
        if let Some(r) = self
            .fallback
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&c)
        {
            return *r;
        }
        let ranges_check = |i: &usize| {
            let r = os2::UnicodeRanges(self.faces[*i].unicode_ranges);
            r.0 == 0 || r.contains_char(c)
        };
        let mut candidates: Vec<usize> = crate::matching::FALLBACK_FAMILIES
            .iter()
            .flat_map(|f| self.family(f).iter().copied())
            .collect();
        let mut rest: Vec<usize> = (0..self.faces.len())
            .filter(|i| !candidates.contains(i))
            .filter(ranges_check)
            .collect();
        rest.sort_by_key(|&i| {
            let f = &self.faces[i];
            (
                f.style != FaceStyle::Normal,
                (f.weight - 400.0).abs() as u32,
                f.stretch.abs_diff(5),
            )
        });
        candidates.extend(rest);
        let found = candidates.into_iter().find(|&i| self.covers(i, c));
        self.fallback
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(c, found);
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_roman_names() {
        let mac = |bytes: &'static [u8], encoding_id| name::Name {
            platform_id: name::PlatformId::Macintosh,
            encoding_id,
            language_id: 0,
            name_id: name_id::FAMILY,
            name: bytes,
        };
        assert_eq!(
            mac_roman_name(&mac(b"Helvetica", 0)).as_deref(),
            Some("Helvetica")
        );
        // 0x87 = á, 0xDB = €, 0xF0 = logotipo de Apple (zona de uso privado).
        assert_eq!(
            mac_roman_name(&mac(b"\x87 \xDB \xF0", 0)).as_deref(),
            Some("á € \u{f8ff}")
        );
        // Otra codificación Mac (1 = japonés) no se adivina.
        assert_eq!(mac_roman_name(&mac(b"x", 1)), None);
    }
}
