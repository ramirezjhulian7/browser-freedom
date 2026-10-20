"""Genera crates/bf-js/src/regexp/unicode_tables.rs: las tablas Unicode del motor de RegExp
(ECMA-262 §22.2.2.9 UnicodeMatchProperty / UnicodeMatchPropertyValue, §22.2.2.7.3
Canonicalize).

- General_Category (con las categorías agrupadas L, LC, M, N, P, S, Z, C y Cn calculada como
  los code points sin asignar de UnicodeData.txt) y sus alias (PropertyValueAliases.txt);
- Script y Script_Extensions (Scripts.txt, ScriptExtensions.txt) con sus alias;
- las propiedades binarias de la tabla "Binary Unicode property aliases" (PropList.txt,
  DerivedCoreProperties.txt, emoji-data.txt, DerivedBinaryProperties.txt,
  DerivedNormalizationProps.txt) con sus alias (PropertyAliases.txt);
- las propiedades de strings de la tabla "Binary Unicode properties of strings"
  (emoji-sequences.txt, emoji-zwj-sequences.txt);
- Simple Case Folding (CaseFolding.txt, estados C y S) para Canonicalize con `u`/`v`;
- toUppercase de un code unit (UnicodeData.txt + SpecialCasing.txt) para Canonicalize sin
  `u`/`v`, ya filtrado con las reglas del spec (longitud 1, no baja de 128 a ASCII).

La versión de la UCD es la de los tests generados de test262 en el commit pineado
(`test/built-ins/RegExp/property-escapes/generated`, "Unicode v17.0.0"): los datos están en
crates/bf-js/data/ucd-17.0.0/.

Uso: python -I tools/gen_unicode_props.py [directorio-ucd]
"""

import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.path.join(ROOT, "crates", "bf-js", "data", "ucd-17.0.0")
OUT = os.path.join(ROOT, "crates", "bf-js", "src", "regexp", "unicode_tables.rs")
MAX = 0x10FFFF

# ECMA-262 §22.2.2.9, tabla "Binary Unicode property aliases": nombre canónico → archivo.
BINARY = {
    "ASCII": None,
    "ASCII_Hex_Digit": "PropList.txt",
    "Alphabetic": "DerivedCoreProperties.txt",
    "Any": None,
    "Assigned": None,
    "Bidi_Control": "PropList.txt",
    "Bidi_Mirrored": "DerivedBinaryProperties.txt",
    "Case_Ignorable": "DerivedCoreProperties.txt",
    "Cased": "DerivedCoreProperties.txt",
    "Changes_When_Casefolded": "DerivedCoreProperties.txt",
    "Changes_When_Casemapped": "DerivedCoreProperties.txt",
    "Changes_When_Lowercased": "DerivedCoreProperties.txt",
    "Changes_When_NFKC_Casefolded": "DerivedNormalizationProps.txt",
    "Changes_When_Titlecased": "DerivedCoreProperties.txt",
    "Changes_When_Uppercased": "DerivedCoreProperties.txt",
    "Dash": "PropList.txt",
    "Default_Ignorable_Code_Point": "DerivedCoreProperties.txt",
    "Deprecated": "PropList.txt",
    "Diacritic": "PropList.txt",
    "Emoji": "emoji-data.txt",
    "Emoji_Component": "emoji-data.txt",
    "Emoji_Modifier": "emoji-data.txt",
    "Emoji_Modifier_Base": "emoji-data.txt",
    "Emoji_Presentation": "emoji-data.txt",
    "Extended_Pictographic": "emoji-data.txt",
    "Extender": "PropList.txt",
    "Grapheme_Base": "DerivedCoreProperties.txt",
    "Grapheme_Extend": "DerivedCoreProperties.txt",
    "Hex_Digit": "PropList.txt",
    "IDS_Binary_Operator": "PropList.txt",
    "IDS_Trinary_Operator": "PropList.txt",
    "ID_Continue": "DerivedCoreProperties.txt",
    "ID_Start": "DerivedCoreProperties.txt",
    "Ideographic": "PropList.txt",
    "Join_Control": "PropList.txt",
    "Logical_Order_Exception": "PropList.txt",
    "Lowercase": "DerivedCoreProperties.txt",
    "Math": "DerivedCoreProperties.txt",
    "Noncharacter_Code_Point": "PropList.txt",
    "Pattern_Syntax": "PropList.txt",
    "Pattern_White_Space": "PropList.txt",
    "Quotation_Mark": "PropList.txt",
    "Radical": "PropList.txt",
    "Regional_Indicator": "PropList.txt",
    "Sentence_Terminal": "PropList.txt",
    "Soft_Dotted": "PropList.txt",
    "Terminal_Punctuation": "PropList.txt",
    "Unified_Ideograph": "PropList.txt",
    "Uppercase": "DerivedCoreProperties.txt",
    "Variation_Selector": "PropList.txt",
    "White_Space": "PropList.txt",
    "XID_Continue": "DerivedCoreProperties.txt",
    "XID_Start": "DerivedCoreProperties.txt",
}

