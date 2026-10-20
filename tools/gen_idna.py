"""Genera crates/bf-url/src/tables.rs a partir de los datos de Unicode en crates/bf-url/data/:

- IdnaMappingTable.txt (UTS #46): estado y mapeo de cada code point.
- DerivedBidiClass.txt (UCD): clase bidi, para la regla CheckBidi (RFC 5893).
- DerivedJoiningType.txt (UCD): tipo de unión, para CheckJoiners (RFC 5892 Apéndice A).

Uso (desde la raíz del repositorio): python -I tools/gen_idna.py
Los archivos de datos se descargan de https://www.unicode.org/Public/idna/latest/ y
https://www.unicode.org/Public/UCD/latest/ucd/extracted/.
"""

import os
import re

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.path.join(ROOT, 'crates', 'bf-url', 'data')
OUT = os.path.join(ROOT, 'crates', 'bf-url', 'src', 'tables.rs')

STATUS = {'valid': 0, 'mapped': 1, 'deviation': 2, 'disallowed': 3, 'ignored': 4}
BIDI = ['L', 'R', 'AL', 'AN', 'EN', 'ES', 'CS', 'ET', 'ON', 'BN', 'NSM']
BIDI_NAMES = {
    'Left_To_Right': 'L', 'Right_To_Left': 'R', 'Arabic_Letter': 'AL', 'Arabic_Number': 'AN',
    'European_Number': 'EN', 'European_Separator': 'ES', 'Common_Separator': 'CS',
    'European_Terminator': 'ET', 'Other_Neutral': 'ON', 'Boundary_Neutral': 'BN',
    'Nonspacing_Mark': 'NSM',
}
JOINING = {'D': 1, 'L': 2, 'R': 3, 'T': 4, 'C': 5}


def parse_range(field):
    field = field.strip()
    if '..' in field:
        a, b = field.split('..')
        return int(a, 16), int(b, 16)
    v = int(field, 16)
    return v, v


def version(path):
    with open(path, encoding='utf-8') as f:
        for line in f:
            m = re.search(r'Version: (\S+)|-(\d+\.\d+\.\d+)\.txt', line)
            if m:
                return m.group(1) or m.group(2)
    return '?'


def idna():
    entries = []
    mapping = []
    offsets = {}
    with open(os.path.join(DATA, 'IdnaMappingTable.txt'), encoding='utf-8') as f:
        for line in f:
            line = line.split('#', 1)[0].strip()
            if not line:
                continue
            fields = [x.strip() for x in line.split(';')]
            start, end = parse_range(fields[0])
            status = STATUS[fields[1]]
            target = ''
            if status == STATUS['mapped'] or (status == STATUS['deviation'] and len(fields) > 2
                                               and fields[2]):
                target = ''.join(chr(int(x, 16)) for x in fields[2].split()) if len(fields) > 2 else ''
            if status != STATUS['mapped']:
                target = ''
            if target not in offsets:
                offsets[target] = sum(len(x.encode('utf-8')) for x in mapping)
                mapping.append(target)
            off = offsets[target]
            prev = entries[-1] if entries else None
            if (prev and prev[1] + 1 == start and prev[2] == status and status != STATUS['mapped']):
                entries[-1] = (prev[0], end, status, 0, 0)
            else:
                entries.append((start, end, status, off if target else 0, len(target.encode('utf-8'))))
    return entries, ''.join(mapping)


