# Estado de WPT

Línea base en **macOS** del 2026-11-12 (`master` `8917a5f`; Apple M4 de 10 núcleos, Rust
1.100.0, máquina compartida con otros cinco ejecutores). Las tablas de Windows siguen más abajo
como histórico.

## Reftests en macOS (2026-11-12)

Medido con `REFTEST_GROUPS=<los 30 grupos de GROUPS> cargo test --release -p bf-paint --test
wpt_reftests_groups -- --ignored --nocapture --test-threads=4`, en 27 s. El harness usa un hilo
por núcleo sin importar `--test-threads`. Es el mismo harness que `wpt_reftests`, pero no toca
este archivo: `wpt_reftests` (y `wpt-reftests` sin filtro) lo reescriben entero con su tabla y
borran las secciones de abajo.

- Commit de WPT: `4ad507b28db0a636f528def9161d109fe11b93a3`.
- Condiciones: viewport 800×600, DPR 1, fondo blanco, fuentes del sistema (las de macOS) más Ahem (`tests/wpt/fonts/Ahem.ttf`, como si estuviera instalada) y `@font-face`.
- `.xht`/`.xhtml` se parsean como XML; el resto, como HTML.
- `needs-js`: tests de testharness.js, manuales o con `<script>` o atributos manejadores de eventos `on*` (en el test o en la referencia).
- `error`: pánico o recurso faltante.
- El porcentaje se calcula sobre pass + fail + error.
- La última columna es la última cifra de Windows de cada grupo, con su fecha (ver las secciones históricas). El total de Windows es de otra lista de grupos (17).

| Directorio | pass | fail | needs-js | error | total | % pass (de los ejecutables) | Windows (2026-MM-DD) |
|---|---:|---:|---:|---:|---:|---:|---:|
| css/CSS2/box-display | 80 | 6 | 53 | 0 | 139 | 93.0% | 81.4% (10-20) |
| css/CSS2/margin-padding-clear | 666 | 16 | 57 | 0 | 739 | 97.7% | 96.6% (10-20) |
| css/CSS2/normal-flow | 657 | 37 | 162 | 0 | 856 | 94.7% | 93.9% (10-20) |
| css/CSS2/floats | 77 | 23 | 25 | 0 | 125 | 77.0% | 77.0% (10-20) |
| css/CSS2/floats-clear | 190 | 21 | 36 | 0 | 247 | 90.0% | 88.2% (10-20) |
| css/CSS2/positioning | 495 | 18 | 65 | 0 | 578 | 96.5% | 89.5% (10-20) |
| css/CSS2/linebox | 184 | 7 | 53 | 0 | 244 | 96.3% | 92.7% (11-10) |
| css/CSS2/text | 357 | 23 | 183 | 0 | 563 | 93.9% | 94.0% (11-10) |
| css/CSS2/colors | 10 | 9 | 3 | 0 | 22 | 52.6% | 100.0% (10-20) |
| css/CSS2/borders | 498 | 6 | 258 | 0 | 762 | 98.8% | 98.8% (10-20) |
| css/css-backgrounds | 536 | 79 | 336 | 0 | 951 | 87.2% | 86.5% (11-12) |
| css/css-flexbox | 747 | 202 | 478 | 0 | 1427 | 78.7% | 76.3% (11-12) |
| css/selectors | 106 | 13 | 405 | 0 | 524 | 89.1% | 89.2% (10-20) |
| css/CSS2/tables | 238 | 12 | 889 | 0 | 1139 | 95.2% | 98.4% (10-20) |
| css/css-tables | 106 | 32 | 164 | 0 | 302 | 76.8% | 73.9% (10-20) |
| css/css-grid | 1135 | 432 | 913 | 0 | 2480 | 72.4% | 70.3% (11-12) |
| css/css-ui | 75 | 37 | 1097 | 0 | 1209 | 67.0% | 67.0% (10-20) |
| css/css-transforms | 577 | 131 | 196 | 0 | 904 | 81.5% | 78.7% (11-12) |
| svg | 324 | 127 | 971 | 0 | 1422 | 71.8% | 70.5% (11-12) |
| css/css-writing-modes | 854 | 250 | 215 | 0 | 1319 | 77.4% | 17.8% (11-10) |
| css/CSS2/bidi-text | 83 | 22 | 3 | 0 | 108 | 79.0% | 79.0% (11-10) |
| css/css-text | 811 | 560 | 491 | 0 | 1862 | 59.2% | 59.2% (11-10) |
| css/css-overflow | 329 | 176 | 402 | 0 | 907 | 65.1% | 62.6% (11-07) |
| css/css-position | 42 | 77 | 228 | 0 | 347 | 35.3% | 33.6% (11-07) |
| css/css-images | 280 | 125 | 104 | 0 | 509 | 69.1% | – |
| css/css-color | 233 | 63 | 81 | 0 | 377 | 78.7% | 78.4% (11-12) |
| css/css-values | 86 | 81 | 330 | 0 | 497 | 51.5% | 49.7% (11-12) |
| css/css-env | 0 | 0 | 11 | 0 | 11 | – | – |
| css/css-properties-values-api | 3 | 3 | 111 | 0 | 117 | 50.0% | 50.0% (11-12) |
| css/css-variables | 156 | 25 | 64 | 0 | 245 | 86.2% | 86.2% (11-12) |
| **Total** | 9935 | 2613 | 8384 | 0 | 20932 | 79.2% | 76.4% (17 grupos) |

