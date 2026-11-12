# Roadmap

La fase 1 (motor de documentos estáticos) está cerrada. Este documento fija las fases
siguientes. Cada fase tiene criterios de aceptación medibles: una fase termina cuando todos
se cumplen en `cargo test --workspace` y en las suites indicadas, sin regresiones en las
anteriores.

Las reglas no cambian:
- motor propio;
- spec literal con la sección citada en el código;
- la suite oficial manda;
- `#![forbid(unsafe_code)]` (cualquier excepción, por ejemplo en el GC, se justifica por
  escrito en el archivo y se revisa aparte).

**Estado al 2026-11-12** (`master` `9a10f87`). Cada apartado lleva un bloque **Estado** con lo
hecho, lo parcial y lo que falta, la cifra medida y dónde vive. Las cifras salen de
[`wpt-status.md`](wpt-status.md), [`test262-status.md`](test262-status.md),
[`perf.md`](perf.md) y los `docs/pendiente/e*.md` (sección "Estado por bloque", al final);
cuando un criterio no se ha medido, se dice. Un criterio "cumplido" lo está en la medición
citada, no en una corrida completa de la fase.

## Punto de partida (fin de la fase 1)

| Suite | Resultado |
|---|---|
| html5lib-tests (tokenizer) | 7041 pasan, 0 fallan |
| WPT `html/syntax/parsing` | 3898 pasan, 0 fallan |
| css-parsing-tests | 253 pasan, 0 fallan |
| WPT `url/resources` | 1444 pasan, 0 fallan, 7 omitidos (multibyte) |
| WPT reftests (13 directorios) | 3811 / 5067 ejecutables (75.2%); 2110 `needs-js` |

---

## Fase 2: JavaScript y la plataforma web dinámica

Lo que falla hoy en sitios reales (funciones que faltan y errores por investigar) está en
[`faltantes-web.md`](faltantes-web.md).

### 2.1 Motor JavaScript `bf-js` (ECMAScript 2026)

- **Parser propio**: tokenizador y parser de ECMA-262 2026 (scripts y módulos, ASI, regex
  literals, template literals, clases, async/generadores, *early errors*) que produce un AST
  con posiciones.
- **Bytecode y VM propias**:
  - compilador AST → bytecode de registros;
  - entornos léxicos;
  - closures, excepciones, iteradores, generadores y async/await sobre una cola de jobs.
- **GC**: recolector propio con marcado y barrido (luego generacional), raíces explícitas y
  *handles* para los bindings. Sin conteo de referencias como mecanismo principal.
- **Builtins**:
  - `Object`, `Function`, `Array` (y typed arrays), `String` (UTF-16), `Number`/`BigInt`,
    `Math`;
  - `Date` (zona horaria del sistema), `RegExp` con motor propio (Unicode sets y named
    groups), `JSON`;
  - `Map`/`Set`/`WeakMap`/`WeakRef`, `Promise`, `Proxy`/`Reflect` e `Intl` mínimo.
- `DomString` pasa a WTF-8 (surrogates sueltos): desbloquea los 4 casos irrepresentables del
  tokenizador.
- **Atomics multi-agente** (pendiente): `$262.agent` completo (`start`, `broadcast`,
  `receiveBroadcast`, `report`, `getReport`, `leaving`) con un agente por hilo y los Shared
  Data Blocks compartidos entre hilos sin `unsafe` (`Arc<[AtomicU8]>`); con eso
  `Atomics.wait`/`notify`/`waitAsync` entre agentes (los 112 tests de test262 que usan
  `$262.agent`). Hoy hay un solo agente: `Atomics.wait` solo vence por timeout y `notify`
  despierta solo waiters de `waitAsync` del mismo agente.

**Aceptación**:
- **test262** como suite oficial, en el commit pineado en `tools/test262-commit.txt` y con
  harness propio (`cargo test -p bf-js --test test262 -- --nocapture`, resumen por directorio
  en `docs/test262-status.md`).
- Metas: `language/` ≥ 90%; `built-ins/` ≥ 80%; `intl402/` fuera de alcance salvo lo que
  use la plataforma.
- Ningún pánico en todo test262 (los errores son excepciones JS o fallos medidos).
- Un benchmark propio (`richards`, `deltablue` y `navier-stokes` portados) corre sin
  errores. Su tiempo queda registrado, sin meta todavía.

