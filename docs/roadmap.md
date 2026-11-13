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

**Estado al 2026-11-13** (`master` `0c8b903`, cierre del ciclo). Cada apartado lleva un
bloque **Estado** con lo hecho, lo parcial y lo que falta, la cifra medida y dónde vive. Las
cifras salen de [`wpt-status.md`](wpt-status.md), [`test262-status.md`](test262-status.md),
[`perf.md`](perf.md) y los `docs/pendiente/e*.md` y `docs/pendiente/mac/e*.md` (sección
"Estado por bloque", al final); cuando un criterio no se ha medido, se dice. Un criterio
"cumplido" lo está en la medición citada, no en una corrida completa de la fase. Cada cifra
dice dónde se midió: **Mac** (macOS, Apple M4, desde el 2026-11-12) o **Windows** (las
mediciones anteriores, con su fecha).

## Avance por criterios

Un criterio sin medir cuenta como no cumplido. Cada fase pesa igual en el total. La fase 1
está cerrada y no entra en la cuenta. Con ella al 100 %, el total sería (100 + 94.7 + 94.1 +
100 + 33.3) / 5 = **84.4 %**.

| Fase | Criterios | Cumplidos | Sin medir | Avance |
|---|---:|---:|---:|---:|
| 2: JavaScript y la plataforma web dinámica | 19 | 18 | 0 | 94.7 % |
| 3: completar CSS y el texto | 17 | 16 | 0 | 94.1 % |
| 4: red, medios y documentos | 9 | 9 | 0 | 100.0 % |
| 5: navegador de uso diario | 6 | 2 | 0 | 33.3 % |
| **Total** (media de las fases) | 51 | 45 | 0 | **80.5 %** |

Cuenta: (94.7 + 94.1 + 100 + 33.3) / 4 = 80.5 %; cumplidos 18 + 16 + 9 + 2 = 45 de
19 + 17 + 9 + 6 = 51.

Sin cambios en la tabla desde la ronda 3 del ciclo de cierre: lo integrado después
(hasta `0c8b903`) mejora cifras, pero no cierra ningún criterio (ver la lista de abajo).
Quedan fuera de `master`, en curso, `army/e4`, `army/e5` y `army/e6` (`RETOMAR-EN-MAC.md`,
sección 8).

Cambios respecto de la tabla de la ronda 1 del ciclo de cierre (76.4 %):
- **Fase 5, `html-aam` y `wai-aria`**: 93.3 % y 91.8 % en `master` (E1, integrado en
  `21ea345`; `pendiente/mac/e1.md`, "Ciclo de cierre, ronda 1").

Cambios de la ronda 1 del ciclo de cierre respecto de la tabla del 2026-11-12 (68.0 %):
- **2.3, reftests `needs-js`**: se cumple con el criterio reformulado (ver 2.3).
- **3.3, `linear-gradient` en css-flexbox**: 16 / 16 (E5, ronda 6).
- **4.1, recarga de Wikipedia**: se cumple con el criterio reformulado (ver 4.1).
- **4.3, rueda sobre `<textarea>` y list box**: hay test (E6, ronda 6).
- **Fase 5**: los tres criterios que estaban sin medir ya tienen cifra, pero ninguno se cumple
  en `master`.

Criterios no cumplidos, con su cifra actual y su meta (todas del Mac):

| Apartado | Criterio | Cifra actual | Meta | Fuente |
|---|---|---|---|---|
| 2.3 | WPT `html/syntax/parsing` sin fallos, con los tests de `document.write` | 8159 / 8166 subtests (99.9 %), 236 documentos; de los 7 que quedan, 5 piden que el parser ceda el control en mitad de un paso y 2 no se pueden cumplir | 0 fallos | `pendiente/mac/e1.md`, "Ciclo de cierre, ronda 5" |
| 3.6 | Los ~50 reftests de css-flexbox por palabras clave de tamaño | 95 / 107 en `master` (8 de los 12 que faltan son de CSS Flexbox 2) | pasan | `pendiente/mac/e5.md`, "Ciclo de cierre, ronda 3" |
| 5 | Un lector de pantalla lee Wikipedia | no: el motor calcula rol y nombre accesibles, pero el árbol no se expone al sistema (ver "Decisiones pendientes del usuario") | lo lee | `pendiente/mac/e1.md`, "Ciclo de cierre, ronda 1" |
| 5 | Un fuzzer de IPC no escapa del sandbox | no: no hay procesos ni IPC (ver "Decisiones pendientes del usuario") | no escapa | — |
| 5 | 60 fps al desplazar Wikipedia | E4: mediana de 8.0 ms por cuadro a densidad 1 y 17.7 ms a densidad 2 (carga 15-36). Planificador sobre `0995c60`, carga 46-58: 12.6-15.3 ms a densidad 1 y 27.7-37.7 ms a densidad 2. Sin confirmar con la máquina libre | 60 fps (16.7 ms) | `pendiente/mac/e4.md`, "Cierre, ronda 3"; la del planificador, su reporte de la ronda 7 |
| 5 | Primera pintura de Wikipedia en release | última medida completa 823-936 ms (E2, carga 25, antes del arreglo de red de E6); sin medida sobre `master` con la máquina libre. Por partes: red 401 ms (E6), parseo 56-60 ms (E2 y E3), cascada 76 ms (E4), maqueta 100-168 ms (E4) | < 500 ms | [`rendimiento.md`](rendimiento.md); `pendiente/mac/e6.md` y `e3.md`, "Ciclo de cierre, ronda 5"; `e4.md`, "Cierre, ronda 1" y "Cierre, ronda 3"; la de E2, `git show 3dfeedf:docs/pendiente/mac/e2.md`, "Ciclo de cierre, ronda 6" |

El método de las dos cifras de rendimiento está en [`rendimiento.md`](rendimiento.md). Las
dos se midieron con la máquina cargada por otros ejecutores.

## Decisiones pendientes del usuario