Subconjuntos y un grupo fuera de `GROUPS`, de la misma corrida:

| Directorio | pass | fail | needs-js | % pass |
|---|---:|---:|---:|---:|
| css/css-position/sticky | 11 | 0 | 98 | 100.0% |
| css/css-images (`object-fit*`, `object-position*`) | 179 | 3 | 18 | 98.4% |
| css/css-backgrounds (`border-image*`) | 63 | 4 | 5 | 94.0% |
| css/css-grid/grid-lanes | 584 | 267 | 145 | 68.6% |
| css/css-sizing (no está en `GROUPS`; `REFTEST_GROUPS=css/css-sizing`) | 355 | 170 | 238 | 67.6% |

- `css/selectors/of-type-selectors.xhtml` pasa.
- `css/CSS2/colors` baja de 19/19 (Windows, 2026-10-20) a 10/19. Los 9 fallos son
  `color-applies-to-*`: el color es el correcto, pero la "T" de "Filler Text" queda unos
  píxeles corrida. El test parte el texto en dos celdas ("Filler" y "&nbsp;Text") y la
  referencia no; con la Times de macOS el ajuste entre el espacio y la "T" cambia. Es una
  diferencia de fuentes del sistema, no de colores.
- `css/CSS2/tables` baja de 246/4 a 238/12 (`table-anonymous-objects-079` a `086`, `-211`,
  `border-conflict-element-001d`/`e`, `column-visibility-004`); sin investigar si es por las
  fuentes del Mac o por un cambio de layout.

Los fallos quedan en `target/reftests-groups/<ruta>-{actual,expected,diff}.png` y el detalle
por test en `target/reftests-groups/results.tsv`.

## Windows: corrida completa de reftests del 2026-10-20 (histórico)

Generado entonces por `cargo test -p bf-paint --test wpt_reftests -- --ignored --nocapture`
(mismo commit de WPT y condiciones, con las fuentes de Windows).

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
| css/css-backgrounds | 532 | 82 | 336 | 1 | 951 | 86.5% |
| css/css-flexbox | 633 | 316 | 478 | 0 | 1427 | 66.7% |
| css/selectors | 107 | 13 | 404 | 0 | 524 | 89.2% |
| css/CSS2/tables | 246 | 4 | 889 | 0 | 1139 | 98.4% |
| css/css-tables | 102 | 36 | 164 | 0 | 302 | 73.9% |
| css/css-grid | 635 | 930 | 913 | 2 | 2480 | 40.5% |
| css/css-ui | 75 | 37 | 1097 | 0 | 1209 | 67.0% |
| svg (E7, 2026-11-12) | 318 | 133 | 971 | 0 | 1422 | 70.5% |
| **Total** | 5794 | 1785 | 6146 | 4 | 13729 | 76.4% |

La fila `svg` se midió aparte el 2026-11-12 en `army/e7` con el mismo commit de WPT
(`cargo test -p bf-paint --test wpt_reftests -- --ignored wpt_svg --nocapture`); las
filas `css/css-grid` y `css/css-flexbox` son de E5 (2026-11-06, ver abajo); la de
`css/css-backgrounds` es de E7 (2026-11-12, `border-image`; `REFTEST_GROUPS=css/css-backgrounds cargo
test -p bf-paint --test wpt_reftests_groups -- --ignored`) y el resto es del 2026-10-20.

Medidos aparte con filtro (`wpt-reftests <dir>`, 2026-11-07, rama `army/e12`; no entran en el
total de arriba hasta la próxima corrida completa). Desde esta medición los atributos `on*`
(`onload="…"`) cuentan como script (`needs-js`), igual que `<script>`:

| Directorio | pass | fail | needs-js | error | total | % pass (de los ejecutables) |
|---|---:|---:|---:|---:|---:|---:|
| css/css-overflow | 316 | 189 | 402 | 0 | 907 | 62.6% |
| css/css-position | 40 | 79 | 228 | 0 | 347 | 33.6% |
| css/css-position/sticky | 10 | 1 | 99 | 0 | 110 | 90.9% |
| css/css-images (`object-fit*`, `object-position*`) | 179 | 4 | 26 | 0 | 209 | 97.8% |