def ranges(path, names, missing_default):
    """Rangos (inicio, fin, valor) con @missing como valores por defecto."""
    values = {}
    explicit = []
    missing = []
    with open(path, encoding='utf-8') as f:
        for line in f:
            if line.startswith('# @missing:'):
                field, value = [x.strip() for x in line[len('# @missing:'):].split(';')]
                missing.append((parse_range(field), value))
                continue
            line = line.split('#', 1)[0].strip()
            if not line:
                continue
            field, value = [x.strip() for x in line.split(';')[:2]]
            explicit.append((parse_range(field), value))
    # Los @missing se aplican en orden y después los explícitos; se guardan solo los valores
    # que interesan (el resto queda como "otro").
    spans = []
    for (a, b), v in missing + explicit:
        v = names.get(v, v)
        spans.append((a, b, v))
    # Resolver por barrido: un mapa de puntos de corte.
    points = sorted({a for a, _, _ in spans} | {b + 1 for _, b, _ in spans} | {0, 0x110000})
    result = []
    # Para eficiencia: pintar sobre una lista de segmentos ordenados.
    seg_val = {}
    import bisect
    idx = {p: i for i, p in enumerate(points)}
    vals = [missing_default] * (len(points) - 1)
    for a, b, v in spans:
        for i in range(idx[a], idx[b + 1]):
            vals[i] = v
    for i, v in enumerate(vals):
        a, b = points[i], points[i + 1] - 1
        if result and result[-1][2] == v and result[-1][1] + 1 == a:
            result[-1] = (result[-1][0], b, v)
        else:
            result.append((a, b, v))
    return result


def main():
    entries, mapping = idna()
    bidi = ranges(os.path.join(DATA, 'DerivedBidiClass.txt'), BIDI_NAMES, 'L')
    joining = ranges(os.path.join(DATA, 'DerivedJoiningType.txt'), {}, 'U')
    out = []
    out.append('// GENERADO por tools/gen_idna.py a partir de crates/bf-url/data/. No editar a mano.')
    out.append(f'// IdnaMappingTable {version(os.path.join(DATA, "IdnaMappingTable.txt"))}, '
               f'DerivedBidiClass {version(os.path.join(DATA, "DerivedBidiClass.txt"))}, '
               f'DerivedJoiningType {version(os.path.join(DATA, "DerivedJoiningType.txt"))}.')
    out.append('')
    out.append('/// (inicio, fin, estado, desplazamiento y longitud en bytes dentro de `IDNA_MAPPING`).')
    out.append('/// Estados: 0 valid, 1 mapped, 2 deviation, 3 disallowed, 4 ignored.')
    out.append('pub(crate) static IDNA: &[(u32, u32, u8, u32, u8)] = &[')
    for a, b, s, o, n in entries:
        out.append(f'    (0x{a:X}, 0x{b:X}, {s}, {o}, {n}),')
    out.append('];')
    out.append('')
    esc = ''.join(c if 0x20 <= ord(c) < 0x7F and c not in '"\\' else f'\\u{{{ord(c):x}}}'
                  for c in mapping)
    out.append('/// Destinos de los mapeos, concatenados.')
    out.append(f'pub(crate) static IDNA_MAPPING: &str = "{esc}";')
    out.append('')
    out.append('/// Clases bidi que usa RFC 5893 (el resto, 255).')
    out.append('pub(crate) static BIDI: &[(u32, u32, u8)] = &[')
    for a, b, v in bidi:
        code = BIDI.index(v) if v in BIDI else 255
        out.append(f'    (0x{a:X}, 0x{b:X}, {code}),')
    out.append('];')
    out.append('')
    out.append('/// Joining_Type: 1 D, 2 L, 3 R, 4 T, 5 C (U no se lista).')
    out.append('pub(crate) static JOINING: &[(u32, u32, u8)] = &[')
    for a, b, v in joining:
        if v in JOINING:
            out.append(f'    (0x{a:X}, 0x{b:X}, {JOINING[v]}),')
    out.append('];')
    with open(OUT, 'w', encoding='utf-8', newline='\n') as f:
        f.write('\n'.join(out) + '\n')
    print(f'{OUT}: {len(entries)} rangos IDNA, {len("".join(mapping).encode("utf-8"))} bytes de mapeo, '
          f'{len(bidi)} rangos bidi')


if __name__ == '__main__':
    main()
