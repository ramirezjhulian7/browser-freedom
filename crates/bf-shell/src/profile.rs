//! El perfil del usuario: la carpeta donde Freedom guarda el historial, los marcadores y las
//! descargas (`%LOCALAPPDATA%\Freedom` en Windows, `~/Library/Application Support/Freedom` en
//! macOS, `$XDG_DATA_HOME/freedom` o `~/.local/share/freedom` en los demás), y el formato de texto propio de esos archivos: un
//! registro por línea, campos separados por tabulador, con `\\`, `\t`, `\n` y `\r` escapados.
//! Las líneas que empiezan con `#` son comentarios.
//!
//! `FREEDOM_PROFILE` cambia la carpeta (pruebas, perfiles aparte). Las sesiones sin ventana
//! (`freedom session`) no tocan el perfil salvo que esa variable esté puesta
//! ([`set_ephemeral`]).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

static EPHEMERAL: AtomicBool = AtomicBool::new(false);

/// Sin `FREEDOM_PROFILE`, los almacenes quedan en memoria (no se lee ni escribe el perfil).
pub fn set_ephemeral() {
    EPHEMERAL.store(true, Ordering::Relaxed);
}

/// La carpeta del perfil, o `None` si no hay dónde guardar (los almacenes quedan en memoria).
pub fn dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("FREEDOM_PROFILE").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    if EPHEMERAL.load(Ordering::Relaxed) {
        return None;
    }
    let env = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    if cfg!(windows) {
        return env("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Freedom"));
    }
    if cfg!(target_os = "macos") {
        // La misma carpeta que `bf_net::data_dir` (cookies y HSTS).
        return env("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/Freedom"));
    }
    env("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .map(|d| d.join("freedom"))
}

/// La ruta de `name` dentro del perfil.
pub fn file(name: &str) -> Option<PathBuf> {
    dir().map(|d| d.join(name))
}

/// Escapa un campo: `\` → `\\`, tabulador → `\t`, saltos → `\n` y `\r`.
pub fn escape_field(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

/// Lo inverso de [`escape_field`]; un `\` suelto o desconocido queda tal cual.
pub fn unescape_field(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Un registro como línea (sin el salto final).
pub fn join_record(fields: &[&str]) -> String {
    fields
        .iter()
        .map(|f| escape_field(f))
        .collect::<Vec<_>>()
        .join("\t")
}

/// Los campos de una línea; `None` para las vacías y los comentarios.
pub fn split_record(line: &str) -> Option<Vec<String>> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.trim().is_empty() || line.starts_with('#') {
        return None;
    }
    Some(line.split('\t').map(unescape_field).collect())
}

/// Escribe `contents` en `path` sin dejarlo a medias: a un temporal y luego un rename.
pub fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

/// Agrega `line` (y un salto) al final de `path`.
pub fn append_line(path: &Path, line: &str) -> io::Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(format!("{line}\n").as_bytes())
}

/// Segundos desde la época Unix.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `(año, mes, día)` del día `days` desde la época (Howard Hinnant, `civil_from_days`).
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// `AAAA-MM-DD` del instante `secs` (UTC).
pub fn format_day(secs: u64) -> String {
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `HH:MM` del instante `secs` (UTC).
pub fn format_clock(secs: u64) -> String {
    let rem = secs % 86_400;
    format!("{:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_round_trip() {
        let raw = "a\tb\\c\nd\re";
        assert_eq!(escape_field(raw), "a\\tb\\\\c\\nd\\re");
        assert_eq!(unescape_field(&escape_field(raw)), raw);
        let line = join_record(&["1", "x\ty", ""]);
        assert_eq!(split_record(&line).unwrap(), vec!["1", "x\ty", ""]);
        assert_eq!(split_record("# comentario"), None);
        assert_eq!(split_record("  "), None);
    }

    #[test]
    fn dates() {
        assert_eq!(format_day(0), "1970-01-01");
        // 2026-11-06 12:34:56 UTC.
        assert_eq!(format_day(1_793_968_496), "2026-11-06");
        assert_eq!(format_clock(1_793_968_496), "12:34");
    }
}