Los fallos se guardan en `target/reftests/<ruta>-{actual,expected,diff}.png` y el detalle por test en `target/reftests/results.tsv`.

## E9: bidi, cortes de línea y shaping por script (2026-11-10)

Medido con `REFTEST_GROUPS=css/CSS2/bidi-text,css/css-text,css/css-writing-modes,css/css-flexbox,css/CSS2/text,css/CSS2/linebox cargo test -p bf-paint --test wpt_reftests_groups -- --ignored --nocapture`
sobre `army/e9` con `master` `f61201f` integrado (mismo WPT y condiciones que la tabla de
arriba). La columna `master faeb997` es la línea base de E9 (2026-11-07). Los tres primeros
grupos aún no están en `GROUPS` de `reftest.rs` (E7 los añade en su rama).

| Directorio | master faeb997 | army/e9 | needs-js | error |
|---|---:|---:|---:|---:|
| css/CSS2/bidi-text | 56/105 (53.3 %) | 83/105 (79.0 %) | 3 | 0 |
| css/css-text | 465/1372 (33.9 %) | 812/1372 (59.2 %) | 490 | 0 |
| css/css-writing-modes | 136/1103 (12.3 %) | 196/1103 (17.8 %) | 201 | 0 |
| css/css-flexbox | 608/949 (64.1 %) | 634/949 (66.8 %) | 478 | 0 |
| css/CSS2/text | 350/381 (91.9 %) | 358/381 (94.0 %) | 182 | 0 |
| css/CSS2/linebox | 176/190 (92.1 %) | 177/190 (92.7 %) | 53 | 1 |

Las ganancias de `css-flexbox` vienen de E5 (integrado desde `master`), no de E9. Conformidad
Unicode (`cargo test -p bf-layout --test unicode_conformance`, UCD 17.0.0): BidiTest
770241/770241, BidiCharacterTest 91707/91707, LineBreakTest 19338/19338.

## E5: flexbox rtl, propiedades lógicas inline y lanes con subgrid (2026-11-12)

Medido con `REFTEST_GROUPS=css/css-flexbox,css/css-grid cargo test -p bf-paint --test wpt_reftests_groups -- --ignored --test-threads=4`
sobre `army/e5` con `master` `caba13e` integrado (mismo WPT y condiciones que la tabla de
arriba). La columna `master` es la medición de la rama sin los cambios de la ronda (2026-11-10,
`master` `f61201f`).

| Directorio | master | army/e5 | needs-js | error |
|---|---:|---:|---:|---:|
| css/css-flexbox | 633/949 (66.7 %) | 699/949 (73.7 %) | 478 | 0 |
| css/css-grid | 959/1567 (61.2 %) | 1054/1567 (67.3 %) | 913 | 2 |
| css/css-grid/grid-lanes | 425/851 (49.9 %) | 518/851 (60.9 %) | – | – |

Pasa de "pass" a "fail" solo `css/css-flexbox/overflow-top-left.html`: su parte con
`writing-mode` vertical pasaba por casualidad.

## E2: `color-mix()`, espacios de color, `env()`, `@property` y math functions (2026-11-12)

Medido con `REFTEST_GROUPS=css/css-color,css/css-values,css/css-env,css/css-properties-values-api,css/css-variables cargo test -p bf-paint --test wpt_reftests_groups -- --ignored --test-threads=4`
sobre `army/e2` (`master` `9a10f87`). La columna `master` es la misma rama sin los cambios de
bf-style de la ronda (mismo checkout de WPT y misma máquina). Los grupos entran en `GROUPS`.

| Directorio | master | army/e2 | needs-js | error |
|---|---:|---:|---:|---:|
| css/css-color | 144/296 (48.6 %) | 232/296 (78.4 %) | 81 | 0 |
| css/css-values | 83/167 (49.7 %) | 83/167 (49.7 %) | 330 | 0 |
| css/css-env | – | – | 11 | 0 |
| css/css-properties-values-api | 1/6 (16.7 %) | 3/6 (50.0 %) | 111 | 0 |
| css/css-variables | 156/181 (86.2 %) | 156/181 (86.2 %) | 64 | 0 |
| **Total** | 384/650 (59.1 %) | 474/650 (72.9 %) | 597 | 0 |