Cinco puntos no avanzan sin una decisión que no le toca a ningún ejecutor:
- **Lector de pantalla (fase 5).** El motor ya calcula rol y nombre accesibles, pero un lector
  de pantalla de macOS solo lee lo que se expone por `NSAccessibility`. Eso pide FFI con
  Objective-C, es decir `unsafe`, que el proyecto prohíbe, o un crate externo nuevo.
- **Fuzzer de IPC (fase 5).** Hace falta separar el motor en procesos (navegador y contenido)
  con IPC propio. Hoy cada pestaña es un hilo del mismo proceso.
- **3.6, flexbox por palabras clave de tamaño.** 8 de los 108 tests son de CSS Flexbox 2
  (`flex-wrap: balance`, `flex-line-count`), un borrador sin texto estable. O se implementa
  sin especificación estable o se sacan del criterio.
- **2.3, los 2 subtests de `html/syntax/parsing` que no se pueden cumplir.**
  `DOMContentLoaded-defer.html` pide lo contrario que
  `DOMContentLoaded-defer-task-order.tentative.html`, que pasa, y
  `adoption-agency-reparenting-pagehide.tentative.html` espera un `pagehide` que el estándar
  no dispara. O se sacan del criterio o 2.3 no puede llegar a 0 fallos.
- **Primera pintura, el suelo de la red.** El documento de Wikipedia trae `max-age=0,
  must-revalidate` y hay que validarlo en cada visita. En un proceso nuevo eso son tres viajes
  de ida y vuelta (TCP, TLS 1.3 y el 304), unos 115 ms cada uno en esta red: un suelo de unos
  345 ms antes de parsear nada (`pendiente/mac/e6.md`, "Ciclo de cierre, ronda 5"). Bajarlo
  pide reanudar TLS con 0-RTT (RFC 8446 §2.3), guardando los *session tickets* entre procesos.
  O se acepta, o se mide la primera pintura sin la red (con el documento ya validado), o se
  cambia la meta.

Los criterios cumplidos están en el bloque **Estado** de cada apartado. Tres de ellos se apoyan
en capturas y no en una suite: Hacker News con columnas (3.1), los iconos y la torta de
Wikipedia y el logo de Hacker News (4.2), todas de E7 en Windows (2026-11-06). Los de sitios
reales de 2.3 y 2.4 (`real_sites.rs`) usan la red y están ignorados por defecto.

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

**Estado** (5 de 5 criterios cumplidos):
- **Hecho**: parser (test262 solo parsing: 51 490 pasan, 0 fallan, 527 omitidos por
  propuestas; test262-parser-tests 3332, 0 fallan; Windows, sin fecha en
  `test262-status.md`); bytecode, VM e intérprete
  (`bf-js/src/{compiler,vm}/`); GC propio de marcado y barrido con umbral adaptativo y pausas
  medidas (`perf.md`, sección GC); builtins de ES2026 completos en `built-ins/` (Mac, 2026-11-12: Temporal
  4605/4605, Explicit Resource Management, `Array.fromAsync`, `Uint8Array` base64/hex,
  ShadowRealm 64/64); Atomics multi-agente con `$262.agent` y `SharedArrayBuffer` real entre
  hilos (`bf-js/src/host/agents.rs`, `built-ins/Atomics` 389/389); el `TODO(bf-dom)` de
  `DomString` ya no está en `bf-infra/src/lib.rs`.
- **Criterios**, test262 completo en ejecución (**Mac, 2026-11-12**, `master` `5b9a230`,
  `test262-status.md`): `language/` 23 183 pasan, 11 fallan (**99.95 %**, meta ≥ 90 %
  cumplida); `built-ins/` 23 554 pasan, 3 fallan (**99.99 %**, meta ≥ 80 % cumplida); total
  50 594 / 51 220 (**98.8 %**; Windows, 2026-11-07: 95.3 %). **0 pánicos** en todo test262
  (el runner los anota como `pánico: …`; criterio cumplido) y 1 timeout por la carga de la
  máquina. `richards` y `deltablue` (implementación propia: el original es GPL) y
  `navier-stokes` en `crates/bf-js/bench/`, con su comprobación en verde: 590.6, 460.8 y
  9 551.7 ms (Mac, 2026-11-12, máquina cargada; `perf.md`): criterio cumplido.
- **Parcial**: `intl402/` **82.0 %** (2 773 / 3 382, Mac, 2026-11-12; con `Intl.Segmenter`,
  `DurationFormat`, `DisplayNames` y los datos de `de`/`pl`). Faltan zonas IANA y los
  calendarios `chinese`/`dangi` (`pendiente/mac/e3.md`).
- **Falta**: GC generacional o incremental. `navier-stokes` es el benchmark más lento (unos
  640 ms por cuadro, Mac). `fib(30)` 363 ms en Windows (E8, 2026-11-12); los 20 benchmarks en
  el Mac se midieron con la máquina cargada y hay que repetirlos.

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

**Estado** (6 de 6 criterios cumplidos):
- **Hecho**: testharness.js corre en el motor (`bf-web/src/html/testharness.rs`, binario
  `wpt-testharness`; criterio cumplido). `html/dom` **97.2 %** (Mac, 2026-11-12; meta ≥ 60 %
  cumplida); `css/cssom` **81.6 %** (Mac, 2026-11-12; meta ≥ 50 % cumplida). E1, Mac,
  2026-11-12 (`pendiente/mac/e1.md`): `dom/nodes` **92.8 %** (12060 / 12998; meta ≥ 85 %
  cumplida), `dom/events` **80.2 %** (734 / 915; meta ≥ 80 % cumplida), `idlharness` de DOM
  **86.9 %** (2058 / 2369) y de HTML **80.0 %** (5920 / 7398) (meta ≥ 80 % cumplida). Más
  allá del apartado (Mac, 2026-11-12): Shadow DOM (`shadow-dom/` 97.3 %), custom elements
  (`custom-elements/` 83.5 %), `XMLSerializer` (E1, `bf-dom/src/{shadow,custom}.rs`,
  `bf-web/src/dom/`); `css/cssom-view` 56.5 %.
- **Parcial**: `domparsing/` 23.9 % (Mac; sin `tentative/` 42.6 %): faltan los métodos
  posicionales y la API Sanitizer.