# Tabla "Binary Unicode properties of strings".
STRING_PROPS = [
    "Basic_Emoji",
    "Emoji_Keycap_Sequence",
    "RGI_Emoji_Modifier_Sequence",
    "RGI_Emoji_Flag_Sequence",
    "RGI_Emoji_Tag_Sequence",
    "RGI_Emoji_ZWJ_Sequence",
    "RGI_Emoji",
]

GC_GROUPS = {
    "C": ["Cc", "Cf", "Cn", "Co", "Cs"],
    "L": ["Ll", "Lm", "Lo", "Lt", "Lu"],
    "LC": ["Ll", "Lt", "Lu"],
    "M": ["Mc", "Me", "Mn"],
    "N": ["Nd", "Nl", "No"],
    "P": ["Pc", "Pd", "Pe", "Pf", "Pi", "Po", "Ps"],
    "S": ["Sc", "Sk", "Sm", "So"],
    "Z": ["Zl", "Zp", "Zs"],
}


def data_lines(path):
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.split("#", 1)[0].strip()
            if line:
                yield [p.strip() for p in line.split(";")]


def parse_cps(field):
    if ".." in field:
        a, b = field.split("..")
        return int(a, 16), int(b, 16)
    v = int(field, 16)
    return v, v


def merge(rs):
    rs = sorted(rs)
    out = []
    for a, b in rs:
        if out and a <= out[-1][1] + 1:
            out[-1] = (out[-1][0], max(out[-1][1], b))
        else:
            out.append((a, b))
    return out


def union(*sets):
    allr = []
    for s in sets:
        allr.extend(s)
    return merge(allr)


def complement(rs):
    out = []
    prev = 0
    for a, b in merge(rs):
        if a > prev:
            out.append((prev, a - 1))
        prev = b + 1
    if prev <= MAX:
        out.append((prev, MAX))
    return out


def subtract(a, b):
    return intersect(a, complement(b))


def intersect(a, b):
    out = []
    i = j = 0
    while i < len(a) and j < len(b):
        lo = max(a[i][0], b[j][0])
        hi = min(a[i][1], b[j][1])
        if lo <= hi:
            out.append((lo, hi))
        if a[i][1] < b[j][1]:
            i += 1
        else:
            j += 1
    return out


def prop_ranges(data, fname, prop):
    out = []
    for f in data_lines(os.path.join(data, fname)):
        if len(f) >= 2 and f[1] == prop:
            out.append(parse_cps(f[0]))
    return merge(out)


def unicode_data(data):
    """Categoría general por code point (con los rangos First/Last) y mayúscula simple."""
    gc = {}
    upper = {}
    first = None
    for f in data_lines(os.path.join(data, "UnicodeData.txt")):
        cp = int(f[0], 16)
        name, cat = f[1], f[2]
        if name.endswith(", First>"):
            first = cp
            continue
        if name.endswith(", Last>"):
            gc.setdefault(cat, []).append((first, cp))
            continue
        gc.setdefault(cat, []).append((cp, cp))
        if f[12]:
            upper[cp] = int(f[12], 16)
    return {k: merge(v) for k, v in gc.items()}, upper


def value_aliases(data, prop):
    """Alias de los valores de `prop` en PropertyValueAliases.txt: [nombres] por valor."""
    out = []
    for f in data_lines(os.path.join(data, "PropertyValueAliases.txt")):
        if f[0] == prop:
            names = [n for n in f[1:] if n]
            out.append(names)
    return out