92 tests pasan de "fail" a "pass". Pasan de "pass" a "fail"
`css/css-color/color-mix-currentcolor-002.html` y
`color-mix-currentcolor-nested-for-color-property.html`: antes la prueba y la referencia eran
inválidas por igual (las dos pintaban el color de reserva); ahora la referencia es válida y la
prueba usa `currentcolor` dentro de `color-mix()`, que todavía no se admite. Lo que más falla en
`css-color`: sintaxis relativa (`relative-currentcolor-*`), `display-p3-linear`, imágenes con
perfil de color (`tagged-images-*`), `contrast-color()` y los `deprecated-sameas-*` (colores
del sistema obsoletos). Las math functions, `env()` y `@property` se miden sobre todo con
testharness (`needs-js`); sus pruebas propias están en `bf-style/tests/modern.rs`.

## E5: abspos en flex, `safe` y lanes (2026-11-12, ronda 4)

Mismo comando sobre `army/e5` con `master` `d6540e8`; "antes" es el FINAL `4762047`.

| Directorio | 4762047 | army/e5 | needs-js | error |
|---|---:|---:|---:|---:|
| css/css-flexbox | 699/949 (73.7 %) | 724/949 (76.3 %) | 478 | 0 |
| css/css-grid | 1054/1567 (67.3 %) | 1102/1567 (70.3 %) | 913 | 2 |
| css/css-grid/grid-lanes | 518/851 (60.9 %) | 565/851 (66.4 %) | – | – |

Pasan de "pass" a "fail" `css-flexbox/align-items-007` (los SVG de E7 se pintan y uno sin
`width`/`height` llega con 300×150 de tamaño natural), `flex-abspos-align-self-safe-outer-cb-003`
(tentative) y `grid-lanes-grid-item-self-baseline-002b` (escritura vertical).

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

- Namespaces arbitrarios en el DOM: `css/selectors/of-type-selectors.xhtml` ya pasa en el Mac
  (2026-11-12); falta el resto de `Namespace::Other` (`docs/roadmap.md`, 3.6).
- E5, 2026-11-06 (Windows): 851 de los 1567 tests ejecutables de `css/css-grid` son de
  `grid-lanes` (masonry); en el Mac (2026-11-12) pasan 584 de 851. `css/css-sizing` 67.6 % en
  el Mac (no está en `GROUPS`).
- `css/css-overflow` (E12, 2026-11-12, `army/e12`; antes 50.5%): con `text-overflow` y
  `overflow-clip-margin`, faltan `::scroll-marker`/`::scroll-button` (44), `text-overflow` en
  RTL/bidi, vertical, ruby, `<textarea>` y con fuentes CJK de `@font-face`, el recorte por eje
  del `<svg>` raíz, el `line-clamp` que depende de `transform` (13) y ocultar las absolutas
  tras el clamp point.
  `css/css-position/sticky`: casi todo es testharness o desplaza con script; falla
  `position-sticky-rtl` (scroll containers RTL, con el origen a la derecha).
- `border-image` existe (`crates/bf-paint/src/border_image.rs`): 63 de 67 tests ejecutables de
  `border-image*` pasan en el Mac. `transform` también (`css/css-transforms` 81.5 %); las 3D
  se aplanan.
- El ajuste vertical de contornos (`FontProvider::vertical_fit`) es una aproximación propia al
  hinting, no comportamiento de spec; se desactiva con `Fonts::set_vertical_fit(false)`.

## testharness

Medido en macOS el 2026-11-12 (`master` `8917a5f`) con
`target/release/wpt-testharness <directorios> --threads 4 --results …` (formato de
`cargo run --release -p bf-web --bin wpt-testharness -- --out docs/wpt-status.md`).

- Commit de WPT: `4ad507b28db0a636f528def9161d109fe11b93a3`; directorios: `html/dom`, `css/cssom`, `css/cssom-view`, `html/semantics/forms`, `html/webappapis`, `fetch/api`, `fetch/http-cache`, `encoding`, `shadow-dom`, `custom-elements`, `domparsing`, `css/css-transitions`, `css/css-animations`: 3015 documentos en 158 s. `html/syntax/parsing` aparte, archivo por archivo con un tope de 120 s: 65 documentos en 132 s (ver las notas).
- Cada `*.html` con `testharness.js`, `*.any.js` (variante `window`) y `*.window.js` corre en un agente nuevo: el parser con los scripts clásicos (inline y `src` desde `tests/wpt` montado en `http://web-platform.test:8000/`), el bucle de eventos con reloj virtual y un `testharnessreport.js` propio que pasa los resultados al runner. Límite: 10 s por documento (60 s con `<meta name=timeout content=long>` o `META: timeout=long`), un hilo por documento.
- `archivos OK`: el harness terminó con estado OK (no implica que pasen todos sus subtests). Detalle por subtest: `target/wpt-testharness/results.tsv`.