**Estado**:
- **Hecho**: parser (test262 solo parsing: 51 490 pasan, 0 fallan, 527 omitidos por
  propuestas; test262-parser-tests 3332, 0 fallan); bytecode, VM e intérprete
  (`bf-js/src/{compiler,vm}/`); GC propio de marcado y barrido con umbral adaptativo y pausas
  medidas (`perf.md`, sección GC); builtins de ES2026 completos en `built-ins/` (Temporal
  4605/4605, Explicit Resource Management, `Array.fromAsync`, `Uint8Array` base64/hex);
  Atomics multi-agente con `$262.agent` y `SharedArrayBuffer` real entre hilos
  (`bf-js/src/host/agents.rs`, `built-ins/Atomics` 380/380, E3); el `TODO(bf-dom)` de
  `DomString` ya no está en `bf-infra/src/lib.rs`.
- **Criterios**: test262 en ejecución (2026-11-07, `test262-status.md`): `language/` 23 183
  pasan, 11 fallan (**99.95 %**, meta ≥ 90 % cumplida); `built-ins/` 23 491 pasan, 67 fallan
  (**99.7 %**, meta ≥ 80 % cumplida); total 48 791 / 51 220 (**95.3 %**). Pánicos en todo
  test262: el estado no los cuenta aparte (la tabla tiene `failed` y `timeout`), así que el
  criterio **no consta medido**.
- **Parcial**: `intl402/` 30.5 % (1033 / 3382, 2026-11-07, antes de `Intl.Segmenter`,
  `DurationFormat`, `DisplayNames` y los datos de `de`/`pl` de E8, que no tienen corrida global
  nueva); faltan calendarios no ISO y zonas IANA de Temporal (fuera de alcance por ahora,
  `pendiente/e8.md`).
- **Falta**: `richards`, `deltablue` y `navier-stokes` no están portados. Hay 17 benchmarks
  propios en `crates/bf-js/bench/` con tiempos en `perf.md`: `fib(30)` 363 ms (E8,
  2026-11-12, primera vez bajo la meta interna de 400 ms). GC generacional o incremental.

### 2.2 Bindings DOM y WebIDL

- Generador de bindings desde los IDL de WHATWG/W3C (los `.idl` de `tests/wpt/interfaces`
  como fuente), con las conversiones de WebIDL: tipos, overloads, `[LegacyUnforgeable]`,
  iterables, promesas y excepciones `DOMException`.
- Interfaces:
  - DOM: `Node`, `Element`, `Document`, `Text`, `Event`/`EventTarget`, `MutationObserver`;
  - HTML: `HTMLElement` y subclases, `Window`, `Location`, `History`;
  - CSSOM: `getComputedStyle`, `CSSStyleDeclaration`, `element.style`;
  - CSSOM View: `offset*`, `getBoundingClientRect`, `scroll*`.
- Re-estilo y re-layout incrementales tras mutaciones (invalidación por nodo).

**Aceptación**:
- testharness.js corre en el motor, con el reporte capturado por el harness de WPT.
- WPT `dom/nodes` ≥ 85%, `dom/events` ≥ 80%, `html/dom` ≥ 60% y `css/cssom` ≥ 50%.
- `interfaces/*.idl`: el test `idlharness` de DOM y HTML pasa ≥ 80% de las aserciones.

**Estado**:
- **Hecho**: testharness.js corre en el motor (`bf-web/src/html/testharness.rs`, binario
  `wpt-testharness`; criterio cumplido). `html/dom` **96.9 %** (meta ≥ 60 % cumplida;
  reflexión 99.9 %); `css/cssom` **78.0 %** (E2, 2026-11-07; meta ≥ 50 % cumplida). Más allá
  del apartado: Shadow DOM (`shadow-dom/` 97.3 %), custom elements (`custom-elements/`
  82.9 %), `XMLSerializer` (E1, `bf-dom/src/{shadow,custom}.rs`, `bf-web/src/dom/`);
  `css/cssom-view` 50.0 % (E2, 2026-11-06).
- **Parcial**: `dom/nodes` **81.4 %** (meta 85 %; casi todo lo que falta de `*NS*` lee
  `<iframe>` con documentos XML); `dom/events` **71.5 %** (meta 80 %; E1 midió después 599
  pasan frente a 584); `domparsing/` 21.8 % (faltan los métodos posicionales y la API
  Sanitizer).
- **Sin medir**: `idlharness` de DOM y HTML (8 archivos terminan en `ERROR WebIDL2 is not
  defined`); re-estilo y re-layout incrementales (no hay cifra). Los bindings se escriben a
  mano por interfaz, no salen de un generador desde los `.idl`.

