"""Copia asimetrix-arq-generator 1.0.0 y le aplica la identidad de Freedom.

La copia dibuja en el lienzo la marca de Freedom (el pájaro y el marco abierto de
`Icon::Brand` en ámbar, y «Freedom» en contornos de Montserrat) en lugar del lockup de
Asimetrix, usa la paleta de Freedom (tinta y ámbar) en el preset clásico y cambia los textos
de marca del visor. No toca la instalación original del plugin.

Uso (desde la raíz del repositorio; necesita fontTools):

    python tools/arquitectura/rebrand_arq.py <ruta del plugin> <carpeta de salida> <Montserrat-Regular.ttf>
"""

import os
import re
import shutil
import sys

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

SKILL, FORK, FONT = sys.argv[1], sys.argv[2], sys.argv[3]
REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

AMBER = '#F5A524'
WORD_FILL = '#8D94A6'
H = 149.17
WORD_X = 183.17
WORD_SIZE = 112
WORD_BASELINE = 116


def edit(rel, pairs):
    p = os.path.join(FORK, rel)
    s = open(p, encoding='utf-8').read()
    for old, new in pairs:
        assert s.count(old) == 1, (rel, old[:70], s.count(old))
        s = s.replace(old, new)
    open(p, 'w', encoding='utf-8', newline='\n').write(s)


def mark_path():
    """El polígono de `Icon::Brand` (grilla de 16) escalado a la altura del lockup."""
    icons = open(os.path.join(REPO, 'crates/bf-shell/src/window/icons.rs'), encoding='utf-8').read()
    body = icons[icons.index('const BRAND'):]
    body = body[:body.index('];')]
    pts = [(float(a), float(b)) for a, b in re.findall(r'\(([\d.]+), ([\d.]+)\)', body)]
    k = H / 16
    return 'M' + ' L'.join(f'{x * k:.2f},{y * k:.2f}' for x, y in pts) + ' Z'


def word_path():
    """«Freedom» en contornos de Montserrat (SIL OFL 1.1) y el ancho total del lockup."""
    font = TTFont(FONT)
    glyphs = font.getGlyphSet()
    cmap = font.getBestCmap()
    k = WORD_SIZE / font['head'].unitsPerEm
    x = 0.0
    out = []
    for ch in 'Freedom':
        name = cmap[ord(ch)]
        pen = SVGPathPen(glyphs, lambda v: f'{v:.1f}')
        glyphs[name].draw(TransformPen(pen, (k, 0, 0, -k, WORD_X + x, WORD_BASELINE)))
        out.append(pen.getCommands())
        x += font['hmtx'][name][0] * k
    return ' '.join(out), round(WORD_X + x + 6, 2)


if os.path.exists(FORK):
    shutil.rmtree(FORK)
shutil.copytree(SKILL, FORK)

# --- La marca del lienzo. ---
mark_d = mark_path()
word_d, width = word_path()
rel = 'renderers/shared/asimetrix-brand-mark.mjs'
p = os.path.join(FORK, rel)
s = open(p, encoding='utf-8').read()
a = s.index('export const ASIMETRIX_LOCKUP')
b = s.index('export const ASIMETRIX_ISOTYPE')
s = s[:a] + f'''// Lockup de Freedom (tools/arquitectura/rebrand_arq.py): el pájaro y el marco abierto de
// `Icon::Brand` en ámbar y la palabra «Freedom» en contornos de Montserrat (SIL OFL 1.1).
// Los colores van como atributos para que viajen igual en cada exportación.
export const ASIMETRIX_LOCKUP = Object.freeze({{
  source: 'crates/bf-shell/src/window/icons.rs (Icon::Brand)',
  viewBox: '0 0 {width} {H}',
  width: {width},
  height: {H},
  paths: Object.freeze(["{mark_d}"]),
  fillRule: null,
  markup: '<path fill="{AMBER}" d="{mark_d}"/>'
    + '<path fill="{WORD_FILL}" stroke="{WORD_FILL}" stroke-width="3" stroke-linejoin="round" d="{word_d}"/>',
}});

''' + s[b:]
open(p, 'w', encoding='utf-8', newline='\n').write(s)
edit('renderers/shared/asimetrix-brand.mjs', [(
    'scale(${round(markScale * 1000) / 1000})">${brandMarkPaths(ASIMETRIX_LOCKUP)}</g>',
    'scale(${round(markScale * 1000) / 1000})">${ASIMETRIX_LOCKUP.markup || brandMarkPaths(ASIMETRIX_LOCKUP)}</g>',
)])