- **Sin medir**: re-estilo y re-layout incrementales (no hay cifra). Los bindings se escriben
  a mano por interfaz, no salen de un generador desde los `.idl`.

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
- ≥ 60 % de los reftests `needs-js` no manuales pasa a ejecutarse.
  - *Reformulado por decisión del usuario (2026-11-13).* Texto original: ~~"De los 2110
    reftests `needs-js` de la fase 1, ≥ 60% pasa a ejecutarse (el resto son manuales o de
    otras APIs)"~~.
  - Razón: 852 de los 2114 `needs-js` (40.3 %) son tests visuales manuales sin JavaScript
    ("Test passes if…"), que ningún runner puede ejecutar. El máximo alcanzable era 59.7 %,
    por debajo de la meta (`pendiente/mac/e6.md`, ronda 5).
- Google, Wikipedia y Hacker News con su JavaScript cargan sin pánicos, y el buscador de
  Google funciona escribiendo en la caja y pulsando Intro.

**Estado** (4 de 5 criterios cumplidos):
- **Hecho**: hilo por pestaña con motor JS, scripts del documento, `document.write`, módulos e
  import maps (`bf-web/src/bom/script.rs`), eventos de usuario despachados desde
  `bf-shell/src/window/events.rs`, `fetch()`/XHR con CORS, *mode* y *credentials*
  (`bf-web/src/net/cors.rs`, E2); `fetch()` por partes, streams, IndexedDB, WebSocket y
  EventSource (E2); workers, `SharedWorker`, `MessagePort` y `BroadcastChannel` entre agentes
  (E3, `bf-web/src/workers/`). `requestAnimationFrame` integrado (E4). E3, ronda 6: el punto
  de control de microtasks antes de cada script del parser (§13.2.6.4.8), *flush autofocus
  candidates* en *update the rendering*, los scripts SVG (§13.2.6.5) y el `load` del
  `<iframe>` tras `document.close()`. E2, rondas 6 y 8 (`pendiente/mac/e2.md`): *update the
  image data* de los `<img>` (§4.8.4.3.5: `load`, `error`, `decode()`); el `load` de un
  `<iframe>` espera al de su documento y retiene el del padre (§7.4.6.3); `load` de `<link>`.
  E5, ronda 7 (`pendiente/mac/e5.md`): CSS Font Loading (`document.fonts`, `FontFace`,
  `FontFaceSet`), `css/css-font-loading` 61.1 % (44 / 72).
- **Criterios** (Mac, 2026-11-12): `fetch/api/basic` **89.4 %** (839 / 939; meta ≥ 70 %
  cumplida). `html/webappapis/scripting/event-loops` **75.0 %** (6 / 8 subtests en 7
  documentos, E3 ronda 6; meta ≥ 70 % cumplida; el runner ya no cuenta
  `task_microtask_ordering-manual.html`, que es manual: con él serían 6 / 9). Google,
  Wikipedia y Hacker News cargan sin pánicos con su JavaScript y la búsqueda de Google
  funciona: `crates/bf-shell/tests/real_sites.rs` (E6; criterio cumplido).
- **Reftests `needs-js` no manuales: 93.1 %** (criterio reformulado cumplido; meta ≥ 60 %).
  - El runner es `freedom wpt-reftests-js` (`crates/bf-shell/src/reftest_js.rs`, de E6), en
    `master` desde `3f2076c`.
  - De los 2114 `needs-js` de la fase 1, 1262 no son manuales. Se ejecutan 1175: 381 reftests
    con JavaScript y 794 documentos de testharness que terminan con el harness OK. Sobre el
    total, el 55.6 % (`pendiente/mac/e6.md`, ronda 5).
  - En la ronda 6 de E6, sobre `master`, los reftests con JavaScript ejecutados subieron a
    387 / 402 (96.3 %) (`pendiente/mac/e6.md`, ronda 6). La cifra combinada no se volvió a
    medir.
- **Parcial**: `html/syntax/parsing` **8159 / 8166 subtests (99.9 %)** en 236 documentos (Mac,
  2026-11-13, E1, `pendiente/mac/e1.md`, "Ciclo de cierre, ronda 5"; 8155 en la ronda 2 de
  E3). Las variantes `?file=…` de `html5lib_url`, `html5lib_write` y `html5lib_write_single`
  pasan todas (5423 / 5423), y el árbol de html5lib sin JS sigue en 3900 pasan, 0 fallan.
  Meta: 0 fallos. Quedan 7:
  - Construcción síncrona de custom elements y los *children changed steps* de un `script` en
    mitad de un token, como `foster-parenting-moved-table` (5). Necesitan que el tree builder
    pueda llamar al host en mitad de un paso; hoy solo lo hace entre tokens.
  - `DOMContentLoaded-defer.html` (1): pide lo contrario que
    `DOMContentLoaded-defer-task-order.tentative.html`, que pasa.
  - `adoption-agency-reparenting-pagehide.tentative.html` (1): espera un `pagehide` que el
    estándar no dispara.
  - Estos dos últimos no se pueden cumplir; sacarlos del criterio es una decisión pendiente
    (ver "Decisiones pendientes del usuario").
  - Los 4 de nodos que un script lleva a otro documento ya pasan: el parser sigue a los nodos
    adoptados (E1, `bf-web/src/dom/adopt.rs`).
- **Falta**: `TODO(event-loop)` sigue en `bf-dom/src/forms.rs`. En el runner de reftests con
  JavaScript, los errores de todos los grupos bajaron de 141 a 114 con CSS Font Loading
  (`pendiente/mac/e5.md`, ronda 7). Siguen faltando `Element.animate()` y algunos `load`
  (`pendiente/mac/e6.md`, ronda 6).

### 2.4 Interactividad de formularios

- Foco y navegación con `Tab`, edición de texto (cursor, selección, portapapeles) e
  `input`/`change`.
- Casillas y radios con grupos; `select` desplegable; `textarea` con scroll.
- Validación de restricciones y envío de formularios (`application/x-www-form-urlencoded` y
  `multipart/form-data`, GET y POST).
- `::placeholder` y `:focus-visible` (ver `TODO(forms)`).