| Directorio | archivos | archivos OK | subtests | pass | fail | timeout | notrun | % pass |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| css/css-animations | 127 | 124 | 1325 | 555 | 764 | 2 | 4 | 41.9% |
| css/css-transitions | 125 | 111 | 3347 | 690 | 707 | 484 | 1466 | 20.6% |
| css/cssom | 261 | 251 | 2050 | 1673 | 377 | 0 | 0 | 81.6% |
| css/cssom-view | 229 | 211 | 2168 | 1225 | 854 | 10 | 79 | 56.5% |
| custom-elements | 44 | 42 | 1081 | 834 | 247 | 0 | 0 | 77.2% |
| custom-elements/form-associated | 22 | 17 | 59 | 56 | 1 | 1 | 1 | 94.9% |
| custom-elements/htmlconstructor | 2 | 2 | 20 | 4 | 16 | 0 | 0 | 20.0% |
| custom-elements/parser | 12 | 11 | 42 | 21 | 21 | 0 | 0 | 50.0% |
| custom-elements/reactions | 57 | 57 | 538 | 480 | 58 | 0 | 0 | 89.2% |
| custom-elements/registries | 39 | 33 | 2318 | 2069 | 249 | 0 | 0 | 89.3% |
| custom-elements/state | 5 | 5 | 28 | 22 | 6 | 0 | 0 | 78.6% |
| custom-elements/upgrading | 7 | 7 | 29 | 15 | 14 | 0 | 0 | 51.7% |
| domparsing | 46 | 43 | 1099 | 468 | 631 | 0 | 0 | 42.6% |
| domparsing/tentative | 35 | 25 | 984 | 29 | 947 | 0 | 8 | 2.9% |
| encoding | 82 | 78 | 23286 | 22712 | 406 | 168 | 0 | 97.5% |
| encoding/legacy-mb-japanese | 51 | 15 | 333544 | 129854 | 0 | 1 | 203689 | 38.9% |
| encoding/legacy-mb-korean | 26 | 4 | 239968 | 43272 | 0 | 0 | 196696 | 18.0% |
| encoding/legacy-mb-schinese | 6 | 6 | 1017 | 1017 | 0 | 0 | 0 | 100.0% |
| encoding/legacy-mb-tchinese | 23 | 6 | 193189 | 43521 | 0 | 0 | 149668 | 22.5% |
| encoding/streams | 35 | 31 | 337 | 327 | 10 | 0 | 0 | 97.0% |
| fetch/api | 371 | 361 | 5650 | 4700 | 883 | 62 | 5 | 83.2% |
| fetch/http-cache | 38 | 37 | 417 | 387 | 26 | 1 | 3 | 92.8% |
| html/dom | 225 | 211 | 64537 | 62713 | 1797 | 9 | 18 | 97.2% |
| html/semantics/forms | 514 | 381 | 4763 | 2138 | 2204 | 98 | 323 | 44.9% |
| html/syntax | 65 | 61 | 2742 | 2722 | 18 | 2 | 0 | 99.3% |
| html/webappapis | 344 | 285 | 1889 | 1476 | 340 | 48 | 25 | 78.1% |
| shadow-dom | 65 | 62 | 760 | 663 | 89 | 3 | 5 | 87.2% |
| shadow-dom/declarative | 56 | 27 | 7762 | 7710 | 50 | 2 | 0 | 99.3% |
| shadow-dom/focus | 37 | 15 | 109 | 38 | 27 | 22 | 22 | 34.9% |
| shadow-dom/focus-navigation | 57 | 5 | 45 | 2 | 3 | 22 | 18 | 4.4% |
| shadow-dom/leaktests | 4 | 3 | 16 | 13 | 2 | 1 | 0 | 81.2% |
| shadow-dom/reference-target | 16 | 14 | 6081 | 5957 | 121 | 1 | 2 | 98.0% |
| shadow-dom/untriaged | 54 | 53 | 251 | 234 | 16 | 1 | 0 | 93.2% |
| **Total** | 3080 | 2594 | 901451 | 337597 | 10884 | 938 | 552032 | 37.5% |

Las causas más frecuentes de los archivos sin estado OK:

- 294: `TIMEOUT `
- 30: `ERROR Unhandled rejection: Cannot read properties of undefined (reading 'description')`
- 29: `ERROR Uncaught TypeError: Cannot read properties of undefined (reading 'cp')`
- 19: `NORESULT `
- 11: `ERROR Uncaught ReferenceError: trustedTypes is not defined`
- 8: `ERROR Unhandled rejection: WebIDL2 is not defined`
- 8: `ERROR Unhandled rejection: document.getAnimations is not a function`
- 6: `ERROR Uncaught TypeError: Illegal constructor`

## Notas de testharness