def property_aliases(data):
    out = {}
    for f in data_lines(os.path.join(data, "PropertyAliases.txt")):
        names = [n for n in f if n]
        for n in names:
            out[n] = names
    return out


def special_upper(data):
    """toUppercase incondicional de SpecialCasing.txt (los de una condición se ignoran)."""
    out = {}
    for f in data_lines(os.path.join(data, "SpecialCasing.txt")):
        if len(f) >= 5 and f[4]:
            continue
        cp = int(f[0], 16)
        out[cp] = [int(x, 16) for x in f[3].split()]
    return out


def rust_ranges(name, rs):
    lines = [f"static {name}: &[(u32, u32)] = &["]
    row = []
    for a, b in rs:
        row.append(f"(0x{a:X}, 0x{b:X})")
        if len(row) == 6:
            lines.append("    " + ", ".join(row) + ",")
            row = []
    if row:
        lines.append("    " + ", ".join(row) + ",")
    lines.append("];")
    return "\n".join(lines)


def rust_str(cps):
    return '"' + "".join(f"\\u{{{c:X}}}" for c in cps) + '"'


def ident(name):
    return "".join(c if c.isalnum() else "_" for c in name).upper()


def main():
    data = sys.argv[1] if len(sys.argv) > 1 else DATA
    out = []
    out.append("//! Tablas Unicode del motor de RegExp (UCD 17.0.0, la versión de los tests generados de")
    out.append("//! test262). Generado por `tools/gen_unicode_props.py`: no editar a mano.")
    out.append("")
    out.append('pub const UNICODE_VERSION: &str = "17.0.0";')
    out.append("")
    out.append("/// Rangos cerrados de code points, ordenados y disjuntos.")
    out.append("pub type Ranges = &'static [(u32, u32)];")
    out.append("")

    gc, upper = unicode_data(data)
    assigned = union(*gc.values())
    gc["Cn"] = complement(assigned)
    for g, parts in GC_GROUPS.items():
        gc[g] = union(*[gc.get(p, []) for p in parts])

    # General_Category.
    entries = []
    for names in value_aliases(data, "gc"):
        short = names[0]
        if short not in gc:
            raise SystemExit(f"gc sin datos: {short}")
        sname = "GC_" + ident(short)
        out.append(rust_ranges(sname, gc[short]))
        entries.append((names, sname))
    out.append("")
    out.append("/// General_Category: los nombres del valor (corto, largo y alias) y sus code points.")
    out.append("pub static GENERAL_CATEGORY: &[(&[&str], Ranges)] = &[")
    for names, sname in entries:
        out.append(f"    (&[{', '.join(repr_str(n) for n in names)}], {sname}),")
    out.append("];")
    out.append("")

    # Script y Script_Extensions.
    sc = {}
    for f in data_lines(os.path.join(data, "Scripts.txt")):
        sc.setdefault(f[1], []).append(parse_cps(f[0]))
    sc = {k: merge(v) for k, v in sc.items()}
    aliases = value_aliases(data, "sc")
    long_of = {}
    for names in aliases:
        for n in names:
            long_of[n] = names[1]
    listed_scx = []
    scx = {}
    for f in data_lines(os.path.join(data, "ScriptExtensions.txt")):
        r = parse_cps(f[0])
        listed_scx.append(r)
        for s in f[1].split():
            scx.setdefault(long_of[s], []).append(r)
    listed_scx = merge(listed_scx)
    all_sc = merge([r for v in sc.values() for r in v])
    sc["Unknown"] = complement(all_sc)
    entries = []
    for names in aliases:
        long = names[1]
        base = sc.get(long, [])
        ext = union(subtract(base, listed_scx), scx.get(long, []))
        a = "SC_" + ident(long)
        b = "SCX_" + ident(long)
        out.append(rust_ranges(a, base))
        out.append(rust_ranges(b, ext))
        entries.append((names, a, b))
    out.append("")
    out.append("/// Script: nombres del valor, code points de Script y de Script_Extensions.")
    out.append("pub static SCRIPT: &[(&[&str], Ranges, Ranges)] = &[")
    for names, a, b in entries:
        out.append(f"    (&[{', '.join(repr_str(n) for n in names)}], {a}, {b}),")
    out.append("];")
    out.append("")

    # Propiedades binarias.
    palias = property_aliases(data)
    entries = []
    for name, fname in BINARY.items():
        if name == "ASCII":
            rs = [(0, 0x7F)]
        elif name == "Any":
            rs = [(0, MAX)]
        elif name == "Assigned":
            rs = assigned
        else:
            rs = prop_ranges(data, fname, name)
            if not rs:
                raise SystemExit(f"propiedad sin datos: {name}")
        names = palias.get(name, [name])
        if name not in names:
            names = [name]
        sname = "BIN_" + ident(name)
        out.append(rust_ranges(sname, rs))
        entries.append((names, sname))
    out.append("")
    out.append("/// Propiedades binarias: nombres (canónico y alias) y code points.")
    out.append("pub static BINARY: &[(&[&str], Ranges)] = &[")
    for names, sname in entries:
        out.append(f"    (&[{', '.join(repr_str(n) for n in names)}], {sname}),")
    out.append("];")
    out.append("")

    # Propiedades de strings.
    singles = {p: [] for p in STRING_PROPS}
    strings = {p: [] for p in STRING_PROPS}
    for fname in ("emoji-sequences.txt", "emoji-zwj-sequences.txt"):
        for f in data_lines(os.path.join(data, fname)):
            prop = f[1]
            if prop not in singles:
                continue
            field = f[0]
            if " " in field:
                strings[prop].append([int(x, 16) for x in field.split()])
            else:
                singles[prop].append(parse_cps(field))
    for p in STRING_PROPS[:-1]:
        singles["RGI_Emoji"].extend(singles[p])
        strings["RGI_Emoji"].extend(strings[p])
    out.append("/// Propiedades de strings: nombre, code points sueltos y strings de más de uno.")
    out.append("pub static STRING_PROPERTIES: &[(&str, Ranges, &[&str])] = &[")
    for p in STRING_PROPS:
        rs = merge(singles[p])
        out.append(f'    ("{p}", &[')
        for a, b in rs:
            out.append(f"        (0x{a:X}, 0x{b:X}),")
        out.append("    ], &[")
        for s in sorted(set(tuple(s) for s in strings[p])):
            out.append(f"        {rust_str(s)},")
        out.append("    ]),")
    out.append("];")
    out.append("")

    # Simple Case Folding.
    scf = []
    for f in data_lines(os.path.join(data, "CaseFolding.txt")):
        if f[1] in ("C", "S"):
            scf.append((int(f[0], 16), int(f[2], 16)))
    scf.sort()
    out.append("/// Simple Case Folding (CaseFolding.txt, estados C y S): (code point, scf).")
    out.append("pub static SIMPLE_CASE_FOLDING: &[(u32, u32)] = &[")
    row = []
    for a, b in scf:
        row.append(f"(0x{a:X}, 0x{b:X})")
        if len(row) == 6:
            out.append("    " + ", ".join(row) + ",")
            row = []
    if row:
        out.append("    " + ", ".join(row) + ",")
    out.append("];")
    out.append("")

    # Canonicalize sin `u`/`v` (§22.2.2.7.3 pasos 4–11).
    special = special_upper(data)
    canon = []
    for cp in range(0x10000):
        if 0xD800 <= cp <= 0xDFFF:
            continue
        u = special.get(cp)
        if u is None:
            u = [upper.get(cp, cp)]
        if len(u) != 1 or u[0] > 0xFFFF:
            continue
        cu = u[0]
        if cu == cp:
            continue
        if cp >= 128 and cu < 128:
            continue
        canon.append((cp, cu))
    out.append("/// Canonicalize sin `u`/`v`: (code unit, toUppercase) cuando cambia.")
    out.append("pub static NON_UNICODE_UPPER: &[(u16, u16)] = &[")
    row = []
    for a, b in canon:
        row.append(f"(0x{a:X}, 0x{b:X})")
        if len(row) == 6:
            out.append("    " + ", ".join(row) + ",")
            row = []
    if row:
        out.append("    " + ", ".join(row) + ",")
    out.append("];")

    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(out) + "\n")
    print(f"escrito {OUT}")


def repr_str(s):
    return '"' + s + '"'


if __name__ == "__main__":
    main()
