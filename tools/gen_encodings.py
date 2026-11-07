"""Genera crates/bf-infra/src/encoding_tables.rs a partir de los datos del WHATWG Encoding
Standard guardados en crates/bf-infra/data/:

- encodings.json: nombres y etiquetas (labels) de cada encoding.
- indexes.json: índices de los encodings single-byte y de los multibyte (jis0208, jis0212,
  euc-kr, gb18030, gb18030-ranges, big5, iso-2022-jp-katakana), que van a
  encoding_mb_tables.rs.

Uso (desde la raíz del repositorio): python -I tools/gen_encodings.py, y luego
rustfmt --edition 2024 crates/bf-infra/src/encoding_tables.rs crates/bf-infra/src/encoding_mb_tables.rs.
Fuentes: https://encoding.spec.whatwg.org/encodings.json e indexes.json.
"""

import json
import os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.path.join(ROOT, 'crates', 'bf-infra', 'data')
OUT = os.path.join(ROOT, 'crates', 'bf-infra', 'src', 'encoding_tables.rs')
OUT_MB = os.path.join(ROOT, 'crates', 'bf-infra', 'src', 'encoding_mb_tables.rs')

# (índice, constante, tipo de elemento)
MULTIBYTE = [
    ('jis0208', 'JIS0208', 'u16'),
    ('jis0212', 'JIS0212', 'u16'),
    ('euc-kr', 'EUC_KR', 'u16'),
    ('gb18030', 'GB18030', 'u16'),
    ('big5', 'BIG5', 'u32'),
    ('iso-2022-jp-katakana', 'ISO_2022_JP_KATAKANA', 'u16'),
]


def write_multibyte(indexes):
    out = ['// GENERADO por tools/gen_encodings.py a partir de crates/bf-infra/data/indexes.json.',
           '// No editar a mano.',
           '',
           '//! Índices multibyte del WHATWG Encoding Standard (§5): el code point de cada pointer,',
           '//! 0 = sin code point (null).',
           '']
    for key, const, ty in MULTIBYTE:
        table = [0 if x is None else x for x in indexes[key]]
        if ty == 'u16':
            assert max(table) <= 0xFFFF, key
        out.append(f'/// Index {key} ({len(table)} pointers).')
        out.append(f'pub(crate) static {const}: [{ty}; {len(table)}] = [')
        for i in range(0, len(table), 16):
            out.append('    ' + ', '.join(f'0x{x:X}' for x in table[i:i + 16]) + ',')
        out.append('];')
        out.append('')
    ranges = indexes['gb18030-ranges']
    out.append('/// Index gb18030 ranges: (pointer, code point), ordenado por pointer.')
    out.append(f'pub(crate) static GB18030_RANGES: [(u32, u32); {len(ranges)}] = [')
    for p, c in ranges:
        out.append(f'    ({p}, 0x{c:X}),')
    out.append('];')
    with open(OUT_MB, 'w', encoding='utf-8', newline='\n') as f:
        f.write('\n'.join(out) + '\n')
    print(f'{OUT_MB}: {len(MULTIBYTE) + 1} índices multibyte')


def main():
    with open(os.path.join(DATA, 'encodings.json'), encoding='utf-8') as f:
        groups = json.load(f)
    with open(os.path.join(DATA, 'indexes.json'), encoding='utf-8') as f:
        indexes = json.load(f)
    labels = []
    single = []
    for g in groups:
        for e in g['encodings']:
            name = e['name']
            for label in e['labels']:
                labels.append((label, name))
            if g['heading'] == 'Legacy single-byte encodings':
                index_name = 'iso-8859-8' if name == 'ISO-8859-8-I' else name.lower()
                table = indexes[index_name]
                assert len(table) == 128, name
                single.append((name, [0 if x is None else x for x in table]))
    labels.sort()
    out = ['// GENERADO por tools/gen_encodings.py a partir de crates/bf-infra/data/. No editar a mano.',
           '',
           '/// (label, nombre canónico), ordenado por label.',
           'pub(crate) static LABELS: &[(&str, &str)] = &[']
    for label, name in labels:
        out.append(f'    ({json.dumps(label)}, {json.dumps(name)}),')
    out.append('];')
    out.append('')
    out.append('/// Encodings single-byte: code point de los bytes 0x80..=0xFF (0 = sin mapeo).')
    out.append('pub(crate) static SINGLE_BYTE: &[(&str, [u16; 128])] = &[')
    for name, table in single:
        out.append(f'    ({json.dumps(name)}, [')
        for i in range(0, 128, 8):
            out.append('        ' + ', '.join(f'0x{x:04X}' for x in table[i:i + 8]) + ',')
        out.append('    ]),')
    out.append('];')
    with open(OUT, 'w', encoding='utf-8', newline='\n') as f:
        f.write('\n'.join(out) + '\n')
    print(f'{OUT}: {len(labels)} labels, {len(single)} encodings single-byte')
    write_multibyte(indexes)


if __name__ == '__main__':
    main()