**Aceptación**: WPT `html/semantics/forms` ≥ 60%. Buscar en Google desde su caja y enviar un
formulario POST a un servidor de prueba local (test en `bf-shell`).

**Estado** (3 de 3 criterios cumplidos):
- **Hecho**: `html/semantics/forms` **63.5 %** (3044 / 4791, E1, Mac, 2026-11-12; meta ≥ 60 %
  cumplida; 44.9 % en la línea base del Mac y 42.2 % en Windows). La *activation behavior*
  (envío con `submit`/`formdata`/`invalid`, reset, `label`), form-associated custom
  elements, `HTMLFormControlsCollection` y `RadioNodeList` (E1, `bf-web/src/html/forms.rs`);
  `textarea` y list box con scroll (E12). Envío POST contra un servidor local:
  `crates/bf-shell/tests/forms.rs` (`post_urlencoded`, `post_multipart`, E6; criterio
  cumplido). Buscar en Google escribiendo en la caja y pulsando Intro:
  `crates/bf-shell/tests/real_sites.rs::google_search` (E6, Mac, 2026-11-12; ignorado por
  defecto porque usa la red; criterio cumplido).
- **Falta**: `DataTransfer`; `TODO(forms)` sigue en `bf-layout/src/controls.rs`.

---

## Fase 3: completar CSS y el texto

### 3.1 Tablas

- CSS 2.2 §17 y css-tables-3:
  - modelo de filas, columnas y grupos; cajas anónimas de tabla;
  - layout automático y fijo; `border-collapse`;
  - `caption-side`; alineación vertical de celdas.

**Aceptación**: WPT `css/CSS2/tables` ≥ 75% y `css/css-tables` ≥ 50%. Hacker News se ve con
sus columnas (hoy cada celda cae en su propia línea).

**Estado**: **hecho** (3 de 3). `css/CSS2/tables` **95.2 %** y `css/css-tables` **76.8 %** (Mac,
2026-11-12; las dos metas cumplidas; en Windows, 2026-10-20: 98.4 % y 73.9 %; la bajada de
`CSS2/tables` está sin investigar, ver `wpt-status.md`). Hacker News con columnas: no hay
medición escrita, pero las capturas de E7 (Windows, 2026-11-06) muestran su página bien.

### 3.2 Grid

- css-grid-2:
  - líneas con nombre, áreas y `repeat()`;
  - `minmax()`, `fr`, auto-placement y alineación;
  - subgrid.

**Aceptación**: WPT `css/css-grid` ≥ 60%.

**Estado**: **hecho** (1 de 1). `css/css-grid` **72.4 %** (1135 / 1567, Mac, 2026-11-12; meta
cumplida). Grid lanes (Grid 3) 584 / 851 (Mac); subgrid 56 / 88 (Windows, 2026-11-06)
(`bf-layout/src/grid.rs`, `grid/lanes.rs`). Falta: baseline, `z-index` en ítems (necesita
pintura), subgrid dentro de lanes.

### 3.3 Efectos visuales y animación

- `box-shadow` y `text-shadow`.
- Gradientes lineales, radiales y cónicos (`TODO(backgrounds)`).
- `border-image`; `background-attachment: fixed` y el clip `text`/`border-area`.
- Transforms 2D/3D (con stacking context y hit testing transformado), filtros y
  `mix-blend-mode`.
- Transitions y animations (`@keyframes` ya se parsea), sincronizadas con el event loop.

**Aceptación**:
- WPT `css/css-backgrounds` ≥ 70% (42.0 % al escribir el criterio).
- `css/css-transforms` ≥ 50%, `css/css-transitions` y `css/css-animations` ≥ 40%.
- Los reftests de css-flexbox que fallan por `linear-gradient` en el fondo de control pasan.

**Estado** (5 de 5 criterios cumplidos):
- **Hecho**: `css/css-backgrounds` **94.5 %** (581 / 615, E2, Mac, 2026-11-12; meta ≥ 70 %
  cumplida; el 42 % y el 52 % de antes eran de otro recuento). `box-shadow` y `text-shadow`
  (`bf-style/src/properties/mod.rs`); gradientes (`bf-style/src/properties/gradient.rs`);
  `border-image` (`bf-paint/src/border_image.rs`, E7); `filter`, `clip-path` con formas
  básicas y `mix-blend-mode` en cajas HTML (`bf-paint/src/filters.rs`, E7). `transform`,
  transiciones y animaciones (E4): `css/css-transforms` **81.5 %** (reftests, Mac,
  2026-11-12; meta ≥ 50 % cumplida), `css/css-transitions` **72.1 %** (2470 / 3428) y
  `css/css-animations` **55.5 %** (789 / 1422) (testharness, E4, Mac, ronda 5,
  `pendiente/mac/e4.md`; metas ≥ 40 % cumplidas; en la ronda 3 eran 71.6 % y 51.8 % con otros
  totales de subtests). El `TODO(E4)` de `bf-layout/src/positioned.rs` ya no está.
- **Hecho después**: los reftests de css-flexbox con `linear-gradient`: **16 / 16** (Mac,
  2026-11-12, E5 ronda 6, en `master` desde `cb9ab93`,
  `pendiente/mac/e5-flexbox-subconjuntos.md`; criterio cumplido). `align-self-016` pasa con el
  reajuste fit-content en columnas multilínea y la suma de floats seguidos en el max-content.
- **Falta**: `background-attachment: fixed` (`TODO(backgrounds)`, `bf-paint/src/builder.rs`);
  `transform` 3D (se aplana, sin `perspective`); `url()` en `filter` y `clip-path` de HTML.

### 3.4 Texto internacional

- UAX #9 (bidi) en el formato inline, `direction`/`unicode-bidi` completos.
- `writing-mode` vertical y `text-orientation` (Writing Modes 4).
- UAX #14 (oportunidades de corte de línea), `hyphens`, `text-justify` y shaping complejo
  (árabe, índicos) con fallback de fuentes.

**Aceptación**:
- WPT `css/css-writing-modes` ≥ 40% y `css/CSS2/bidi-text` ≥ 70%.
- Los ~80 reftests de css-flexbox que fallan por `writing-mode`/rtl pasan ≥ 75%.