### 2.3 Event loop

- Event loop de HTML §8.1.7:
  - task sources y cola de microtasks;
  - *update the rendering* sincronizado con el repintado de la ventana;
  - `requestAnimationFrame`.
- Integración con el parser:
  - scripts *parser-blocking*, `async` y `defer`, y módulos;
  - `document.write` (la entrada del tokenizador pasa a ser de code points; ver los
    `TODO(bf-dom)`);
  - la cola de microtasks de `bf-dom/src/forms.rs` se reemplaza por la real
    (`TODO(event-loop)`).
- Eventos de usuario despachados desde `bf-shell`: `click`, `input`, `keydown`, `focus`,
  `scroll` y `submit`.
- Timers (`setTimeout`, `setInterval`) con clamping de HTML.
- `fetch()` y `XMLHttpRequest` sobre `bf-net`: CORS, *request mode* y *credentials*.

**Aceptación**:
- WPT `html/webappapis/scripting/event-loops` ≥ 70%, `html/syntax/parsing` sigue en 0
  fallos (ahora con los tests que necesitan `document.write`) y `fetch/api/basic` ≥ 70%.
- De los 2110 reftests `needs-js` de la fase 1, ≥ 60% pasa a ejecutarse (el resto son
  manuales o de otras APIs).
- Google, Wikipedia y Hacker News con su JavaScript cargan sin pánicos, y el buscador de
  Google funciona escribiendo en la caja y pulsando Intro.

**Estado**:
- **Hecho**: hilo por pestaña con motor JS, scripts del documento, `document.write`, módulos e
  import maps (`bf-web/src/bom/script.rs`), eventos de usuario despachados desde
  `bf-shell/src/window/events.rs`, `fetch()`/XHR con CORS, *mode* y *credentials*
  (`bf-web/src/net/cors.rs`, E2); `fetch()` por partes, streams, IndexedDB, WebSocket y
  EventSource (E2); workers, `SharedWorker`, `MessagePort` y `BroadcastChannel` entre agentes
  (E3, `bf-web/src/workers/`). El `TODO(bf-dom)` del tokenizador ya no está.
- **Criterios**: `fetch/api` 81.0 % entero (`fetch/api/cors` 86.4 %, E2) y
  `html/webappapis` 53.8 % entero; `event-loops` y `fetch/api/basic` **no se han medido por
  separado**. `html/syntax/parsing`: 3900 pasan, 0 fallan (README); no consta si incluye ya
  los tests de `document.write`.
- **Falta**: los reftests `needs-js` siguen sin ejecutarse (6144 en `wpt-status.md`): criterio
  sin cumplir. `TODO(event-loop)` sigue en `bf-dom/src/forms.rs`. *Update the rendering* y
  `requestAnimationFrame` sincronizados con el repintado: rAF existe en `bf-js` pero nadie lo
  llama (bucle de cuadros de E4 sin integrar, `pendiente/e4.md`). Google, Wikipedia y
  Hacker News sin pánicos: no hay una prueba formal; ver "Hallazgos de pruebas reales".

### 2.4 Interactividad de formularios

- Foco y navegación con `Tab`, edición de texto (cursor, selección, portapapeles) e
  `input`/`change`.
- Casillas y radios con grupos; `select` desplegable; `textarea` con scroll.
- Validación de restricciones y envío de formularios (`application/x-www-form-urlencoded` y
  `multipart/form-data`, GET y POST).
- `::placeholder` y `:focus-visible` (ver `TODO(forms)`).

**Aceptación**: WPT `html/semantics/forms` ≥ 60%. Buscar en Google desde su caja y enviar un
formulario POST a un servidor de prueba local (test en `bf-shell`).

**Estado**:
- **Parcial**: `html/semantics/forms` **42.2 %** (meta 60 %): la *activation behavior* (envío
  con `submit`/`formdata`/`invalid`, reset, `label`) está; la mayoría de `form-submission-*`
  leen el resultado en un `<iframe>` con `target` (`notrun`). Form-associated custom elements,
  `HTMLFormControlsCollection` y `RadioNodeList` (E1, `bf-web/src/html/forms.rs`); `textarea`
  y list box con scroll (E12). Envío POST desde la ventana (`LoadContext::post` en
  `bf-shell/src/load.rs`).
