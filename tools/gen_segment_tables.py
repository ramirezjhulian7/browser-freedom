"""Genera crates/bf-js/src/builtins/intl/segment_tables.rs: las propiedades de UAX #29
(Unicode Text Segmentation) que usa Intl.Segmenter (ECMA-402 §18).

- Grapheme_Cluster_Break (auxiliary/GraphemeBreakProperty.txt);
- Word_Break (auxiliary/WordBreakProperty.txt);
- Sentence_Break (auxiliary/SentenceBreakProperty.txt);
- Indic_Conjunct_Break (DerivedCoreProperties.txt, `InCB`), para la regla GB9c;
- Extended_Pictographic (emoji-data.txt), para GB11 y WB3c.

Cada tabla es una lista ordenada de rangos (inicio, fin, valor) sin huecos internos que
compartan valor; lo que no está en la tabla vale 0 (`Other`/`None`). Los datos son los de
Unicode 17.0.0 en crates/bf-js/data/ucd-17.0.0/.

Uso: python -I -X utf8 tools/gen_segment_tables.py
"""

import os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.path.join(ROOT, "crates", "bf-js", "data", "ucd-17.0.0")
OUT = os.path.join(ROOT, "crates", "bf-js", "src", "builtins", "intl", "segment_tables.rs")

GCB = ["Other", "CR", "LF", "Control", "Extend", "ZWJ", "Regional_Indicator", "Prepend",
       "SpacingMark", "L", "V", "T", "LV", "LVT"]
WB = ["Other", "CR", "LF", "Newline", "Extend", "ZWJ", "Regional_Indicator", "Format",
      "Katakana", "Hebrew_Letter", "ALetter", "Single_Quote", "Double_Quote", "MidNumLet",
      "MidLetter", "MidNum", "Numeric", "ExtendNumLet", "WSegSpace"]
SB = ["Other", "CR", "LF", "Extend", "Sep", "Format", "Sp", "Lower", "Upper", "OLetter",
      "Numeric", "ATerm", "SContinue", "STerm", "Close"]
INCB = ["None", "Linker", "Consonant", "Extend"]


def parse(path, prop=None):
    """(inicio, fin, valor) de un archivo de la UCD; con `prop`, solo las líneas
    `cp ; prop ; valor` (o `cp ; prop` para una propiedad binaria)."""
    out = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.split("#", 1)[0].strip()
            if not line:
                continue
            fields = [x.strip() for x in line.split(";")]
            if prop is not None:
                if len(fields) < 2 or fields[1] != prop:
                    continue
                value = fields[2] if len(fields) > 2 else True
            else:
                value = fields[1]
            cps = fields[0]
            if ".." in cps:
                a, b = cps.split("..")
                lo, hi = int(a, 16), int(b, 16)
            else:
                lo = hi = int(cps, 16)
            out.append((lo, hi, value))
    return out


def merge(ranges):
    ranges = sorted(ranges)
    out = []
    for lo, hi, v in ranges:
        if out and out[-1][2] == v and out[-1][1] + 1 == lo:
            out[-1] = (out[-1][0], hi, v)
        else:
            assert not out or out[-1][1] < lo, (hex(lo), out[-1])
            out.append((lo, hi, v))
    return out


def table(name, doc, ranges, names):
    idx = {n: i for i, n in enumerate(names)}
    rows = merge([(lo, hi, idx[v]) for lo, hi, v in ranges])
    lines = [f"/// {doc}", f"pub const {name}: &[(u32, u32, u8)] = &["]
    for lo, hi, v in rows:
        lines.append(f"    (0x{lo:04X}, 0x{hi:04X}, {v}),")
    lines.append("];")
    return "\n".join(lines)


def consts(module, doc, names):
    lines = [f"/// {doc}", f"pub mod {module} {{"]
    for i, n in enumerate(names):
        lines.append(f"    pub const {n.upper()}: u8 = {i};")
    lines.append("}")
    return "\n".join(lines)


def main():
    aux = os.path.join(DATA, "auxiliary")
    gcb = parse(os.path.join(aux, "GraphemeBreakProperty.txt"))
    wb = parse(os.path.join(aux, "WordBreakProperty.txt"))
    sb = parse(os.path.join(aux, "SentenceBreakProperty.txt"))
    incb = parse(os.path.join(DATA, "DerivedCoreProperties.txt"), "InCB")
    ep = parse(os.path.join(DATA, "emoji-data.txt"), "Extended_Pictographic")
    ep_rows = merge([(lo, hi, 1) for lo, hi, _ in ep])
    parts = [
        "//! Generado por `tools/gen_segment_tables.py` desde la UCD 17.0.0 (UAX #29). No editar.",
        "",
        consts("gcb", "Valores de Grapheme_Cluster_Break.", GCB),
        "",
        consts("wb", "Valores de Word_Break.", WB),
        "",
        consts("sb", "Valores de Sentence_Break.", SB),
        "",
        consts("incb", "Valores de Indic_Conjunct_Break.", INCB),
        "",
        table("GCB", "Grapheme_Cluster_Break (GraphemeBreakProperty.txt).", gcb, GCB),
        "",
        table("WB", "Word_Break (WordBreakProperty.txt).", wb, WB),
        "",
        table("SB", "Sentence_Break (SentenceBreakProperty.txt).", sb, SB),
        "",
        table("INCB", "Indic_Conjunct_Break (DerivedCoreProperties.txt).", incb, INCB),
        "",
        "/// Extended_Pictographic (emoji-data.txt).",
        "pub const EXT_PICT: &[(u32, u32)] = &[",
    ]
    for lo, hi, _ in ep_rows:
        parts.append(f"    (0x{lo:04X}, 0x{hi:04X}),")
    parts.append("];")
    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(parts) + "\n")


if __name__ == "__main__":
    main()