- **Sin medir en el Mac en esta corrida**: `dom/*`, `url`, `console`,
  `html/semantics/scripting-1` y `html/semantics/interactive-elements` (sus filas siguen en la
  tabla histórica de Windows). `dom/nodes` y `dom/events` los mide E1: la última cifra del
  Mac es `dom/nodes` 82.9 % y `dom/events` 72.4 % (sobre `master` `5b9a230`), **pendiente de
  actualizar**.
- Subdirectorios del criterio del roadmap, sacados del TSV de la misma corrida:
  `fetch/api/basic` 839 / 939 (89.4 %), `html/webappapis/scripting/event-loops` 4 / 9 (44.4 %,
  8 documentos). Directorios enteros: `custom-elements/` 3096 / 3710 (83.5 %), `shadow-dom/`
  14617 / 15024 (97.3 %), `domparsing/` 497 / 2083 (23.9 %; sin `tentative/` 468 / 1099,
  42.6 %), `encoding/` sin `legacy-mb-*` 23039 / 23623 (97.5 %).
- (Resuelto: ver "E3, ronda 6" más abajo.) **3 documentos terminaban en `CRASH`**: un pánico `Gc liberado` en `crates/bf-js/src/heap.rs`
  (`Heap::get` con un `Gc` ya recolectado) en
  `custom-elements/form-associated/ElementInternals-setFormValue.html`,
  `html/webappapis/dynamic-markup-insertion/opening-the-input-stream/ignore-opens-during-unload.window.html`
  y `html/webappapis/scripting/processing-model-2/integration-with-the-javascript-job-queue/promise-job-detached-iframe.html`.
  Los dos últimos usan marcos que se descargan.
- (Resuelto: ver "E3, ronda 6" más abajo.) **`html/syntax/parsing/adoption-agency-reparenting-shadow-cycle.html` no terminaba**: el
  límite de 10 s no lo corta. Con todos los directorios en una sola corrida, su hilo siguió más de
  35 min de CPU dentro de `bf_web::bom::frames::sync_frames` → `load_document` → el
  tree builder (`adoption_agency`, `Document::shadow_including_root`), así que
  `html/syntax/parsing` se midió archivo por archivo con un tope externo de 120 s y ese
  documento queda fuera de la fila `html/syntax`. Fuera de él: 2722 / 2742 subtests
  (99.3 %), 17 `FAIL`, 1 `PRECONDITION_FAILED`, 2 `TIMEOUT`; `html5lib_url.html`,
  `html5lib_write.html` y `html5lib_write_single.html` terminan en `ERROR Cannot read
  properties of undefined (reading 'runType')` y `the-end.html` en `TIMEOUT`. El árbol de
  html5lib sin JS (`cargo test -p bf-html --test wpt_tree_construction`) sigue en 3900 pasan,
  0 fallan, 4 omitidos.
- Tabla histórica de Windows: sin `encoding/legacy-mb-*`, 145693 / 160297 subtests (90.9 %,
  2026-11-07, `army/e10`).
- `html/dom/reflection-*.html` (Windows, 2026-11-07): 61266 / 61302 subtests (99.9%); fallan solo `link.nonce` y
  `style.nonce` (36): el IDL `nonce` de HTMLOrSVGElement no refleja el atributo (guarda
  `[[CryptographicNonce]]`), y el test lo trata como string reflejado.
- `encoding/legacy-mb-*`: los decodificadores y encoders están todos (GBK/gb18030, Big5,
  EUC-JP, ISO-2022-JP, Shift_JIS, EUC-KR); lo que no pasa son los tests que decodifican por
  `<iframe>` o leen un envío de formulario a un iframe (sin navegación de iframes: `notrun`).
- Los tests que usan `test_driver` (WebDriver) terminan en `TIMEOUT`.
- `webmessaging/broadcastchannel` (corrida aparte, 2026-11-07, mismo commit de WPT, con
  `service-workers/service-worker/resources` en el sparse): 12 documentos, 52 / 66 subtests
  (78.8%). Los 14 restantes (`cross-origin`, `detached-iframe`, `opaque-origin`, `ordering`,
  `origin.window`, `sandbox`) crean `<iframe>` y esperan su `load`: el runner no carga
  marcos (`TIMEOUT`).
- `html/semantics/forms`: la *activation behavior* (envío con `submit`/`formdata`/`invalid`,
  reset, `label`) está; la mayoría de `form-submission-0` y `form-submission-target` leen el
  resultado del envío en un `<iframe>` con `target` (sin navegación de iframes: `notrun`).
  Faltan además `DataTransfer` (archivos), `HTMLInputElement.labels` y `attachShadow`.
- `html/semantics/interactive-elements`: `the-dialog-element` (sin `HTMLDialogElement`) es
  casi todo el directorio; en `details` falta la exclusividad por `name`.

## E3, ronda 6: `html/syntax/parsing` y `event-loops` (2026-11-12)

