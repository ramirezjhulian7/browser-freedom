"""Genera crates/bf-layout/src/unicode_tables.rs: las propiedades Unicode del texto
internacional del layout.

- Bidi_Class (extracted/DerivedBidiClass.txt, con sus líneas @missing) para UAX #9;
- Bidi_Paired_Bracket y Bidi_Paired_Bracket_Type (BidiBrackets.txt) para la regla N0;
- Line_Break (LineBreak.txt) para UAX #14, con East_Asian_Width F/W/H (EastAsianWidth.txt),
  Extended_Pictographic (emoji-data.txt), General_Category Pi/Pf/Cn (UnicodeData.txt), que usan
  las reglas LB15a, LB15b, LB19a, LB30 y LB30b;
- Vertical_Orientation (VerticalOrientation.txt) para `text-orientation: mixed` (UAX #50);
- Script (Scripts.txt) para el fallback de fuentes y el shaping por run de script.

Datos: UCD 17.0.0 en crates/bf-infra/data/ucd-17.0.0/ (UnicodeData.txt y emoji-data.txt de la
misma versión, en crates/bf-js/data/ucd-17.0.0/).

Uso: python -I tools/gen_unicode_text.py
"""

import os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.path.join(ROOT, "crates", "bf-infra", "data", "ucd-17.0.0")
JSDATA = os.path.join(ROOT, "crates", "bf-js", "data", "ucd-17.0.0")
OUT = os.path.join(ROOT, "crates", "bf-layout", "src", "unicode_tables.rs")
MAX = 0x10FFFF

BIDI_LONG = {
    "Left_To_Right": "L",
    "Right_To_Left": "R",
    "Arabic_Letter": "AL",
    "European_Terminator": "ET",
}
BIDI = ["L", "R", "AL", "EN", "ES", "ET", "AN", "CS", "NSM", "BN", "B", "S", "WS", "ON",
        "LRE", "LRO", "RLE", "RLO", "PDF", "LRI", "RLI", "FSI", "PDI"]


def lines(path):
    with open(path, encoding="utf-8") as f:
        for raw in f:
            yield raw.rstrip("\n")


def parse_range(s):
    s = s.strip()
    if ".." in s:
        a, b = s.split("..")
        return int(a, 16), int(b, 16)
    return int(s, 16), int(s, 16)


def load_property(path, missing_map=None):
    """Valores por code point: primero las líneas @missing (en orden), luego las explícitas."""
    values = [None] * (MAX + 1)
    missing = []
    explicit = []
    for line in lines(path):
        if line.startswith("# @missing:"):
            rng, val = line[len("# @missing:"):].split(";")
            missing.append((parse_range(rng), val.strip()))
            continue
        body = line.split("#")[0].strip()
        if not body:
            continue
        rng, val = body.split(";")[:2]
        explicit.append((parse_range(rng), val.strip()))
    for (a, b), v in missing + explicit:
        if missing_map:
            v = missing_map.get(v, v)
        for c in range(a, b + 1):
            values[c] = v
    return values


def ranges(values, keep=lambda v: True):
    out = []
    start = 0
    for c in range(1, MAX + 2):
        if c == MAX + 1 or values[c] != values[start]:
            if values[start] is not None and keep(values[start]):
                out.append((start, c - 1, values[start]))
            start = c
    return out


def emit_ranges(out, name, ty, rs, fmt):
    out.append(f"pub(crate) static {name}: &[(u32, u32, {ty})] = &[")
    for a, b, v in rs:
        out.append(f"    (0x{a:X}, 0x{b:X}, {fmt(v)}),")
    out.append("];")
    out.append("")


