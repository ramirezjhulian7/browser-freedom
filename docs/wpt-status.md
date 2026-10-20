# Estado de los reftests de WPT

Generado por `cargo test -p bf-paint --test wpt_reftests -- --ignored --nocapture`.

- Fecha: 2026-10-19
- Commit de WPT: `4ad507b28db0a636f528def9161d109fe11b93a3`
- Condiciones: viewport 800×600, DPR 1, fondo blanco, fuentes del sistema más Ahem (`tests/wpt/fonts/Ahem.ttf`, como si estuviera instalada) y `@font-face`.
- `.xht`/`.xhtml` se parsean como XML; el resto, como HTML.
- `needs-js`: tests de testharness.js, manuales o con `<script>` (en el test o en la referencia).
- `error`: pánico o recurso faltante.
- El porcentaje se calcula sobre pass + fail + error.

| Directorio | pass | fail | needs-js | error | total | % pass (de los ejecutables) |
|---|---:|---:|---:|---:|---:|---:|
| css/CSS2/box-display | 65 | 21 | 53 | 0 | 139 | 75.6% |
| css/CSS2/margin-padding-clear | 575 | 107 | 57 | 0 | 739 | 84.3% |
| css/CSS2/normal-flow | 621 | 72 | 162 | 1 | 856 | 89.5% |
| css/CSS2/floats | 70 | 30 | 25 | 0 | 125 | 70.0% |
| css/CSS2/floats-clear | 152 | 59 | 36 | 0 | 247 | 72.0% |
| css/CSS2/positioning | 459 | 54 | 65 | 0 | 578 | 89.5% |
| css/CSS2/linebox | 167 | 23 | 53 | 1 | 244 | 87.4% |
| css/CSS2/text | 335 | 46 | 182 | 0 | 563 | 87.9% |
| css/CSS2/colors | 11 | 8 | 3 | 0 | 22 | 57.9% |
| css/CSS2/borders | 425 | 79 | 258 | 0 | 762 | 84.3% |
| css/css-backgrounds | 259 | 357 | 334 | 1 | 951 | 42.0% |
| css/css-flexbox | 565 | 384 | 478 | 0 | 1427 | 59.5% |
| css/selectors | 107 | 13 | 404 | 0 | 524 | 89.2% |
| **Total** | 3811 | 1253 | 2110 | 3 | 7177 | 75.2% |

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
- El ajuste vertical de contornos (`FontProvider::vertical_fit`) es una aproximación propia al
  hinting, no comportamiento de spec; se desactiva con `Fonts::set_vertical_fit(false)`.