- **Falta**: `DataTransfer`, `HTMLInputElement.labels`; `TODO(forms)` sigue en
  `bf-layout/src/controls.rs`. El test POST contra un servidor local en `bf-shell` y la
  búsqueda en Google **no constan medidos**.

---

## Fase 3: completar CSS y el texto

### 3.1 Tablas

- CSS 2.2 §17 y css-tables-3:
  - modelo de filas, columnas y grupos; cajas anónimas de tabla;
  - layout automático y fijo; `border-collapse`;
  - `caption-side`; alineación vertical de celdas.

**Aceptación**: WPT `css/CSS2/tables` ≥ 75% y `css/css-tables` ≥ 50%. Hacker News se ve con
sus columnas (hoy cada celda cae en su propia línea).

**Estado**: **hecho**. `css/CSS2/tables` **98.4 %** y `css/css-tables` **73.9 %** (2026-10-20,
`wpt-status.md`; las dos metas cumplidas). Hacker News con columnas: no hay medición escrita,
pero las capturas de E7 (2026-11-06) muestran su página bien.

### 3.2 Grid

- css-grid-2:
  - líneas con nombre, áreas y `repeat()`;
  - `minmax()`, `fr`, auto-placement y alineación;
  - subgrid.

**Aceptación**: WPT `css/css-grid` ≥ 60%.

**Estado**: **hecho**. `css/css-grid` **67.3 %** (1054 / 1567, E5, 2026-11-12; meta cumplida;
la tabla principal de `wpt-status.md` aún muestra 40.5 % de la corrida completa anterior).
Subgrid 56/88; grid lanes (Grid 3) 518/851 (`bf-layout/src/grid.rs`, `grid/lanes.rs`). Falta:
baseline, `z-index` en ítems (necesita pintura), subgrid dentro de lanes.

### 3.3 Efectos visuales y animación

- `box-shadow` y `text-shadow`.
- Gradientes lineales, radiales y cónicos (`TODO(backgrounds)`).
- `border-image`; `background-attachment: fixed` y el clip `text`/`border-area`.
- Transforms 2D/3D (con stacking context y hit testing transformado), filtros y
  `mix-blend-mode`.
- Transitions y animations (`@keyframes` ya se parsea), sincronizadas con el event loop.

**Aceptación**:
- WPT `css/css-backgrounds` ≥ 70% (hoy 42.0%).
- `css/css-transforms` ≥ 50%, `css/css-transitions` y `css/css-animations` ≥ 40%.
- Los reftests de css-flexbox que fallan por `linear-gradient` en el fondo de control pasan.

**Estado**:
- **Parcial**: `css/css-backgrounds` **52.0 %** (meta 70 %). `box-shadow` y `text-shadow`
  existen (`bf-style/src/properties/mod.rs`); gradientes en `bf-style/src/properties/gradient.rs`
  (el `TODO(backgrounds)` de `bf-paint/src/builder.rs` ya solo habla de
  `background-attachment: fixed`). Filtros solo en SVG (`filter: url(#id)`, E7).
- **Falta**: `border-image` (unos 50 fallos de `css-backgrounds`); `background-attachment:
  fixed`; `transform`, transitions y animations: el código de E4
  (`bf-style/src/properties/{transform,animations}.rs`, `bf-style/src/animation.rs`) existe pero
  no está registrado en ningún `mod`, y el containing block de `transform` de E12 está inactivo
  (`TODO(E4)` en `bf-layout/src/positioned.rs`); `mix-blend-mode`. `css-transforms`,
  `css-transitions`, `css-animations` y los reftests de flexbox con `linear-gradient`: **sin
  medir**.

### 3.4 Texto internacional

- UAX #9 (bidi) en el formato inline, `direction`/`unicode-bidi` completos.
- `writing-mode` vertical y `text-orientation` (Writing Modes 4).
- UAX #14 (oportunidades de corte de línea), `hyphens`, `text-justify` y shaping complejo
  (árabe, índicos) con fallback de fuentes.

**Aceptación**:
- WPT `css/css-writing-modes` ≥ 40% y `css/CSS2/bidi-text` ≥ 70%.
- Los ~80 reftests de css-flexbox que fallan por `writing-mode`/rtl pasan ≥ 75%.

