"""Genera crates/bf-html/src/tokenizer/named_entities.rs a partir de crates/bf-html/data/entities.json.

Uso (desde la raíz del repositorio):

    python -I tools/gen_entities.py

La fuente es https://html.spec.whatwg.org/entities.json (tabla de named character references del
spec WHATWG HTML, sección 13.5).
"""

import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SOURCE = ROOT / "crates" / "bf-html" / "data" / "entities.json"
TARGET = ROOT / "crates" / "bf-html" / "src" / "tokenizer" / "named_entities.rs"


def rust_str(codepoints):
    out = []
    for cp in codepoints:
        if 0x20 <= cp <= 0x7E and chr(cp) not in ('"', "\\"):
            out.append(chr(cp))
        else:
            out.append("\\u{%x}" % cp)
    return '"' + "".join(out) + '"'


def main():
    entities = json.loads(SOURCE.read_text(encoding="utf-8"))
    rows = []
    for name, value in entities.items():
        if not name.startswith("&"):
            sys.exit(f"nombre inesperado: {name!r}")
        rows.append((name[1:], value["codepoints"]))
    # Los nombres son ASCII, así que el orden de str de Python coincide con el orden de bytes de Rust.
    rows.sort(key=lambda row: row[0])

    lines = [
        "// GENERADO por tools/gen_entities.py a partir de data/entities.json. No editar a mano.",
        "",
        "/// (nombre sin `&`, incluyendo `;` cuando lo lleva; texto resultante), ordenado por nombre.",
        "pub(crate) static NAMED_ENTITIES: &[(&str, &str)] = &[",
    ]
    for name, codepoints in rows:
        lines.append(f'    ("{name}", {rust_str(codepoints)}),')
    lines += [
        "];",
        "",
        "#[cfg(test)]",
        "mod tests {",
        "    use super::NAMED_ENTITIES;",
        "",
        "    #[test]",
        "    fn table_is_strictly_sorted() {",
        "        assert!(NAMED_ENTITIES.windows(2).all(|w| w[0].0 < w[1].0));",
        "    }",
        "",
        "    #[test]",
        "    fn table_has_all_entries() {",
        "        assert_eq!(NAMED_ENTITIES.len(), 2231);",
        "    }",
        "",
        "    #[test]",
        "    fn table_contains_known_entries() {",
        "        for name in [\"amp;\", \"amp\", \"notin;\"] {",
        "            assert!(",
        "                NAMED_ENTITIES",
        "                    .binary_search_by_key(&name, |&(n, _)| n)",
        "                    .is_ok(),",
        "                \"falta {name}\"",
        "            );",
        "        }",
        "    }",
        "}",
        "",
    ]
    TARGET.write_text("\n".join(lines), encoding="utf-8", newline="\n")
    print(f"{len(rows)} entidades -> {TARGET.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