Medido en macOS sobre `army/e3` (`master` `d1148b8` más la ronda 6 de E3) con
`target/release/wpt-testharness <dir> --threads 3`.

| Directorio | documentos | archivos OK | subtests | pass | % pass |
|---|---:|---:|---:|---:|---:|
| `html/syntax/parsing` | 236 | 235 | 8166 | 8150 | 99.8 % (8155, 99.9 %, el 2026-11-13: ver `docs/roadmap.md`, 2.3) |
| `html/webappapis/scripting/event-loops` | 7 | 7 | 8 | 6 | 75.0 % |

Cambios del runner (`crates/bf-web/src/bin/wpt-testharness.rs`):
- **Variantes de los `.html`**: cada `<meta name="variant" content="?…">` es un documento con
  esa query, como en WPT. Antes `html5lib_url.html`, `html5lib_write.html` y
  `html5lib_write_single.html` corrían sin `?file=…` y daban `bad config` (3 subtests); ahora
  son 173 documentos y 5423 subtests, que pasan todos. También corren las variantes de otros
  directorios (`html/dom/elements/…/innertext-with-white-spaces.html`,
  `html/semantics/forms/textfieldselection/selection-not-application.html`…).
- **Tests manuales fuera** (`-manual` antes de la extensión): necesitan a una persona; antes
  terminaban en `TIMEOUT` (`task_microtask_ordering-manual.html`, por ejemplo).
- **Vigilante**: cada documento corre en su hilo y, si no termina en su límite más 10 s, el
  runner lo da por `NORESULT` y sigue (el hilo abandonado termina con el proceso). El límite de
  `run_doc` solo lo comprobaba el motor JS; un bucle en código Rust no lo veía.

Lo que arregló el motor en esta ronda: el ciclo del adoption agency a través de un shadow
root o del contenido de un template creado por script (el documento que no terminaba), los
scripts SVG, CR LF partidos entre dos `document.write`, las URL `blob:` en `<iframe>`, el
`load` del `<iframe>` tras `document.close()`, las excepciones de los scripts de un marco
reportadas en el marco (antes en la ventana que lo contiene: el harness del documento de test
terminaba en `ERROR`), el punto de control de microtasks antes de cada script del parser y
*flush autofocus candidates*.

Comparación con `master` `d1148b8` subtest por subtest en `html/dom`, `html/webappapis`,
`html/semantics`, `custom-elements`, `shadow-dom`, `dom`, `css/cssom` y `domparsing` (4482
documentos): **0 regresiones, 3416 subtests ganados**. Los tres `NodeList-static-length-getter-
tampered-*` (timeout largo) se agotaron una vez con cada binario según la carga de la máquina
(carga 197).

Quedan en `html/syntax/parsing` (16): custom elements que mueven nodos durante el parseo (5:
`custom-element-reparenting*`), foster parenting (4), adoption agency entre documentos (4,
uno `PRECONDITION_FAILED` tentative) y "the end" (`the-end.html`, `pageshow`, 2 `TIMEOUT`;
`DOMContentLoaded-defer.html`). En `event-loops`:
`new-scroll-event-dispatched-at-next-updating-rendering-time.html` (media queries y `scroll`
de un marco con su tamaño) y `raf-from-animation-event-serviced-in-same-rendering-update.html`
(`element.animate`).

Los tres `CRASH` (`Gc liberado`) de la línea base no se reproducen sobre `master`: E1 fijó
con `host_pin` los objetos de `html/dataset.rs::register` y `storage/idb.rs::install`, que se
creaban sin raíz al crear el realm de un marco. Para que el patrón no vuelva,
`bf_web::register_all` corre dentro de `Agent::rooting_scope` (`bf-js/src/vm/mod.rs`): todo lo
que se asigna dentro es raíz hasta que termina.

## Windows: testharness del 2026-11-07 al 2026-11-10 (histórico)

Tabla anterior (Windows), con los directorios por defecto del runner (2563 documentos en 88 s)
más las filas de `custom-elements*`, `domparsing*` y `shadow-dom*` (2026-11-10, `army/e1`) y
`encoding*` (2026-11-07, `army/e10`).

