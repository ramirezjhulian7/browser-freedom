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

### 2.4 Interactividad de formularios

- Foco y navegación con `Tab`, edición de texto (cursor, selección, portapapeles) e
  `input`/`change`.
- Casillas y radios con grupos; `select` desplegable; `textarea` con scroll.
- Validación de restricciones y envío de formularios (`application/x-www-form-urlencoded` y
  `multipart/form-data`, GET y POST).
- `::placeholder` y `:focus-visible` (ver `TODO(forms)`).

**Aceptación**: WPT `html/semantics/forms` ≥ 60%. Buscar en Google desde su caja y enviar un
formulario POST a un servidor de prueba local (test en `bf-shell`).

---

## Fase 3: completar CSS y el texto

### 3.1 Tablas

- CSS 2.2 §17 y css-tables-3:
  - modelo de filas, columnas y grupos; cajas anónimas de tabla;
  - layout automático y fijo; `border-collapse`;
  - `caption-side`; alineación vertical de celdas.

**Aceptación**: WPT `css/CSS2/tables` ≥ 75% y `css/css-tables` ≥ 50%. Hacker News se ve con
sus columnas (hoy cada celda cae en su propia línea).

### 3.2 Grid

- css-grid-2:
  - líneas con nombre, áreas y `repeat()`;
  - `minmax()`, `fr`, auto-placement y alineación;
  - subgrid.

**Aceptación**: WPT `css/css-grid` ≥ 60%.

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

### 3.4 Texto internacional

- UAX #9 (bidi) en el formato inline, `direction`/`unicode-bidi` completos.
- `writing-mode` vertical y `text-orientation` (Writing Modes 4).
- UAX #14 (oportunidades de corte de línea), `hyphens`, `text-justify` y shaping complejo
  (árabe, índicos) con fallback de fuentes.

**Aceptación**:
- WPT `css/css-writing-modes` ≥ 40% y `css/CSS2/bidi-text` ≥ 70%.
- Los ~80 reftests de css-flexbox que fallan por `writing-mode`/rtl pasan ≥ 75%.

### 3.5 Encodings multibyte

- Decodificadores y encoders de WHATWG Encoding: GBK/gb18030, Big5, EUC-JP, ISO-2022-JP,
  Shift_JIS y EUC-KR, con los índices de `crates/bf-infra/data/indexes.json`.

**Aceptación**:
- WPT `encoding/` ≥ 90% de los tests sin JS (los de testharness, en la fase 2).
- Los 7 casos omitidos de `url/resources/percent-encoding.json` pasan.

### 3.6 Palabras clave de tamaño intrínseco y `Namespace::Other`

- `min-content`, `max-content`, `fit-content()` y `contain-intrinsic-size` en `width`,
  `height`, `min-*`, `max-*` y `flex-basis`.
- `aspect-ratio`.
- Namespaces arbitrarios en el DOM (`Namespace::Other` con la URI) y el parser XML.

**Aceptación**:
- WPT `css/css-sizing` ≥ 60%.
- Los ~50 reftests de css-flexbox que fallan por las palabras clave de tamaño pasan.
- `css/selectors/of-type-selectors.xhtml` pasa.

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

### 4.2 Formatos de imagen

- SVG como documento propio (parser XML existente, layout SVG 2 básico: formas, paths,
  texto, `viewBox`, gradientes) usado en `<img>`, `background-image` y SVG en línea.
- Decodificadores propios de WebP (VP8/VP8L) y AVIF (AV1 intra), y ICO/BMP.
- Animación de GIF/APNG (`TODO(roadmap)` en `bf-image`).

**Aceptación**:
- WPT `svg/` reftests ≥ 40%.
- Los iconos (hoy cuadrados negros por `mask-image`) y el gráfico de torta de Wikipedia se ven.
- El logo SVG de Hacker News se ve.

### 4.3 Scroll y overflow

- Scroll containers reales: barras de desplazamiento, `overflow: auto|scroll` con su
  propio offset y `scrollbar-gutter`.
- `position: sticky` (`TODO(sticky)`) y `scroll-behavior`.
- `object-fit`/`object-position`, `<picture>`/`srcset` y la carga asíncrona e incremental de
  imágenes con re-layout.

**Aceptación**: WPT `css/css-overflow` ≥ 50% y `css/css-position/sticky` ≥ 50%. `<textarea>`
y los list box se desplazan con la rueda.

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

---

## Deuda técnica anotada en el código

Salida de `grep -rn "TODO(" crates` al cierre de la fase 1, agrupada por crate. Cada
`TODO(etiqueta)` cita la fase que lo resuelve.

| Crate | Ubicación | Pendiente | Fase |
|---|---|---|---|
| `bf-dom` | `src/forms.rs:27` | `TODO(event-loop)`: la cola de microtasks de `option` es provisional; pasa al event loop real con `bf-js`. | 2.3 |
| `bf-html` | `src/tokenizer/mod.rs:62` | `TODO(bf-dom)`: la entrada del tokenizador como code points, para los surrogates de `document.write`. | 2.1 / 2.3 |
| `bf-infra` | `src/lib.rs:8` | `TODO(bf-dom)`: `DomString` es un alias de `String`; pasa a WTF-8. | 2.1 |
| `bf-infra` | `src/encoding.rs:5` | `TODO(roadmap)`: encodings multibyte (GBK, gb18030, Big5, EUC-JP, ISO-2022-JP, Shift_JIS, EUC-KR). | 3.5 |
| `bf-image` | `src/lib.rs:4` | `TODO(roadmap)`: SVG, WebP, AVIF, ICO/BMP y animación de GIF/APNG. | 4.2 |
| `bf-layout` | `src/controls.rs:9` | `TODO(forms)`: interactividad de formularios, `::placeholder` y scroll interno de `textarea` y list box. | 2.4 / 4.3 |
| `bf-layout` | `src/positioned.rs:3` | `TODO(sticky)`: `position: sticky` necesita scroll containers reales. | 4.3 |
| `bf-net` | `src/lib.rs:5` | `TODO(roadmap)`: HTTP/2, caché HTTP, CORS y políticas de seguridad de Fetch. | 2.3 / 4.1 |
| `bf-paint` | `src/builder.rs:472` | `TODO(backgrounds)`: `background-attachment: fixed` se pinta como `scroll`; los gradientes no se dibujan. | 3.3 |
| `bf-shell` | `src/load.rs:17` | `TODO(http-cache)`: el `CachedLoader` en memoria se reemplaza por la caché HTTP de `bf-net`. | 4.1 |
| `bf-style` | `src/cascade.rs:610` | `TODO(cascade)`: las reglas UA de quirks ganan por especificidad máxima; debería ser un origen "UA quirks" explícito. | 3 |

Pendientes que no tienen `TODO(` en el código pero están medidos en
[`wpt-status.md`](wpt-status.md):
- las dos referencias contradictorias y `letter-spacing-080` (sección "Tests con expectativa
  dudosa");
- el ajuste vertical de contornos, que es una aproximación al hinting.