**Estado** (3 de 3 criterios cumplidos):
- **Hecho**: UAX #9 y UAX #14 completos (`bf-layout/src/{bidi,linebreak}.rs`; conformidad UCD
  17.0.0, Windows, E9, 2026-11-10: BidiTest 770241/770241, BidiCharacterTest 91707/91707,
  LineBreakTest 19338/19338);
  IFC con niveles bidi, `text-justify`, `text-align-last`, `line-break`, `hyphens` manual
  (`bf-layout/src/inline.rs`); shaping por script con fallback (`bf-font`). Escritura vertical
  (`writing-mode: vertical-rl`/`vertical-lr`), flujos ortogonales e ítems de grid ortogonales
  (E5, `master` `7469c42`). Mac, 2026-11-12: `css/css-writing-modes` **77.4 %** (854 / 1104;
  meta ≥ 40 % cumplida), `css/CSS2/bidi-text` **79.0 %** (meta ≥ 70 % cumplida),
  `css/css-text` 59.2 %. Flexbox con `direction: rtl` y propiedades lógicas inline (E5).
- **Hecho después**: los reftests de css-flexbox por `writing-mode`/rtl: **107 / 136
  (78.7 %)** (Mac, 2026-11-12, E5 ronda 6, `pendiente/mac/e5-flexbox-subconjuntos.md`; meta
  ≥ 75 % cumplida; 103 / 136 en `master` `d1148b8`).
- **Falta**: `hyphens: auto` con diccionario, propagación de `direction`/`writing-mode` del
  `<body>` (`pendiente/e9.md`), `sideways-lr` y la baseline en flujos verticales.

### 3.5 Encodings multibyte

- Decodificadores y encoders de WHATWG Encoding: GBK/gb18030, Big5, EUC-JP, ISO-2022-JP,
  Shift_JIS y EUC-KR, con los índices de `crates/bf-infra/data/indexes.json`.

**Aceptación**:
- WPT `encoding/` ≥ 90% de los tests sin JS (los de testharness, en la fase 2).
- Los 7 casos omitidos de `url/resources/percent-encoding.json` pasan.

**Estado**: **hecho** (2 de 2, E10). Decodificadores y encoders en streaming en
`bf-infra/src/encoding_codec.rs` con los índices generados (`encoding_mb_tables.rs`). WPT
`encoding` **97.5 %** (fila del directorio y también todo `encoding/` sin `legacy-mb-*`:
23 039 / 23 623; testharness, Mac, 2026-11-12); los `legacy-mb-*` que no pasan decodifican
por un `<iframe>` o envían un formulario a uno (`notrun`). Los 7 casos de
`percent-encoding.json` pasan (16/16, 0 omitidos, `cargo test -p bf-url --test wpt_url`, Mac,
2026-11-12). El `TODO(roadmap)` de `bf-infra/src/encoding.rs` ya no está.

### 3.6 Palabras clave de tamaño intrínseco y `Namespace::Other`

- `min-content`, `max-content`, `fit-content()` y `contain-intrinsic-size` en `width`,
  `height`, `min-*`, `max-*` y `flex-basis`.
- `aspect-ratio`.
- Namespaces arbitrarios en el DOM (`Namespace::Other` con la URI) y el parser XML.

**Aceptación**:
- WPT `css/css-sizing` ≥ 60%.
- Los ~50 reftests de css-flexbox que fallan por las palabras clave de tamaño pasan.
- `css/selectors/of-type-selectors.xhtml` pasa.

**Estado** (2 de 3 criterios cumplidos):
- **Hecho**: palabras clave de tamaño, `aspect-ratio`, `contain` y `contain-intrinsic-size`
  (E5, `bf-layout/src/{block,flex,grid}.rs`). `css/css-sizing` **67.6 %** (355 / 525, Mac,
  2026-11-12, `REFTEST_GROUPS=css/css-sizing`: no está en `GROUPS`; meta cumplida).
  `css/selectors/of-type-selectors.xhtml` **pasa** (Mac, 2026-11-12; criterio cumplido).