**Estado**:
- **Hecho**: UAX #9 y UAX #14 completos (`bf-layout/src/{bidi,linebreak}.rs`; conformidad UCD
  17.0.0: BidiTest 770241/770241, BidiCharacterTest 91707/91707, LineBreakTest 19338/19338);
  IFC con niveles bidi, `text-justify`, `text-align-last`, `line-break`, `hyphens` manual
  (`bf-layout/src/inline.rs`); shaping por script con fallback (`bf-font`). `css/CSS2/bidi-text`
  **79.0 %** (E9, 2026-11-10; meta ≥ 70 % cumplida); `css/css-text` 59.2 %. Flexbox con
  `direction: rtl` y propiedades lógicas inline (E5).
- **Parcial**: `css/css-writing-modes` **17.8 %** (meta 40 %): las propiedades `writing-mode` y
  `text-orientation` existen en `bf-style`, pero no hay layout vertical.
- **Falta**: Writing Modes 4 vertical, `hyphens: auto` con diccionario, propagación de
  `direction`/`writing-mode` del `<body>` (`pendiente/e9.md`). El criterio de los ~80 reftests
  de flexbox por `writing-mode`/rtl **no se ha medido aparte** (flexbox entero: 73.7 %, E5).

### 3.5 Encodings multibyte

- Decodificadores y encoders de WHATWG Encoding: GBK/gb18030, Big5, EUC-JP, ISO-2022-JP,
  Shift_JIS y EUC-KR, con los índices de `crates/bf-infra/data/indexes.json`.

**Aceptación**:
- WPT `encoding/` ≥ 90% de los tests sin JS (los de testharness, en la fase 2).
- Los 7 casos omitidos de `url/resources/percent-encoding.json` pasan.

**Estado**: **hecho** (E10). Decodificadores y encoders en streaming en
`bf-infra/src/encoding_codec.rs` con los índices generados (`encoding_mb_tables.rs`). WPT
`encoding/` **96.8 %** (testharness, 2026-11-07); sin `legacy-mb-*`, 90.9 % de los subtests;
los `legacy-mb-*` que fallan cargan un `<iframe>` o envían un formulario a uno. Los 7 casos de
`percent-encoding.json` pasan (16/16, 0 omitidos). El `TODO(roadmap)` de
`bf-infra/src/encoding.rs` ya no está.

### 3.6 Palabras clave de tamaño intrínseco y `Namespace::Other`

- `min-content`, `max-content`, `fit-content()` y `contain-intrinsic-size` en `width`,
  `height`, `min-*`, `max-*` y `flex-basis`.
- `aspect-ratio`.
- Namespaces arbitrarios en el DOM (`Namespace::Other` con la URI) y el parser XML.

**Aceptación**:
- WPT `css/css-sizing` ≥ 60%.
- Los ~50 reftests de css-flexbox que fallan por las palabras clave de tamaño pasan.
- `css/selectors/of-type-selectors.xhtml` pasa.

**Estado**:
- **Hecho**: palabras clave de tamaño, `aspect-ratio`, `contain` y `contain-intrinsic-size`
  (E5, `bf-layout/src/{block,flex,grid}.rs`). `css/css-sizing` **64.3 %** (338 / 526, E5,
  2026-11-06, runner propio: no está en `GROUPS`; meta cumplida).
- **Parcial**: `Namespace::Other` y `Null` existen en `bf-infra`; el parser XML ya trata un URI
  vacío como *null* (E1), pero sigue mapeando el namespace ausente a HTML en otros casos.
- **Falta**: `lookupNamespaceURI`/`isDefaultNamespace`, `@namespace` y `ns|E` en selectores,
  parser XML completo (bloques 2 y 3 de `pendiente/e10.md`). `of-type-selectors.xhtml` **no
  pasa**. Los ~50 reftests de flexbox por palabras clave: **sin medir aparte**.

---

## Fase 4: red, medios y documentos

### 4.1 HTTP/2 y caché

- HTTP/2 propio (RFC 9113: framing, HPACK, flow control y multiplexado) negociado por ALPN.
- Caché HTTP (RFC 9111) en memoria y disco: validación con `ETag`/`Last-Modified` y
  `Cache-Control`. Reemplaza al `CachedLoader` del shell (`TODO(http-cache)`).
- Persistencia de cookies y la Public Suffix List.

**Aceptación**:
- Tests propios contra un servidor h2 local (frames, HPACK con los vectores de RFC 7541,
  prioridades).
- WPT `fetch/http-cache` ≥ 70%.
- Recargar Wikipedia hace ≤ 2 peticiones de red nuevas.

