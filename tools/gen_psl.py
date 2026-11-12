"""Genera crates/bf-infra/data/public_suffix_list.txt a partir de la Public Suffix List de
Mozilla (https://publicsuffix.org/list/public_suffix_list.dat).

El archivo generado tiene una regla por línea, ya en ASCII (cada etiqueta no ASCII pasa a
`xn--` + Punycode, RFC 3492), sin comentarios; la línea `!private` separa las reglas ICANN de
las privadas. `bf-net/src/psl.rs` lo incluye con `include_str!`.

Uso (desde la raíz del repositorio): python -I tools/gen_psl.py [archivo.dat]
Sin argumento, descarga la lista.
"""

import os
import sys
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, 'crates', 'bf-infra', 'data', 'public_suffix_list.txt')
URL = 'https://publicsuffix.org/list/public_suffix_list.dat'


def to_ascii(label):
    """Una etiqueta de la lista en ASCII. Las reglas ya vienen en minúsculas y NFC."""
    if label in ('*',) or label.isascii():
        return label.lower()
    return 'xn--' + label.encode('punycode').decode('ascii')


def convert(text):
    out = []
    private = False
    for line in text.splitlines():
        line = line.strip()
        if line.startswith('// ===BEGIN PRIVATE DOMAINS==='):
            private = True
            out.append('!private')
            continue
        if not line or line.startswith('//'):
            continue
        # La regla es lo que hay hasta el primer espacio.
        rule = line.split()[0]
        exception = rule.startswith('!')
        if exception:
            rule = rule[1:]
        ascii_rule = '.'.join(to_ascii(l) for l in rule.split('.'))
        out.append(('!' if exception else '') + ascii_rule)
    if not private:
        raise SystemExit('la lista no tiene la sección privada')
    return out


def main():
    if len(sys.argv) > 1:
        with open(sys.argv[1], encoding='utf-8') as f:
            text = f.read()
    else:
        with urllib.request.urlopen(URL) as r:
            text = r.read().decode('utf-8')
    rules = convert(text)
    with open(OUT, 'w', encoding='utf-8', newline='\n') as f:
        f.write('\n'.join(rules) + '\n')
    print(f'{OUT}: {len(rules)} reglas')


if __name__ == '__main__':
    main()
