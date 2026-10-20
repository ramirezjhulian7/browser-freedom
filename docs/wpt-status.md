# Estado de los reftests de WPT

Generado por `cargo test -p bf-paint --test wpt_reftests -- --ignored --nocapture`.

- Fecha: 2026-10-20
- Commit de WPT: `4ad507b28db0a636f528def9161d109fe11b93a3`
- Condiciones: viewport 800×600, DPR 1, fondo blanco, fuentes del sistema más Ahem (`tests/wpt/fonts/Ahem.ttf`, como si estuviera instalada) y `@font-face`.
- `.xht`/`.xhtml` se parsean como XML; el resto, como HTML.
- `needs-js`: tests de testharness.js, manuales o con `<script>` (en el test o en la referencia).
- `error`: pánico o recurso faltante.
- El porcentaje se calcula sobre pass + fail + error.

| Directorio | pass | fail | needs-js | error | total | % pass (de los ejecutables) |
|---|---:|---:|---:|---:|---:|---:|
| css/CSS2/box-display | 70 | 16 | 53 | 0 | 139 | 81.4% |
| css/CSS2/margin-padding-clear | 659 | 23 | 57 | 0 | 739 | 96.6% |
| css/CSS2/normal-flow | 652 | 42 | 162 | 0 | 856 | 93.9% |
| css/CSS2/floats | 77 | 23 | 25 | 0 | 125 | 77.0% |
| css/CSS2/floats-clear | 186 | 25 | 36 | 0 | 247 | 88.2% |
| css/CSS2/positioning | 459 | 54 | 65 | 0 | 578 | 89.5% |
| css/CSS2/linebox | 176 | 14 | 53 | 1 | 244 | 92.1% |
| css/CSS2/text | 350 | 31 | 182 | 0 | 563 | 91.9% |
| css/CSS2/colors | 19 | 0 | 3 | 0 | 22 | 100.0% |
| css/CSS2/borders | 498 | 6 | 258 | 0 | 762 | 98.8% |
| css/css-backgrounds | 321 | 295 | 334 | 1 | 951 | 52.0% |
| css/css-flexbox | 603 | 346 | 478 | 0 | 1427 | 63.5% |
| css/selectors | 107 | 13 | 404 | 0 | 524 | 89.2% |
| css/CSS2/tables | 246 | 4 | 889 | 0 | 1139 | 98.4% |
| css/css-tables | 102 | 36 | 164 | 0 | 302 | 73.9% |
| css/css-grid | 562 | 992 | 913 | 13 | 2480 | 35.9% |
| css/css-ui | 75 | 37 | 1097 | 0 | 1209 | 67.0% |
| **Total** | 5162 | 1957 | 5173 | 15 | 12307 | 72.4% |

Los fallos se guardan en `target/reftests/<ruta>-{actual,expected,diff}.png` y el detalle por test en `target/reftests/results.tsv`.

## Tests con expectativa dudosa

- `css/CSS2/text/letter-spacing-080.xht`: el test pide `letter-spacing: 6em` con `font: 20px`
  (120px), pero su referencia `letter-spacing-007-ref.xht` separa los cuadrados 96px (6em a
  16px). Falla por la referencia; no se toca.
- `css/CSS2/box-display/root-box-003.xht` (CSS 2.1: el fondo de `html` con `display: none`
  pinta el canvas) contradice `css/css-backgrounds/background-color-root-propagation-001.html`
  (CSS Backgrounds 3: no se propaga). Se sigue el spec vigente.
- `css/CSS2/margin-padding-clear/margin-collapse-038.xht` (CSS 2.2 §8.3.1 literal: el margen
  del último hijo atraviesa un padre recortado por `max-height`) contradice
  `css/CSS2/normal-flow/max-height-separates-margin.html`. Se sigue CSS 2.2.

## Pendientes conocidos

- Namespaces arbitrarios en el DOM (`Namespace::Other` con la URI): hoy el parser XML mapea
  los namespaces desconocidos o ausentes a HTML, así que `css/selectors/of-type-selectors.xhtml`
  no puede pasar.
- `css/css-grid`: 939 de los 1567 tests ejecutables son de `grid-lanes` (masonry) o `subgrid`,
  que no están implementados (roadmap). Sin ellos pasan 434 de 628 (69.1%).
- `border-image` no existe todavía (unos 50 fallos de `css/css-backgrounds`), ni `transform`.
- El ajuste vertical de contornos (`FontProvider::vertical_fit`) es una aproximación propia al
  hinting, no comportamiento de spec; se desactiva con `Fonts::set_vertical_fit(false)`.