**Estado**:
- **Hecho** (E6): HTTP/2 y HPACK propios (`bf-net/src/{http2,hpack}.rs`); tests contra un
  servidor h2 local en `bf-net/tests/h2.rs` (frames, `CONTINUATION`, control de flujo,
  `GOAWAY`/`REFUSED_STREAM`, ALPN) y los vectores C.1–C.6 de RFC 7541 en ambos sentidos; los
  frames `PRIORITY` se ignoran con `SETTINGS_NO_RFC7540_PRIORITIES` (RFC 9218): criterio
  cumplido. Caché en memoria y disco (`bf-net/src/cache.rs`) con variantes por `Vary`, rangos
  y parciales; WPT `fetch/http-cache` **90.4 %** (377 / 417, `army/e6`, 2026-11-12; meta
  cumplida). Cookies en disco y PSL (`bf-net/src/{cookies,psl}.rs`); el `TODO(http-cache)`
  del shell se quitó (decisión del planificador: `CachedLoader` queda como memoria del
  documento). Además: HSTS, Referrer-Policy, Happy Eyeballs.
- **Parcial**: recargar `es.wikipedia.org` (2026-11-10): 49 entradas, 41 de la caché sin red,
  6 validadas con 304 y **2 descargas** completas, es decir **8 peticiones** (las que llevan
  `max-age=0, must-revalidate`, que RFC 9111 obliga a validar). Una de las descargas
  (`Gadget-ReferenceTooltips.js`, `Vary: Cookie`) se arregló el 2026-11-12; la recarga no se
  ha vuelto a medir.
- **Aceptado, pendiente de integrar** (`4bb212d`, `76d4eca`): `immutable`,
  `stale-while-revalidate`, 6 conexiones por origen, coalescing h2, errores de red con causa,
  autenticación HTTP y caché de DNS (`pendiente/e6.md`).

### 4.2 Formatos de imagen

- SVG como documento propio (parser XML existente, layout SVG 2 básico: formas, paths,
  texto, `viewBox`, gradientes) usado en `<img>`, `background-image` y SVG en línea.
- Decodificadores propios de WebP (VP8/VP8L) y AVIF (AV1 intra), y ICO/BMP.
- Animación de GIF/APNG (`TODO(roadmap)` en `bf-image`).

**Aceptación**:
- WPT `svg/` reftests ≥ 40%.
- Los iconos (hoy cuadrados negros por `mask-image`) y el gráfico de torta de Wikipedia se ven.
- El logo SVG de Hacker News se ve.

**Estado**:
- **Hecho** (E7): `crates/bf-svg` y `bf-paint/src/images/svg.rs` (SVG en `<img>`,
  `background-image`, `mask-image` y en línea; `pattern`, `mask`, filtros, `textPath`,
  `<image>`, `foreignObject` básico, recursos externos); WPT `svg/` **70.5 %** (318 / 451,
  2026-11-12; meta ≥ 40 % cumplida). WebP (VP8, VP8L, `ALPH`), BMP e ICO/CUR propios
  (`bf-image/src/`); animación de GIF y APNG en la ventana. `mask-image`: los iconos de
  Wikipedia y el logo y las flechas de Hacker News se ven (capturas de E7, 2026-11-06); la torta
  de `Pie_chart` llega como PNG en miniatura.
- **Falta**: AVIF (diseño en `pendiente/e7.md`; queda el `TODO(roadmap)` de `bf-image/src/lib.rs`),
  WebP animado (solo el primer fotograma), `mix-blend-mode` en SVG, texto vertical y RTL en SVG.

### 4.3 Scroll y overflow

- Scroll containers reales: barras de desplazamiento, `overflow: auto|scroll` con su
  propio offset y `scrollbar-gutter`.
- `position: sticky` (`TODO(sticky)`) y `scroll-behavior`.
- `object-fit`/`object-position`, `<picture>`/`srcset` y la carga asíncrona e incremental de
  imágenes con re-layout.

**Aceptación**: WPT `css/css-overflow` ≥ 50% y `css/css-position/sticky` ≥ 50%. `<textarea>`
y los list box se desplazan con la rueda.

**Estado**:
- **Hecho** (E12): scroll containers con barras superpuestas, rueda, arrastre y teclado
  (`bf-layout/src/scroll.rs`, `bf-paint/src/scrollbars.rs`); `position: sticky`; `line-clamp`,
  `text-overflow` y `overflow-clip-margin`. `css/css-overflow` **61.8 %** (2026-11-12) y
  `css/css-position/sticky` **90.9 %** (10/11 ejecutables, 99 `needs-js`): metas cumplidas.
  `<textarea>` y los list box son scroll containers y la rueda los mueve (el test con la
  rueda no consta aparte).
