# Estado de los reftests de WPT

Generado por `cargo test -p bf-paint --test wpt_reftests -- --ignored --nocapture`.

- Fecha: 2026-10-20
- Commit de WPT: `4ad507b28db0a636f528def9161d109fe11b93a3`
- Condiciones: viewport 800×600, DPR 1, fondo blanco, fuentes del sistema más Ahem (`tests/wpt/fonts/Ahem.ttf`, como si estuviera instalada) y `@font-face`.
- `.xht`/`.xhtml` se parsean como XML; el resto, como HTML.
- `needs-js`: tests de testharness.js, manuales o con `<script>` o atributos manejadores de eventos `on*` (en el test o en la referencia).
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

- Namespaces arbitrarios en el DOM (`Namespace::Other` con la URI): hoy el parser XML mapea
  los namespaces desconocidos o ausentes a HTML, así que `css/selectors/of-type-selectors.xhtml`
  no puede pasar.
- E5, 2026-11-06 (filas `css/css-grid` y `css/css-flexbox` medidas ese día en `army/e5`):
  851 de los 1567 tests ejecutables de `css/css-grid` son de `grid-lanes` (masonry, 101 pasan);
  sin ellos pasan 534 de 716 (74.6%), y `css/css-grid/subgrid` 56 de 88. `css/css-sizing` (no
  está en `GROUPS`): 338 pass, 188 fail, 237 needs-js (64.3%).
- `css/css-overflow` (E12, 2026-11-12, `army/e12`; antes 50.5%): con `text-overflow` y
  `overflow-clip-margin`, faltan `::scroll-marker`/`::scroll-button` (44), `text-overflow` en
  RTL/bidi, vertical, ruby, `<textarea>` y con fuentes CJK de `@font-face`, el recorte por eje
  del `<svg>` raíz, el `line-clamp` que depende de `transform` (13) y ocultar las absolutas
  tras el clamp point.
  `css/css-position/sticky`: casi todo es testharness o desplaza con script; falla
  `position-sticky-rtl` (scroll containers RTL, con el origen a la derecha).
- `border-image` no existe todavía (unos 50 fallos de `css/css-backgrounds`), ni `transform`.
- El ajuste vertical de contornos (`FontProvider::vertical_fit`) es una aproximación propia al
  hinting, no comportamiento de spec; se desactiva con `Fonts::set_vertical_fit(false)`.

## testharness

Generado por `cargo run --release -p bf-web --bin wpt-testharness -- --out docs/wpt-status.md`.

- Commit de WPT: `4ad507b28db0a636f528def9161d109fe11b93a3`; directorios: `dom`, `html/dom`, `html/semantics/scripting-1`, `html/semantics/forms`, `html/semantics/interactive-elements`, `html/webappapis`, `url`, `encoding`, `console`; 2563 documentos en 88 s.
- Cada `*.html` con `testharness.js`, `*.any.js` (variante `window`) y `*.window.js` corre en un agente nuevo: el parser con los scripts clásicos (inline y `src` desde `tests/wpt` montado en `http://web-platform.test:8000/`), el bucle de eventos con reloj virtual y un `testharnessreport.js` propio que pasa los resultados al runner. Límite: 10 s por documento (60 s con `<meta name=timeout content=long>` o `META: timeout=long`), un hilo por documento.
- `archivos OK`: el harness terminó con estado OK (no implica que pasen todos sus subtests). Detalle por subtest: `target/wpt-testharness/results.tsv`.

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

## Notas de testharness

- Filas `custom-elements*`, `domparsing*` y `shadow-dom*`: medición del 2026-11-10 (rama
  `army/e1` sobre `master` `f61201f`, `wpt-testharness custom-elements shadow-dom domparsing`).
  `custom-elements/` en total 3395 / 4095 (82.9%), `shadow-dom/` 14601 / 15013 (97.3%),
  `domparsing/` sin `tentative/` 233 / 1069 (21.8%).
- Sin `encoding/legacy-mb-*`: 145693 / 160297 subtests (90.9%).
- Las filas `encoding*` son de una medición del 2026-11-07 (rama `army/e10`, runner actual con
  las variantes en workers de los `.any.js`, de ahí más archivos); el resto, de la tabla anterior.
- `html/dom/reflection-*.html`: 61266 / 61302 subtests (99.9%); fallan solo `link.nonce` y
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
