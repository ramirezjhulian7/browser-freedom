"""Genera crates/bf-js/src/lexer/unicode_tables.rs desde los datos de Unicode en
crates/bf-js/data/ (UCD "latest"):

- ID_Start, ID_Continue (DerivedCoreProperties.txt) y Space_Separator (UnicodeData.txt, Zs)
  para el lexer (ECMA-262 §12.2, §12.7);
- los nombres y alias de General_Category y Script (PropertyValueAliases.txt) y de las
  propiedades binarias de la tabla de ECMA-262 §22.2.2.9 (PropertyAliases.txt) para validar
  `\\p{...}` en los patrones de RegExp.

Uso: python -I tools/gen_unicode_id.py
"""

import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.path.join(ROOT, "crates", "bf-js", "data")
OUT = os.path.join(ROOT, "crates", "bf-js", "src", "lexer", "unicode_tables.rs")

# ECMA-262 §22.2.2.9, tabla "Binary Unicode property aliases" (nombres canónicos).
BINARY = [
    "ASCII", "ASCII_Hex_Digit", "Alphabetic", "Any", "Assigned", "Bidi_Control",
    "Bidi_Mirrored", "Case_Ignorable", "Cased", "Changes_When_Casefolded",
    "Changes_When_Casemapped", "Changes_When_Lowercased", "Changes_When_NFKC_Casefolded",
    "Changes_When_Titlecased", "Changes_When_Uppercased", "Dash",
    "Default_Ignorable_Code_Point", "Deprecated", "Diacritic", "Emoji", "Emoji_Component",
    "Emoji_Modifier", "Emoji_Modifier_Base", "Emoji_Presentation", "Extended_Pictographic",
    "Extender", "Grapheme_Base", "Grapheme_Extend", "Hex_Digit", "IDS_Binary_Operator",
    "IDS_Trinary_Operator", "ID_Continue", "ID_Start", "Ideographic", "Join_Control",
    "Logical_Order_Exception", "Lowercase", "Math", "Noncharacter_Code_Point",
    "Pattern_Syntax", "Pattern_White_Space", "Quotation_Mark", "Radical",
    "Regional_Indicator", "Sentence_Terminal", "Soft_Dotted", "Terminal_Punctuation",
    "Unified_Ideograph", "Uppercase", "Variation_Selector", "White_Space", "XID_Continue",
    "XID_Start",
]
# Sin alias en PropertyAliases.txt (no son propiedades de la UCD).
NO_ALIAS = {"ASCII", "Any", "Assigned"}


def ranges_of(path, prop):
    out = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.split("#", 1)[0].strip()
            if not line:
                continue
            cps, name = [p.strip() for p in line.split(";")[:2]]
            if name != prop:
                continue
            if ".." in cps:
                a, b = cps.split("..")
                out.append((int(a, 16), int(b, 16)))
            else:
                out.append((int(cps, 16), int(cps, 16)))
    return merge(out)


def merge(rs):
    rs.sort()
    out = []
    for a, b in rs:
        if out and a <= out[-1][1] + 1:
            out[-1] = (out[-1][0], max(out[-1][1], b))
        else:
            out.append((a, b))
    return out


def zs_ranges():
    out = []
    with open(os.path.join(DATA, "UnicodeData.txt"), encoding="utf-8") as f:
        for line in f:
            fields = line.split(";")
            if fields[2] == "Zs":
                cp = int(fields[0], 16)
                out.append((cp, cp))
    return merge(out)


def value_aliases(prop):
    names = set()
    with open(os.path.join(DATA, "PropertyValueAliases.txt"), encoding="utf-8") as f:
        for line in f:
            line = line.split("#", 1)[0].strip()
            if not line:
                continue
            fields = [p.strip() for p in line.split(";")]
            if fields[0] != prop:
                continue
            for v in fields[1:]:
                if v and v != "n/a":
                    names.add(v)
    return sorted(names)


def binary_aliases():
    aliases = {}
    with open(os.path.join(DATA, "PropertyAliases.txt"), encoding="utf-8") as f:
        for line in f:
            line = line.split("#", 1)[0].strip()
            if not line:
                continue
            fields = [p.strip() for p in line.split(";")]
            short, long = fields[0], fields[1]
            aliases[long] = [short] + fields[2:]
    names = set()
    for b in BINARY:
        names.add(b)
        if b in NO_ALIAS:
            continue
        if b not in aliases:
            sys.exit(f"propiedad binaria sin alias: {b}")
        names.update(a for a in aliases[b] if a)
    return sorted(names)


def table(name, rs):
    body = ",\n".join(f"    (0x{a:04X}, 0x{b:04X})" for a, b in rs)
    return f"pub static {name}: &[(u32, u32)] = &[\n{body},\n];\n"


def names(name, values):
    body = "\n".join(f'    "{v}",' for v in values)
    return f"pub static {name}: &[&str] = &[\n{body}\n];\n"


def main():
    core = os.path.join(DATA, "DerivedCoreProperties.txt")
    with open(core, encoding="utf-8") as f:
        version = re.search(r"DerivedCoreProperties-([\d.]+)\.txt", f.readline()).group(1)
    parts = [
        f"//! Generado por `tools/gen_unicode_id.py` desde la UCD {version}. No editar.\n",
        table("ID_START", ranges_of(core, "ID_Start")),
        table("ID_CONTINUE", ranges_of(core, "ID_Continue")),
        table("SPACE_SEPARATOR", zs_ranges()),
        names("GENERAL_CATEGORY_VALUES", value_aliases("gc")),
        names("SCRIPT_VALUES", value_aliases("sc")),
        names("BINARY_PROPERTIES", binary_aliases()),
    ]
    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(parts))
    print(f"escrito {OUT} (UCD {version})")


if __name__ == "__main__":
    main()