def main():
    out = []
    out.append("//! Propiedades Unicode del texto internacional (UCD 17.0.0).")
    out.append("//! Generado por `tools/gen_unicode_text.py`. No editar.")
    out.append("")
    out.append("#![allow(clippy::unreadable_literal)]")
    out.append("")
    out.append("use crate::bidi::BidiClass;")
    out.append("use crate::linebreak::LineBreak;")
    out.append("")

    # Bidi_Class.
    bidi = load_property(os.path.join(DATA, "DerivedBidiClass.txt"), BIDI_LONG)
    for v in set(bidi):
        assert v in BIDI, v
    emit_ranges(out, "BIDI_CLASS", "BidiClass",
                ranges(bidi, lambda v: v != "L"), lambda v: f"BidiClass::{v}")

    # Bidi brackets.
    out.append("/// (code point, Bidi_Paired_Bracket, ¿es de apertura?).")
    out.append("pub(crate) static BIDI_BRACKETS: &[(u32, u32, bool)] = &[")
    for line in lines(os.path.join(DATA, "BidiBrackets.txt")):
        body = line.split("#")[0].strip()
        if not body:
            continue
        cp, pair, kind = [x.strip() for x in body.split(";")]
        out.append(f"    (0x{cp}, 0x{pair}, {'true' if kind == 'o' else 'false'}),")
    out.append("];")
    out.append("")

    # Line_Break.
    lb = load_property(os.path.join(DATA, "LineBreak.txt"))
    classes = sorted(set(lb))
    out.append("/// Clases Line_Break presentes en la UCD.")
    out.append(f"pub(crate) const LINE_BREAK_CLASSES: usize = {len(classes)};")
    emit_ranges(out, "LINE_BREAK", "LineBreak",
                ranges(lb, lambda v: v != "XX"), lambda v: f"LineBreak::{v}")

    # Propiedades auxiliares de UAX #14 como bits: 1 = EA F/W/H, 2 = Extended_Pictographic,
    # 4 = gc Pi, 8 = gc Pf, 16 = gc Cn, 32 = gc Mn o Mc (para resolver SA en LB1).
    flags = [0] * (MAX + 1)
    eaw = load_property(os.path.join(DATA, "EastAsianWidth.txt"))
    for c in range(MAX + 1):
        if eaw[c] in ("F", "W", "H"):
            flags[c] |= 1
    for line in lines(os.path.join(JSDATA, "emoji-data.txt")):
        body = line.split("#")[0].strip()
        if not body:
            continue
        rng, val = [x.strip() for x in body.split(";")]
        if val == "Extended_Pictographic":
            a, b = parse_range(rng)
            for c in range(a, b + 1):
                flags[c] |= 2
    gc = ["Cn"] * (MAX + 1)
    first = None
    for line in lines(os.path.join(JSDATA, "UnicodeData.txt")):
        f = line.split(";")
        cp = int(f[0], 16)
        if f[1].endswith(", First>"):
            first = cp
            continue
        if f[1].endswith(", Last>"):
            for c in range(first, cp + 1):
                gc[c] = f[2]
            continue
        gc[cp] = f[2]
    for c in range(MAX + 1):
        if gc[c] == "Pi":
            flags[c] |= 4
        elif gc[c] == "Pf":
            flags[c] |= 8
        elif gc[c] == "Cn":
            flags[c] |= 16
        elif gc[c] in ("Mn", "Mc"):
            flags[c] |= 32
    emit_ranges(out, "LB_FLAGS", "u8", ranges(flags, lambda v: v != 0), str)

    # Vertical_Orientation: 0 = R, 1 = U, 2 = Tu, 3 = Tr.
    vo = load_property(os.path.join(DATA, "VerticalOrientation.txt"))
    vo_code = {"R": 0, "U": 1, "Tu": 2, "Tr": 3}
    emit_ranges(out, "VERTICAL_ORIENTATION", "u8",
                ranges(vo, lambda v: v != "R"), lambda v: str(vo_code[v]))

    # Script: índice en SCRIPT_NAMES (0 = Unknown).
    sc = load_property(os.path.join(DATA, "Scripts.txt"))
    names = ["Unknown"] + sorted(set(v for v in sc if v != "Unknown"))
    index = {n: i for i, n in enumerate(names)}
    out.append("/// Nombres de los scripts (valores largos de la UCD); el índice 0 es `Unknown`.")
    out.append("pub(crate) static SCRIPT_NAMES: &[&str] = &[")
    for n in names:
        out.append(f"    \"{n}\",")
    out.append("];")
    out.append("")
    emit_ranges(out, "SCRIPT", "u8",
                ranges(sc, lambda v: v != "Unknown"), lambda v: str(index[v]))

    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(out))
    print(f"escrito {OUT}; clases Line_Break: {' '.join(classes)}")


if __name__ == "__main__":
    main()