- **Falta**: `object-fit`/`object-position`, `<picture>`/`srcset`/`sizes` y carga incremental
  de imágenes (sin empezar); `scroll-behavior: smooth` es instantáneo; `scroll-snap`,
  `overscroll-behavior`, scroll containers RTL.

---

## Fase 5: navegador de uso diario

- **Accesibilidad**: árbol de accesibilidad (HTML-AAM, ARIA 1.2) expuesto a UI Automation
  (Windows), AT-SPI (Linux) y NSAccessibility (macOS). Navegación completa por teclado y
  zoom de texto.
  - Aceptación: WPT `html-aam` y `wai-aria` ≥ 50%; un lector de pantalla lee Wikipedia.
- **Modelo de procesos y sandbox**:
  - un proceso de navegador y un proceso de contenido por sitio, con IPC propio
    (mensajes tipados y memoria compartida para los pixmaps);
  - sandbox del proceso de contenido (AppContainer en Windows, seccomp-bpf en Linux, Seatbelt
    en macOS);
  - origin isolation.

  Aceptación: un pánico en una página no cierra la ventana; un fuzzer de IPC no escapa del
  sandbox.
- **Seguridad de la plataforma**: CSP, mixed content, `Referrer-Policy`, HSTS, Permissions
  Policy y Safe Browsing opcional.
- **Interfaz**: pestañas, descargas, historial persistente, marcadores e
  impresión (`@page`).
- **Rendimiento**: GPU (`wgpu`) para composición y raster de capas, raster en tiles,
  estilo y layout incrementales y en paralelo.
  - Aceptación: 60 fps al desplazar Wikipedia; primera pintura de Wikipedia < 500 ms en
    release.

**Estado**:
- **Accesibilidad**: falta el árbol y su exposición al sistema; solo la reflexión ARIA en el DOM
  (`ARIAMixin`, E1). `html-aam` y `wai-aria`: sin medir.
- **Procesos y sandbox**: falta. Hoy cada pestaña es un hilo del mismo proceso
  (`bf-shell/src/window/worker.rs`); "un pánico no cierra la ventana" no consta medido.
- **Seguridad**: `Referrer-Policy` (`bf-net/src/referrer.rs`, con la política del documento en
  los subrecursos del shell) y HSTS (`bf-net/src/hsts.rs`) hechos (E6); COOP/COEP y
  `crossOriginIsolated` (E3, `bf-web/src/workers/isolation.rs`; el shell aún no pasa las
  cabeceras). Faltan CSP, mixed content, Permissions Policy y Safe Browsing.
- **Interfaz**: pestañas hechas; historial persistente, marcadores y descargas son de E11, que
  todavía no está en `master`; impresión, falta.
- **Rendimiento**: falta GPU y raster en tiles. 60 fps y primera pintura de Wikipedia: sin medir.

---

## Deuda técnica anotada en el código

Salida de `grep -rn "TODO(" crates` al cierre de la fase 1, agrupada por crate. Cada
`TODO(etiqueta)` cita la fase que lo resuelve. La columna "Estado" es del 2026-11-12 (si el
`TODO` sigue en el archivo).

| Crate | Ubicación | Pendiente | Fase | Estado |
|---|---|---|---|---|
| `bf-dom` | `src/forms.rs:27` | `TODO(event-loop)`: la cola de microtasks de `option` es provisional; pasa al event loop real con `bf-js`. | 2.3 | sigue |
| `bf-html` | `src/tokenizer/mod.rs:62` | `TODO(bf-dom)`: la entrada del tokenizador como code points, para los surrogates de `document.write`. | 2.1 / 2.3 | ya no está |
| `bf-infra` | `src/lib.rs:8` | `TODO(bf-dom)`: `DomString` es un alias de `String`; pasa a WTF-8. | 2.1 | ya no está |
| `bf-infra` | `src/encoding.rs:5` | `TODO(roadmap)`: encodings multibyte (GBK, gb18030, Big5, EUC-JP, ISO-2022-JP, Shift_JIS, EUC-KR). | 3.5 | hecho (E10) |
| `bf-image` | `src/lib.rs:4` | `TODO(roadmap)`: SVG, WebP, AVIF, ICO/BMP y animación de GIF/APNG. | 4.2 | queda AVIF |
| `bf-layout` | `src/controls.rs:9` | `TODO(forms)`: interactividad de formularios, `::placeholder` y scroll interno de `textarea` y list box. | 2.4 / 4.3 | sigue (el scroll ya está, E12) |
| `bf-layout` | `src/positioned.rs:3` | `TODO(sticky)`: `position: sticky` necesita scroll containers reales. | 4.3 | hecho (E12); ahora `TODO(E4)` de `transform` |
| `bf-net` | `src/lib.rs:5` | `TODO(roadmap)`: HTTP/2, caché HTTP, CORS y políticas de seguridad de Fetch. | 2.3 / 4.1 | queda "CORS y políticas de seguridad de Fetch" (CORS está en `bf-web`, E2) |
| `bf-paint` | `src/builder.rs:472` | `TODO(backgrounds)`: `background-attachment: fixed` se pinta como `scroll`; los gradientes no se dibujan. | 3.3 | queda `fixed` |
| `bf-shell` | `src/load.rs:17` | `TODO(http-cache)`: el `CachedLoader` en memoria se reemplaza por la caché HTTP de `bf-net`. | 4.1 | quitado (decisión, ver 4.1) |
| `bf-style` | `src/cascade.rs:610` | `TODO(cascade)`: las reglas UA de quirks ganan por especificidad máxima; debería ser un origen "UA quirks" explícito. | 3 | sigue |