# --- Paleta del preset clásico: tinta y ámbar (los colores por tipo de componente quedan). ---
edit('assets/template.html', [
    ('''    [data-theme="dark"] {
      --bg: #020617;
      --grid: #1e293b;
      --text: #ffffff;
      --text-muted: #94a3b8;
      --text-dim: #475569;''', '''    [data-theme="dark"] {
      --ax-accent: #F5A524;
      --bg: #0E1117;
      --grid: #1D222C;
      --text: #ECEEF3;
      --text-muted: #8D94A6;
      --text-dim: #4A5163;'''),
    ('''      --panel: rgba(15, 23, 42, 0.5);
      --panel-border: #1e293b;
      --lane-fill: rgba(15, 23, 42, 0.22);
      --lane-stroke: #334155;

      --arrow: #64748b;
      --arrow-emphasis: #34d399;''', '''      --panel: rgba(22, 26, 34, 0.72);
      --panel-border: #272D3A;
      --lane-fill: rgba(22, 26, 34, 0.35);
      --lane-stroke: #353C4C;

      --arrow: #6B7385;
      --arrow-emphasis: #F5A524;'''),
    ('''      --mask: #0f172a;

      --frontend-fill:    rgba(8, 51, 68, 0.4);''', '''      --mask: #161A22;

      --frontend-fill:    rgba(8, 51, 68, 0.4);'''),
    ('''      --toolbar-bg:       rgba(15, 23, 42, 0.8);
      --toolbar-border:   #334155;
      --toolbar-text:     #e2e8f0;
      --toolbar-hover:    rgba(15, 23, 42, 0.95);
      --toolbar-menu-bg:  #0f172a;
    }''', '''      --toolbar-bg:       rgba(22, 26, 34, 0.86);
      --toolbar-border:   #272D3A;
      --toolbar-text:     #ECEEF3;
      --toolbar-hover:    rgba(34, 40, 52, 0.95);
      --toolbar-menu-bg:  #161A22;
    }'''),
    ('''    [data-theme="light"] {
      --bg: #f8fafc;
      --grid: #e2e8f0;
      --text: #0f172a;
      --text-muted: #64748b;''', '''    [data-theme="light"] {
      --ax-accent: #9A5B00;
      --bg: #F7F8FA;
      --grid: #E8EAEF;
      --text: #171A21;
      --text-muted: #5E6574;'''),
    ('''      --arrow: #94a3b8;
      --arrow-emphasis: #059669;''', '''      --arrow: #8D94A6;
      --arrow-emphasis: #C27C0E;'''),
    ('    svg .ax-brand-band { fill: var(--text); opacity: 0.5; }',
     '    svg .ax-brand-band { opacity: 0.9; }'),
    ('<meta name="generator" content="asimetrix-arq-generator 1.0.0">',
     '<meta name="generator" content="asimetrix-arq-generator 1.0.0 (identidad Freedom)">'),
    ('''      border: 1px solid #22d3ee;
      border-radius: 0.16rem;
      background: rgba(34, 211, 238, 0.14);''', '''      border: 1px solid #F5A524;
      border-radius: 0.16rem;
      background: rgba(245, 165, 36, 0.16);'''),
])

# El acento de la interfaz del visor (título, vistas guiadas): ámbar en lugar del color de
# los nodos «frontend». Solo en el bloque de estilos de la interfaz, antes de los presets.
p = os.path.join(FORK, 'assets/template.html')
lines = open(p, encoding='utf-8').read().split('\n')
for i in range(549, 770):
    lines[i] = lines[i].replace('var(--frontend-stroke)', f'var(--ax-accent, {AMBER})')
    lines[i] = lines[i].replace(f'linear-gradient(90deg, var(--ax-accent, {AMBER}), var(--database-stroke))',
                                f'linear-gradient(90deg, var(--ax-accent, {AMBER}), #FFD27A)')
open(p, 'w', encoding='utf-8', newline='\n').write('\n'.join(lines))

# --- Textos de marca del visor. ---
p = os.path.join(FORK, 'renderers/shared/i18n.mjs')
s = open(p, encoding='utf-8').read()
s = s.replace('generated with Asimetrix Arq.', 'of Freedom.').replace('由 Asimetrix Arq 生成的', 'Freedom 的')
s = s.replace("'ASIMETRIX / PLANO 04'", "'FREEDOM / PLANO 04'").replace("'ASIMETRIX / 图版 04'", "'FREEDOM / 图版 04'")
s = s.replace("['ASIMETRIX · ", "['FREEDOM · ").replace(", 'ASIMETRIX · ", ", 'FREEDOM · ")
assert 'ASIMETRIX' not in s and 'Asimetrix' not in s
open(p, 'w', encoding='utf-8', newline='\n').write(s)
print('generador con la identidad de Freedom en', FORK)