| Directorio | archivos | archivos OK | subtests | pass | fail | timeout | notrun | % pass |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| console | 12 | 12 | 29 | 12 | 17 | 0 | 0 | 41.4% |
| custom-elements | 44 | 26 | 1037 | 778 | 164 | 12 | 83 | 75.0% |
| custom-elements/form-associated | 22 | 16 | 114 | 49 | 8 | 3 | 54 | 43.0% |
| custom-elements/htmlconstructor | 2 | 0 | 20 | 0 | 0 | 2 | 18 | 0.0% |
| custom-elements/parser | 12 | 10 | 42 | 21 | 19 | 1 | 1 | 50.0% |
| custom-elements/reactions | 57 | 45 | 538 | 455 | 35 | 12 | 36 | 84.6% |
| custom-elements/registries | 39 | 28 | 2287 | 2068 | 212 | 4 | 3 | 90.4% |
| custom-elements/state | 5 | 5 | 28 | 12 | 16 | 0 | 0 | 42.9% |
| custom-elements/upgrading | 7 | 2 | 29 | 12 | 1 | 5 | 11 | 41.4% |
| dom | 11 | 11 | 142 | 122 | 20 | 0 | 0 | 85.9% |
| dom/abort | 6 | 5 | 37 | 26 | 10 | 1 | 0 | 70.3% |
| dom/collections | 12 | 12 | 60 | 55 | 5 | 0 | 0 | 91.7% |
| dom/events | 177 | 89 | 817 | 584 | 82 | 62 | 89 | 71.5% |
| dom/lists | 5 | 5 | 189 | 187 | 2 | 0 | 0 | 98.9% |
| dom/nodes | 359 | 299 | 9668 | 7872 | 1038 | 82 | 676 | 81.4% |
| dom/observable | 29 | 25 | 251 | 0 | 242 | 4 | 5 | 0.0% |
| dom/ranges | 68 | 61 | 41823 | 37394 | 253 | 0 | 4176 | 89.4% |
| dom/traversal | 18 | 17 | 1607 | 1573 | 33 | 0 | 1 | 97.9% |
| domparsing | 46 | 35 | 1069 | 233 | 676 | 8 | 152 | 21.8% |
| domparsing/tentative | 35 | 22 | 984 | 29 | 919 | 3 | 33 | 2.9% |
| encoding | 82 | 73 | 23441 | 22700 | 227 | 511 | 3 | 96.8% |
| encoding/legacy-mb-japanese | 51 | 15 | 333544 | 129825 | 29 | 1 | 203689 | 38.9% |
| encoding/legacy-mb-korean | 26 | 4 | 239968 | 43264 | 8 | 0 | 196696 | 18.0% |
| encoding/legacy-mb-schinese | 6 | 6 | 1017 | 1017 | 0 | 0 | 0 | 100.0% |
| encoding/legacy-mb-tchinese | 23 | 6 | 193189 | 43513 | 8 | 0 | 149668 | 22.5% |
| encoding/streams | 35 | 31 | 325 | 325 | 0 | 0 | 0 | 100.0% |
| html/dom | 223 | 158 | 64260 | 62278 | 1883 | 46 | 53 | 96.9% |
| html/semantics/forms | 514 | 393 | 4689 | 1978 | 1862 | 85 | 764 | 42.2% |
| html/semantics/interactive-elements | 114 | 68 | 418 | 49 | 171 | 33 | 165 | 11.7% |
| html/semantics/scripting-1 | 491 | 332 | 2711 | 1441 | 959 | 24 | 287 | 53.2% |
| html/webappapis | 321 | 198 | 1131 | 608 | 293 | 176 | 54 | 53.8% |
| shadow-dom | 65 | 61 | 760 | 661 | 90 | 4 | 5 | 87.0% |
| shadow-dom/declarative | 56 | 26 | 7762 | 7708 | 48 | 5 | 1 | 99.3% |
| shadow-dom/focus | 37 | 14 | 109 | 37 | 27 | 23 | 22 | 33.9% |
| shadow-dom/focus-navigation | 57 | 3 | 45 | 2 | 1 | 24 | 18 | 4.4% |
| shadow-dom/leaktests | 4 | 3 | 16 | 13 | 2 | 1 | 0 | 81.2% |
| shadow-dom/reference-target | 16 | 14 | 6081 | 5957 | 121 | 1 | 2 | 98.0% |
| shadow-dom/untriaged | 54 | 47 | 240 | 223 | 14 | 3 | 0 | 92.9% |
| url | 41 | 39 | 8699 | 8489 | 193 | 17 | 0 | 97.6% |
| **Total** | 3182 | 2216 | 949176 | 381570 | 9688 | 1153 | 556765 | 40.2% |

Las causas más frecuentes de los archivos sin estado OK:

- 643: `TIMEOUT `
- 34: `ERROR Uncaught TypeError: Cannot read properties of null (reading 'querySelectorAll')`
- 28: `NORESULT `
- 8: `ERROR Unhandled rejection: WebIDL2 is not defined`
- 7: `NORESULT tiempo agotado`
- 4: `ERROR Uncaught ReferenceError: unreachable is not defined`
- 2: `ERROR TypeError: Document.parseHTMLUnsafe is not a function`
- 2: `ERROR Unhandled rejection`