Pendientes que no tienen `TODO(` en el código pero están medidos en
[`wpt-status.md`](wpt-status.md):
- las dos referencias contradictorias y `letter-spacing-080` (sección "Tests con expectativa
  dudosa");
- el ajuste vertical de contornos, que es una aproximación al hinting.

---

## Compatibilidad por UA

- Freedom se identifica con su identidad propia (`bf-infra/src/identity.rs`): no imita a
  Chrome ni a otro navegador.
- whatismybrowser.com la identifica como "Netscape Navigator 5 on Windows 10" ("Firefox on
  Windows" en otra parte de la página): lee `navigator.appName`/`appVersion` y el UA. Con eso
  muestra "Sí" a JavaScript, cookies y cookies de terceros (`pendiente/e4.md`). No se cambia la
  identidad para mejorar esa detección.

## Hallazgos de pruebas reales

Pruebas del usuario con el navegador del 2026-11-12:

- **Pánico entre documentos**: un nodo o un `Range` de una ventana usado desde otra (marcos)
  se resolvía contra la arena equivocada y tumbaba el motor. Arreglado por E1 en `eb29fa4`:
  ahora da `NotSupportedError` o el resultado que define el DOM (`pendiente/e1.md`); la
  adopción real entre ventanas sigue pendiente.
- **`Image` y `PerformanceObserver` ausentes**: `new Image()` lanza `ReferenceError` y corta el
  script (anotado en [`faltantes-web.md`](faltantes-web.md)); `PerformanceObserver` no existe.
  E3, en curso.
- **`TypeError` sin mensaje**: errores que llegan a la consola sin texto, lo que impide saber
  qué falló. E3, en curso.

## Estado por bloque

Lo hecho, lo medido y lo que falta de cada bloque del ciclo army:

- [E1](pendiente/e1.md): Shadow DOM, custom elements, `XMLSerializer`, form-associated, nodos de
  otra ventana sin pánico.
- [E2](pendiente/e2.md): red desde JS (`fetch` por partes, CORS, streams), IndexedDB y CSSOM.
- [E3](pendiente/e3.md): workers, agentes, `BroadcastChannel` entre agentes, COOP/COEP.
- [E4](pendiente/e4.md): hilo por pestaña, BOM, iframes con pintado anidado, cookies de terceros;
  `transform` y animaciones sin enlazar.
- [E5](pendiente/e5.md): sizing, grid, subgrid, grid lanes y flexbox rtl.
- [E6](pendiente/e6.md): HTTP/2, caché HTTP, PSL, HSTS, Referrer-Policy, conexiones, autenticación
  y DNS.
- [E7](pendiente/e7.md): formatos de imagen propios, animación de GIF/APNG y SVG.
- [E8](pendiente/e8.md): test262 (95.3 %), Intl y rendimiento de la VM.
- [E9](pendiente/e9.md): bidi (UAX #9), cortes de línea (UAX #14) y shaping por script.
- [E10](pendiente/e10.md): encodings multibyte; namespaces y parser XML pendientes.
- E11: historial, marcadores y descargas; todavía sin `docs/pendiente/e11.md` en `master`.
- [E12](pendiente/e12.md): scroll containers, sticky, `line-clamp`, `text-overflow`.
