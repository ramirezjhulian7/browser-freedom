"""Genera crates/bf-infra/src/encoding_tables.rs a partir de los datos del WHATWG Encoding
Standard guardados en crates/bf-infra/data/:

- encodings.json: nombres y etiquetas (labels) de cada encoding.
- indexes.json: índices de los encodings single-byte (los multibyte quedan para el roadmap).

Uso (desde la raíz del repositorio): python -I tools/gen_encodings.py
Fuentes: https://encoding.spec.whatwg.org/encodings.json e indexes.json.
"""

import json
import os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.path.join(ROOT, 'crates', 'bf-infra', 'data')
OUT = os.path.join(ROOT, 'crates', 'bf-infra', 'src', 'encoding_tables.rs')


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


if __name__ == '__main__':
    main()