- **Parcial**: los reftests de css-flexbox con palabras clave de tamaño: **95 / 107** en
  `master` (Mac, 2026-11-13, E5, `pendiente/mac/e5.md`, "Ciclo de cierre, ronda 3"; meta: que
  pasen todos; 86 / 108 en la ronda 6 de E5, 74 / 108 antes; el 108 contaba un test que hoy no
  se puede ejecutar). Entró el algoritmo de tamaños intrínsecos de flex, §9.9.1 (E5, "Ciclo de
  cierre, ronda 1"). `css/css-flexbox` en total: **803 / 949** (780 antes). Fallan 12:
  - `flex-wrap: balance` y `flex-line-count` (8). Son de CSS Flexbox 2, un borrador sin texto
    estable, y no se implementan; sacarlos del criterio es una decisión pendiente.
  - Otros 4 (el desglose por causa de la ronda 6, en
    `pendiente/mac/e5-flexbox-subconjuntos.md`, es anterior a §9.9.1).
- **`Namespace::Other`**: `Other` y `Null` existen en `bf-infra`; el parser XML ya trata un
  URI vacío como *null* (E1), pero sigue mapeando el namespace ausente a HTML en otros casos.
- **Falta**: `lookupNamespaceURI`/`isDefaultNamespace`, `@namespace` y `ns|E` en selectores,
  parser XML completo (bloques 2 y 3 de `pendiente/e10.md`).

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
- Recargar Wikipedia no descarga ningún cuerpo: solo hace las validaciones condicionales que
  RFC 9111 §4.2.4 y §5.2.2.2 obligan (`must-revalidate`, `no-cache`), y ningún recurso fresco
  se vuelve a pedir.
  - *Reformulado por decisión del usuario (2026-11-13).* Texto original: ~~"Recargar
    Wikipedia hace ≤ 2 peticiones de red nuevas"~~.
  - Razón: Wikipedia sirve 7 recursos con `max-age=0, must-revalidate`, y RFC 9111 obliga a
    validarlos en cada uso. Ningún navegador que siga el estándar puede bajar de 7 peticiones
    (`pendiente/mac/e6.md`, ronda 4).

**Estado** (3 de 3 criterios cumplidos):
- **Hecho** (E6): HTTP/2 y HPACK propios (`bf-net/src/{http2,hpack}.rs`); tests contra un
  servidor h2 local en `bf-net/tests/h2.rs` (frames, `CONTINUATION`, control de flujo,
  `GOAWAY`/`REFUSED_STREAM`, ALPN) y los vectores C.1–C.6 de RFC 7541 en ambos sentidos; los
  frames `PRIORITY` se ignoran con `SETTINGS_NO_RFC7540_PRIORITIES` (RFC 9218): criterio
  cumplido. Caché en memoria y disco (`bf-net/src/cache.rs`) con variantes por `Vary`, rangos
  y parciales; WPT `fetch/http-cache` **92.8 %** (387 / 417, Mac, 2026-11-12; meta
  cumplida; 90.4 % en Windows). Cookies en disco y PSL (`bf-net/src/{cookies,psl}.rs`); el `TODO(http-cache)`
  del shell se quitó (decisión del planificador: `CachedLoader` queda como memoria del
  documento). Además: HSTS, Referrer-Policy, Happy Eyeballs.
- **Hecho después** (criterio reformulado cumplido): recargar
  `es.wikipedia.org/wiki/Navegador_web` (E6, Mac, 2026-11-12, `F5` con la caché en disco,
  `pendiente/mac/e6.md`, ronda 4) da 41 entradas.
  - 34 salen de la caché sin red.
  - **7 se validan con 304 y hay 0 descargas.** Las 7 llevan `max-age=0, must-revalidate`.
  - En Windows (2026-11-10) eran 8 peticiones, 2 de ellas descargas.
- **Integrado** (`4bb212d`, `76d4eca`, ya en `master`): `immutable`,
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

**Estado** (3 de 3 criterios cumplidos):
- **Hecho** (E7): `crates/bf-svg` y `bf-paint/src/images/svg.rs` (SVG en `<img>`,
  `background-image`, `mask-image` y en línea; `pattern`, `mask`, filtros, `textPath`,
  `<image>`, `foreignObject` básico, recursos externos); WPT `svg/` **71.8 %** (324 / 451,
  Mac, 2026-11-12; meta ≥ 40 % cumplida). WebP (VP8, VP8L, `ALPH`), BMP e ICO/CUR propios
  (`bf-image/src/`); animación de GIF y APNG en la ventana. `mask-image`: los iconos de
  Wikipedia y el logo y las flechas de Hacker News se ven (capturas de E7, Windows, 2026-11-06); la torta
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

**Estado** (3 de 3 criterios cumplidos):
- **Hecho** (E12): scroll containers con barras superpuestas, rueda, arrastre y teclado
  (`bf-layout/src/scroll.rs`, `bf-paint/src/scrollbars.rs`); `position: sticky`; `line-clamp`,
  `text-overflow` y `overflow-clip-margin`. Mac, 2026-11-12: `css/css-overflow` **65.1 %** y
  `css/css-position/sticky` **100 %** (11 / 11 ejecutables, 98 `needs-js`): metas cumplidas.
  `object-fit`/`object-position` (`bf-style/src/properties/object.rs`, `bf-paint/src/images.rs`):
  179 de 182 tests `object-fit*`/`object-position*` (98.4 %, Mac).
- **Hecho después**: la rueda sobre un `<textarea>` de 60 líneas y sobre un `<select size=4>`
  con 30 opciones cambia su `scrollTop` y la página no se mueve. Fuera de ellos, la rueda
  desplaza la página (`crates/bf-shell/tests/wheel.rs`, por el mismo camino que
  `WindowEvent::MouseWheel`; E6, ronda 6, `pendiente/mac/e6.md`; criterio cumplido).
- **Hecho después** (E2, `master` `b2b2f0d`): `srcset`, `sizes` y `<picture>` eligen la imagen
  con la densidad real de la pantalla (`bf_image::source`), con `currentSrc`.
- **Hecho después** (E2, ronda 8, `pendiente/mac/e2.md`): `loading=lazy` (HTML §4.8.4.3.9)
  aplaza la petición hasta que la imagen queda a menos de 1250 px del viewport. Cada imagen se
  decodifica una vez por proceso (`bf_image::decode_frames_shared`). WPT `the-img-element`
  75.2 % (1470 / 1954). Desde la ronda 1 del ciclo de cierre (E6, `pendiente/mac/e6.md`) la
  ventana también las pide al desplazar (`Engine::update_view`).
- **Hecho después** (E2, `pendiente/mac/e2.md`, "Ciclo de cierre, ronda 4"; en `master`
  desde `0c8b903`): el pintado respeta `loading=lazy`. `bf-web` anota en el documento las
  imágenes perezosas que espera (`Document::set_image_deferred`) y
  `bf_paint::images::load_images_in` no las pide; cuando llegan, `bf-web` deja la respuesta en
  la caché del loader de la ventana (`ResourceLoader::remember`) y el pintado la toma de ahí,
  sin descargarla dos veces. En la ventana, las perezosas se revisan con la maqueta del
  navegador (E3, HTML §8.1.7.3): las cercanas aparecen un cuadro después de la primera
  pintura.
- **Falta**: `loading=lazy` dentro de un `<iframe>` y dentro de contenedores con `overflow`
  (no se recorta contra ellos); una imagen no perezosa que piden a la vez `bf-web` y el
  primer pintado se descarga dos veces si la respuesta no es cacheable (`CachedLoader` no
  junta las peticiones en vuelo; E6 lo tiene en curso en `army/e6`); carga incremental de
  imágenes; `scroll-behavior: smooth` es instantáneo; `scroll-snap`, `overscroll-behavior`,
  scroll containers RTL.

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

**Estado** (2 de 6 criterios cumplidos):
- **Accesibilidad** (E1, en `master`; `pendiente/mac/e1.md`, "Ciclo de cierre, ronda 1" y
  "Ciclo de cierre, ronda 3"):
  - El motor calcula el rol y el nombre accesible de cada elemento (HTML-AAM, ARIA 1.2,
    AccName 1.2, también a través de shadow roots y slots), y los expone a los tests con
    `test_driver.get_computed_role`/`get_computed_label`.
  - **Árbol de accesibilidad** del documento (`bf_dom::accessibility::tree`, WAI-ARIA 1.2
    §7.1 y Core-AAM §5.1): qué entra y qué no, `aria-owns`, estados (`checked`, `pressed`,
    `expanded`, `selected`, `disabled`, `level`, `value`) y una serialización en texto en
    orden de lectura (`bf_web::html::accessibility`). Hay un test con un artículo de Wikipedia
    (`bf-web/tests/accessibility.rs`).
  - `html-aam` **93.4 %** (830 / 889) y `wai-aria` **95.7 %** (563 / 588): criterio
    cumplido (meta ≥ 50 %). `accname` 91.7 % y `core-aam` 8 / 8.
  - También en `master`: la reflexión ARIA en el DOM (`ARIAMixin`) y el zoom de texto (E11).
  - **Falta** exponer el árbol al sistema (`NSAccessibility`) para que un lector de pantalla
    lea Wikipedia: decisión pendiente (ver "Decisiones pendientes del usuario").
- **Procesos y sandbox**: falta. Hoy cada pestaña es un hilo del mismo proceso
  (`bf-shell/src/window/worker.rs`); separar procesos es una decisión pendiente. Un pánico en
  una pestaña **no cierra la ventana**: la pestaña muestra "Esta página falló" con la URL, el
  mensaje y "Recargar", y las demás siguen (`window::tests::tab_panic_shows_error_page`, E6,
  Mac, 2026-11-12; criterio cumplido).
- **Seguridad**:
  - `Referrer-Policy` (`bf-net/src/referrer.rs`) y HSTS (`bf-net/src/hsts.rs`) (E6); COOP/COEP
    y `crossOriginIsolated` (E3, `bf-web/src/workers/isolation.rs`; el shell aún no pasa las
    cabeceras).
  - **CSP 3 y Mixed Content** (E6, en `master`;
    `bf-net/src/{csp,mixed_content,security}.rs`, `bf-web/src/security.rs`;
    `pendiente/mac/e6.md`, ronda 7 y "Ciclo de cierre", rondas 1 y 3):
    - CSP desde la cabecera y desde `<meta>`: `script-src` con nonce, hash y
      `'strict-dynamic'`, `connect-src`, `frame-src`, `'unsafe-eval'` y `Report-Only`.
    - Contenido mixto: lo activo `http:` en una página `https:` se bloquea y las imágenes
      pasan a `https:`.
    - `img-src` (CSP 3 §6.1.6) en los `<img>` del motor y en el pintado, y el evento
      `securitypolicyviolation` (§5.1, §5.3) con `SecurityPolicyViolationEvent`.
    - WPT `content-security-policy` y `mixed-content`: **231 de 1782** subtests (173 antes;
      la mayoría de los que faltan necesita cabeceras o subdominios de `wptserve`).
    - Falta de CSP: `style-src` y `script-src-attr` en línea; `report-uri`/`report-to`;
      `worker-src`; `media-src`; `connect-src` de `WebSocket` y `EventSource`;
      `frame-ancestors`, `sandbox`, `upgrade-insecure-requests` y `base-uri`; la política
      propia de los marcos y la de los workers.
  - Faltan Permissions Policy y Safe Browsing.
- **Interfaz**:
  - Pestañas, historial persistente, marcadores, descargas con su barra, buscar en la página,
    zoom de texto, opciones, varias ventanas y sesión restaurada (E11, en `master` desde
    `9d64fdd`; `bf-shell/src/{history,bookmarks,downloads,find,settings,restore}.rs`).
  - Atajos con `Cmd` en macOS (E6, `bf-shell/src/window/keys.rs`). Un pánico en el hilo de la
    ventana guarda la sesión antes de salir (E6, ronda 6).
  - **`Freedom.app`** firmada ad hoc con su `.dmg` (`packaging/macos/build.sh`, E6, en `master`
    desde `21ea345`; `pendiente/mac/e6.md`, ronda 7). El Finder todavía no le pasa el documento
    con «Abrir con» (`winit` no expone esos eventos).
  - Impresión (`@page`): falta.
- **Rendimiento** (método y tablas en [`rendimiento.md`](rendimiento.md)):
  - En `master`:
    - índice de reglas en la cascada, por pseudoelemento;
    - raster en tiles de la página, en paralelo y con desplazamiento incremental;
    - capas superpuestas compuestas solo en su caja (`render_over`);
    - la barra de la ventana guardada pintada;
    - el cuadro de la ventana reutilizado y los `sticky` en capa propia (E4, "Cierre, ronda
      3");
    - la cascada salta los `::before`/`::after` sin `content` (E4, "Cierre, ronda 3");
    - con una VPN que da una ruta IPv6 que no sale, IPv4 va primero (RFC 6724 §6 en Happy
      Eyeballs; E6, "Ciclo de cierre, ronda 5").
  - **Desplazamiento** (mediana por cuadro, meta 16.7 ms = 60 fps):
    - E4 (`pendiente/mac/e4.md`, "Cierre, ronda 3"), carga 15-36: **8.0 ms a densidad 1**
      (raster 5.5) y **17.7 ms a densidad 2** (raster 10.8). Antes, 20.9 y 59.1 ms.
    - El planificador sobre `0995c60`, carga 46-58: 12.6-15.3 ms a densidad 1 y 27.7-37.7 ms
      a densidad 2.
    - A densidad 1 cabe en 16.7 ms en las dos medidas; a densidad 2, no. Sin confirmar con
      la máquina libre: el criterio no se da por cumplido.
  - **Primera pintura de Wikipedia** (meta < 500 ms): sin medida sobre `master` con la
    máquina libre (la carga llegó a 160-210 al cerrar el ciclo). La última medida completa es
    de **823-936 ms** (5 corridas, mediana 868 ms, carga 25; E2, "Ciclo de cierre, ronda 6",
    en `git show 3dfeedf:docs/pendiente/mac/e2.md`): con la maqueta de las imágenes
    perezosas ya quitada, pero antes del arreglo de red de E6. Por partes, cada una de su
    fuente y con su carga:

    | Fase | ms | Fuente |
    |---|---:|---|
    | Red y caché | 401 (mediana; 711 antes) | E6, "Ciclo de cierre, ronda 5" |
    | Parseo | 56-60 (medianas; 281-307 antes) | E3, "Ciclo de cierre, ronda 5"; E2, "Ciclo de cierre, ronda 6" |
    | Cascada | 76 (mediana; 105 antes) | E4, "Cierre, ronda 3" |
    | Maqueta | 100-168 | E4, "Cierre, ronda 1" (168) y muestreo de "Cierre, ronda 3" (100) |

    Otras medidas completas, todas de antes de juntar los cambios: 1076 ms (E4, carga 17-20,
    sin los de E3 y E6), 1271 ms (E3, carga 13, sin el de E6) y 1495 ms (E6, carga 9-11, sin
    el de E3).
  - **Suelo de red**: el documento se valida en cada visita (`max-age=0, must-revalidate`) y
    un proceso nuevo no tiene conexión abierta. TCP, TLS 1.3 y el 304 cuestan un viaje de ida
    y vuelta cada uno, unos 115 ms en esta red: unos 345 ms antes de parsear (E6). Qué hacer
    con eso es una decisión pendiente.
  - Falta la GPU.

---

## Deuda técnica anotada en el código

Salida de `grep -rn "TODO(" crates` agrupada por crate. Cada `TODO(etiqueta)` cita la fase
que lo resuelve. La tabla de arriba es la de hoy (`master` `8917a5f`, 2026-11-12); la de
abajo, los `TODO` del cierre de la fase 1 que ya no están.

| Crate | Ubicación | Pendiente | Fase |
|---|---|---|---|
| `bf-dom` | `src/forms.rs:32` | `TODO(event-loop)`: la cola de microtasks de `option` es provisional; pasa al event loop real con `bf-js`. | 2.3 |
| `bf-layout` | `src/controls.rs:9` | `TODO(forms)`: interactividad de formularios y `::placeholder` (el scroll interno de `textarea` y list box ya está, E12). | 2.4 |
| `bf-net` | `src/lib.rs:9` | `TODO(roadmap)`: CORS y políticas de seguridad de Fetch (CORS está en `bf-web`, E2). | 2.3 / 5 |
| `bf-paint` | `src/builder.rs:744` | `TODO(backgrounds)`: `background-attachment: fixed` se pinta como `scroll`. | 3.3 |
| `bf-image` | `src/lib.rs:8` | `TODO(roadmap)`: AVIF (diseño en `docs/pendiente/e7.md`). | 4.2 |
| `bf-svg` | `src/lib.rs:27` | `TODO(svg)`: el resto de primitivas de filtro (`feImage`, `feTurbulence`, luces…). | 4.2 |
| `bf-style` | `src/cascade.rs:971` | `TODO(cascade)`: las reglas UA de quirks ganan por especificidad máxima; debería ser un origen "UA quirks" explícito. | 3 |
| `bf-style` | `src/values/color.rs:499` | `TODO(E2)`: `currentcolor` como argumento (de `color-mix()`) necesita un `Color` sin resolver. | 3 |

Ya no están: `TODO(bf-dom)` del tokenizador (`bf-html`) y de `DomString` (`bf-infra`),
`TODO(roadmap)` de encodings multibyte (`bf-infra/src/encoding.rs`, E10), `TODO(sticky)` y el
`TODO(E4)` de `transform` (`bf-layout/src/positioned.rs`), `TODO(http-cache)` del shell
(decisión, ver 4.1) y la parte de gradientes de `TODO(backgrounds)`.

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
- **`Image` y `PerformanceObserver` ausentes**: hecho (E3, `89a46c3`, en `master`): `Image`,
  `Audio` y `Option` (`bf-web/src/html/legacy_factories.rs`) y `PerformanceObserver`
  (`bf-web/src/bom/performance_observer.rs`).
- **`TypeError` sin mensaje**: hecho (E3 y E8): los errores dicen la operación y el valor
  (`describe()` y `HostState::current_op` en `bf-js/src/host/mod.rs`).

## Estado por bloque

Lo hecho, lo medido y lo que falta de cada bloque del ciclo army:

- [E1](pendiente/e1.md): Shadow DOM, custom elements, `XMLSerializer`, form-associated, nodos de
  otra ventana sin pánico.
- [E2](pendiente/e2.md): red desde JS (`fetch` por partes, CORS, streams), IndexedDB y CSSOM.
- [E3](pendiente/e3.md): workers, agentes, `BroadcastChannel` entre agentes, COOP/COEP.
- [E4](pendiente/e4.md): hilo por pestaña, BOM, iframes con pintado anidado, cookies de terceros;
  `transform`, animaciones, transiciones y `requestAnimationFrame` (integrados en el Mac).
- [E5](pendiente/e5.md): sizing, grid, subgrid, grid lanes y flexbox rtl.
- [E6](pendiente/e6.md): HTTP/2, caché HTTP, PSL, HSTS, Referrer-Policy, conexiones, autenticación
  y DNS.
- [E7](pendiente/e7.md): formatos de imagen propios, animación de GIF/APNG y SVG.
- [E8](pendiente/e8.md): test262 (95.3 % en Windows; 98.8 % en el Mac), Intl y rendimiento de la VM.
- [E9](pendiente/e9.md): bidi (UAX #9), cortes de línea (UAX #14) y shaping por script.
- [E10](pendiente/e10.md): encodings multibyte; namespaces y parser XML pendientes.
- [E11](pendiente/e11.md): historial, marcadores, descargas, buscar, zoom de texto, ventanas y
  sesión restaurada.
- [E12](pendiente/e12.md): scroll containers, sticky, `line-clamp`, `text-overflow`.
- En el Mac (rondas del 2026-11-12): [`pendiente/mac/`](pendiente/mac/), un archivo por
  ejecutor (E1 a E6).
