# Faltantes vistos en sitios reales

Lo que falla al navegar con Freedom por sitios reales, para priorizar el trabajo. Cada fila
dice qué se vio, dónde, y si quedó verificado con una página de prueba aislada
(`freedom session` sobre `master` en `08ff44e`, 2026-10-21) o si la causa todavía hay que
investigarla. Al día con `master` el 2026-11-17. El barrido de 27 sitios públicos del
2026-11-17, con lo que le falta a cada uno y las carencias ordenadas por número de sitios, está
en [`pendiente/mac/barrido-sitios.md`](pendiente/mac/barrido-sitios.md).

## Funciones que faltan (verificado)

| Falta | Síntoma en los sitios | Spec | Dónde va |
|---|---|---|---|
| `crypto.subtle`, salvo `digest` y HMAC | Desde el 2026-11-17 existe, con `digest` (SHA-1/256/384/512) y HMAC (E1). Lo demás (AES, RSA, ECDSA, `deriveKey`…) rechaza con `NotSupportedError`. | Web Cryptography API | 2.2 |
| `localStorage` en páginas `file:` | `SecurityError: The operation is insecure`. Chrome y Firefox lo permiten (origen opaco con almacenamiento propio por archivo o carpeta). | HTML §12.2 | 2.2, `bf-web` bom |

Ya funcionan (verificado en la misma prueba): eventos de clic al DOM, `fetch`,
`XMLHttpRequest`, `querySelectorAll`, `classList`, `innerHTML`, `document.currentScript`,
`location`, `history.pushState`, `MutationObserver` y `WebSocket` (al menos como interfaz).

Resueltos después, en `master` `8917a5f` (2026-11-12, comprobado en el código, no con los
sitios):
- Constructor `Image` (y `Audio` y `Option`): `bf-web/src/html/legacy_factories.rs` (E3).
- `indexedDB`: `bf-web/src/storage/idb.rs` (E2).
- `innerWidth`/`innerHeight` y `clientWidth` salen del viewport real que pasa el shell
  (`bf-web/src/cssom/mod.rs::window_metrics`).
- `customElements`: `bf-web/src/html/custom_elements.rs` (E1).
- `Worker` (y `SharedWorker`): `bf-web/src/workers/` (E3).

Resueltos el 2026-11-13, en `master` desde `1070d8e` (E2, `pendiente/mac/e2.md`, "Ciclo nuevo,
ronda 1"):
- **`IntersectionObserver`** (`bf-web/src/cssom/intersection_observer.rs`). Antes faltaba, y
  `asimetrix.co` caía en su pantalla de error. WPT `intersection-observer` 222 / 315
  (70.5 %) y `intersection-observer/v2` 30 / 65. Lo que falla:
  - `scrollMargin` no recorta los scroll containers intermedios (`scroll-margin-*`, 5 / 19);
  - los objetivos SVG (`svg-*`, 13 / 50), que no tienen fragmentos en la maqueta;
  - los `<iframe>` (2 / 26): la raíz implícita de un marco es el viewport de arriba;
  - `transform` y `zoom`, que `getBoundingClientRect` no aplica;
  - la v2 de visibilidad (`trackVisibility`, oclusión): `isVisible` es siempre `false`;
  - sueltos: `text-target` (un `<br>` no tiene caja), `observe-007/012/013/014` y
    `timestamp` (el tiempo de un observador de otro marco).
- **`ResizeObserver`** (`bf-web/src/cssom/resize_observer.rs`). WPT `resize-observer` 94 / 115
  (81.7 %). Falla `fragments` (un elemento con varios fragmentos).

Resueltos en el ciclo del Mac (2026-11-12 y 13, en `master`):
- **Carga de imágenes desde el documento y los scripts.** Los `<img>` siguen *update the image
  data* (HTML §4.8.4.3.5): `load` y `error`, `complete`, `decode()`, y retienen el `load` de la
  ventana mientras cargan. Asignar `src` desde un script dispara `onload`. También se disparan
  el `load` de `<link>` y el de `preload` (E2, `pendiente/mac/e2.md`, ronda 6;
  `bf-web/tests/img_load.rs`).
- **`loading=lazy`** (HTML §4.8.4.3.9), completo en JavaScript y en el pintado:
  - la imagen se pide cuando queda a menos de 1250 px del viewport, y cada imagen se
    decodifica una sola vez por proceso (E2, ronda 8);
  - la ventana las pide también al desplazar (E6, `pendiente/mac/e6.md`, "Ciclo de cierre,
    ronda 1");
  - el pintado no descarga las perezosas que esperan, y las que llegan las toma de la caché
    del loader de la ventana, sin descargarlas otra vez (E2, `pendiente/mac/e2.md`, "Ciclo de
    cierre, ronda 4");
  - en la ventana se revisan con la maqueta del navegador, no con una propia del motor (E3,
    `pendiente/mac/e3.md`, "Ciclo de cierre, ronda 5"): las cercanas aparecen un cuadro
    después de la primera pintura, como pide HTML §8.1.7.3.
  - una imagen **no** perezosa que piden a la vez el motor y el primer pintado se descarga
    una sola vez aunque la respuesta no sea cacheable (E6, en `master` desde `84e2dd0`).

Resueltos en la ronda 6 del 2026-11-17 (en `master` desde `3a7c372`):
- **E1:**
  - `SVGElement` y su jerarquía (GitLab, YouTube);
  - `DOMMatrix`, `DOMPoint` y `DOMQuad` (Apple);
  - `select.add` (documentación de Python);
  - un módulo con y sin `crossorigin` ya no se evalúa dos veces (NYT, "Custom element … is
    already defined");
  - `sessionStorage` sobrevive a `location.reload()`.
- **E2:**
  - `insertRule` y `adoptedStyleSheets` entran en la cascada que se pinta (la BBC sin estilos);
  - los `<svg>` sin tamaño ya no salen gigantes (la lupa de react.dev);
  - varios desbordes horizontales;
  - GitHub, de unos 4 s a menos de 1 en la maqueta.
- **E3:** el marco lento y el reto de Cloudflare (abajo).

En `master` desde `cb41001` (ronda 7):
- **Las teselas de OpenStreetMap salían con la imagen "418 Access blocked".**
  - Las imágenes iban con `Accept: */*`. Fetch §4 recomienda, para el destino `image`,
    `image/png,image/svg+xml,image/*;q=0.8,*/*;q=0.5`.
  - El servidor de teselas solo da la tesela a las peticiones con `Referer` de
    `openstreetmap.org` si piden una imagen así. Con ese `Accept`, las 30 teselas llegan.
  - El mapa ya se ve desde `eb9776d` (ronda 8 de E2): Leaflet pone `mix-blend-mode:
    plus-lighter` a las teselas, y un stacking context con algo que se mezcla ahora es un grupo
    aislado (Compositing and Blending 1 §3.2). Antes la tesela se sumaba al fondo y salía
    blanca.

Resueltos el 2026-11-17 en la ronda 6 de E3 (en `master` desde `3a7c372`):
- **Un `<iframe>` lento congelaba su página.** El hilo de la pestaña pedía, parseaba y ejecutaba
  el documento del marco de una vez: el de YouTube en `asimetrix.co` lo retenía 2,7 s, y el
  titular de la portada no se veía hasta ~3,3 s después de la primera pintura.
  - Ahora la red del marco va en otro hilo y su documento se parsea en varias tareas (HTML
    §7.4.2.2).
  - El bucle de la pestaña pinta como mucho cada 50 ms de tareas (HTML §8.1.7.3).
  - El titular se ve en la primera captura tras la primera pintura (E3).
- **El reto de Cloudflare no estaba bloqueado.** Stack Overflow lo supera en ~5,6 s, con un
  `XMLHttpRequest`, un `Worker`, `BroadcastChannel` y un `POST` del formulario, y luego carga.
  El barrido solo esperaba ~1 s. No le falta ninguna función estándar
  ([`pendiente/mac/barrido-sitios.md`](pendiente/mac/barrido-sitios.md)).

Resueltos el 2026-11-17 (en `master`):
- **Hojas enlazadas en la ventana.** `<link rel=stylesheet>` dispara `load` (HTML §4.6.7.2) y
  `document.styleSheets` tiene sus reglas; antes disparaba `error` (E2, `489e6f8`).
- **Pistas de presentación contra `@layer`.** Una regla de autor en una capa les gana (CSS
  Cascade 5 §6.5): el logo gigante de `asimetrix.co`. Las imágenes con `width` y `height`
  reservan su hueco con esa proporción antes de llegar (HTML §15.4.3) (E2, `489e6f8`).
- **`TypeError: Illegal invocation`** al quitar oyentes de `performance` y
  `screen.orientation` (E1, `1ee12fa`).
- **Un `ShadowRealm` tumbaba la pestaña** (`el realm no tiene WebContext`): ahora no tiene
  las interfaces del host. Una pestaña que falla dice por qué (E1, `1ee12fa` y `f9dddce`).
  - Falta: las perezosas dentro de un `<iframe>` y dentro de contenedores con `overflow`.
- **CSS Font Loading** (`document.fonts`, `FontFace`, `FontFaceSet`): antes `document.fonts`
  no existía y `document.fonts.ready` lanzaba `TypeError` (E5, `pendiente/mac/e5.md`, ronda 7;
  `bf-web/tests/font_loading.rs`).
- **El `load` de `<iframe>`** espera al `load` de su documento y retiene el del padre (HTML
  §7.4.6.3). Antes salía en cuanto se parseaba el marco (E2, ronda 8). Además se dispara tras
  `document.close()` y acepta URLs `blob:` (E3, `pendiente/mac/e3.md`, ronda 6).
- **Shadow roots en el adoption agency.** Un script que metía un nodo en un shadow root (o en
  el contenido de un `<template>`) durante el parseo podía hacer que el parser creara un ciclo
  y no terminara nunca. Ahora la comprobación sube del shadow root a su host (DOM §4.2.2) (E3,
  ronda 6; `bf-html/tests/robustness.rs`).
- **Pánicos `Gc liberado` al crear marcos**: `bf_web::register_all` corre dentro de
  `Agent::rooting_scope` (E3, ronda 6; `bf-web/tests/gc_rooting.rs`).
- **`pageshow`** tras el `load` de la ventana, un `PageTransitionEvent` con el documento como
  `target` (E3, `pendiente/mac/e3.md`, "Ciclo de cierre, ronda 2").
- **Accesibilidad: rol, nombre y estados accesibles y el árbol de accesibilidad** del
  documento (HTML-AAM, ARIA 1.2, AccName 1.2, Core-AAM), también a través de shadow roots y
  slots, con `aria-owns` y en orden de lectura. WPT `html-aam` 93.4 % y `wai-aria` 95.7 % (E1,
  `pendiente/mac/e1.md`, "Ciclo de cierre", rondas 1 y 3). **Sin exponer al sistema:**
  VoiceOver todavía no puede leer las páginas (decisión pendiente, `roadmap.md`, "Decisiones
  pendientes del usuario").
- **`:popover-open`**: los popovers cerrados ya no se pintan (E5, `pendiente/mac/e5.md`,
  "Ciclo de cierre, ronda 1").
- **Content Security Policy y contenido mixto** (E6, `pendiente/mac/e6.md`, ronda 7 y "Ciclo
  de cierre", rondas 1 y 3; `bf-net/src/{csp,mixed_content,security}.rs`):
  - CSP desde la cabecera y desde `<meta>`: `script-src` con nonce, hash y
    `'strict-dynamic'`, `connect-src` en `fetch` y XHR, `frame-src`, `'unsafe-eval'` y
    `Report-Only`.
  - En una página `https:` el contenido activo `http:` se bloquea y las imágenes pasan a
    `https:`.
  - `img-src` también en los `<img>` que pide el motor (antes solo el pintado la respetaba).
  - El evento `securitypolicyviolation` (`SecurityPolicyViolationEvent`, CSP 3 §5.1 y §5.3)
    en el elemento que hizo la petición, o en el documento.
  - Los bloqueos salen en la consola con la URL y la directiva. Wikipedia manda una CSP que se
    aplica, y la página sigue cargando.
  - WPT `content-security-policy` y `mixed-content`: 231 de 1782 subtests (173 antes).

## Partes de CSP que faltan

Lo que E6 dejó sin enganchar o sin implementar (`pendiente/mac/e6.md`, "Pendiente" de la ronda 7
y del "Ciclo de cierre", rondas 1 y 3). Una página que confíe en estas directivas no queda
protegida por ellas en Freedom:

| Falta | Detalle |
|---|---|
| `style-src` en línea | El chequeo existe en `csp.rs` (`Inline::Style`), pero la cascada no lo usa. |
| `script-src-attr` | Existe (`Inline::ScriptAttribute`), pero los manejadores `on*` no lo consultan. |
| Informes | Falta `report-uri`/`report-to`. El evento `securitypolicyviolation` sale con `sample`, `sourceFile`, `lineNumber` y `columnNumber` vacíos o en 0, y no sale en los workers (no tienen política). |
| Otras directivas | `worker-src`, `media-src` en `<video>`/`<audio>`, `connect-src` de `WebSocket` y `EventSource`, `frame-ancestors`, `sandbox`, `upgrade-insecure-requests` y `base-uri`. |
| Marcos | La política de un marco que navega dentro del mismo realm conserva la URL del primer documento. Las imágenes que pinta un marco pasan por la política del documento de arriba, no la suya, y el evento de `frame-src` va al documento, no al `<iframe>`. |

## Errores en sitios por investigar

| Sitio | Lo que se ve | Pista |
|---|---|---|
| excalidraw.com y gov.co | Desde la ronda 7, las fuentes ya no congelan la pestaña (E2). excalidraw llega a "Loading scene…" y ahí se queda. gov.co monta su estructura, pero el contenido principal queda vacío. | **gov.co no es de red** (E3, ronda 8): 88 respuestas, todas 200 o 204, con CORS correcto, preflights que pasan y JSON válido. "Error al cargar banner informativo" es un `console.log` del sitio. El único error es `TypeError: Cannot read properties of null (reading 'datoconsultadousuario')`, de JavaScript (E1). excalidraw: sin diagnosticar tras el arreglo de las fuentes. |
| www.openstreetmap.org | El mapa se ve desde `eb9776d` (arriba). Los iconos de los controles del mapa salen como "?". | Sin diagnosticar: probablemente las imágenes o el SVG de los iconos de Leaflet (E2). |
| www.nytimes.com | Desde el segundo barrido responde 403 con la verificación de DataDome (`captcha-delivery.com`), también a `curl`, con cualquier `User-Agent`. | Es la reputación de la IP tras muchas cargas automáticas, no el motor. El marco de verificación carga y manda su `POST` (200), pero la portada no se libera en ~7 s y se ve en blanco. Lo que muestra ese marco después del `POST` no está visto. |
| Google (búsquedas) | Responde `429 Too Many Requests` y redirige a `/sorry` con reCAPTCHA; el reCAPTCHA falla con `TypeError: Type error.` dentro de `recaptcha__es_419.js`. Buscar desde la barra queda inutilizable. | Google es el buscador por defecto: es lo más visible para quien usa Freedom. Ver si el 429 depende del `User-Agent` nuevo, de las cabeceras (`Sec-CH-UA`, `Accept`) o del ritmo de peticiones, y qué operación falla en el reCAPTCHA. |
| Aplicación de `app.asimetrix.co` (`/ASIMETRIX`, build `ce1e596`, con sesión iniciada) | La pestaña se cae: "Esta página falló", con el detalle `el realm no tiene WebContext`. En la consola, `ChunkLoadError: Loading CSS chunk 9298 failed`, `TypeError: Illegal invocation` y dos promesas rechazadas sin manejar. | Sin sesión no se reproduce la caída ni el `ChunkLoadError`. Arreglados desde entonces: el `Illegal invocation`, un `ShadowRealm` que tumbaba la pestaña con ese mismo mensaje y las hojas enlazadas que disparaban `error` (ver arriba). Falta que el usuario lo pruebe con su sesión (`RETOMAR-EN-MAC.md`, sección 9b). |
| Aplicación Next.js (asimetrix.co) | `TypeError: Cannot read properties of undefined (reading 'dplId')` al registrar los chunks de Turbopack. | Algún global o propiedad que el runtime de Next.js espera y no existe. |
| Aplicación Vite (emmavet.com.co) | `TypeError: Type error.` en la línea 1 del módulo principal: la aplicación no arranca. | Falla al evaluar un módulo ES; el mensaje no dice qué operación. |
| Sitio de videos | `TypeError: Cannot read properties of undefined (reading 'protocol')` en su script de cabecera. | Algo devuelve `undefined` donde el sitio espera un objeto con `protocol` (¿`<a>.protocol`, `URL` de un recurso, `location` de un iframe?). |
| Analítica de Ahrefs | `TypeError: Failed to construct 'URL': Invalid URL 'undefined'`. | `new URL(undefined)` falla bien por spec: el problema es lo que el script lee antes (probablemente `document.currentScript` en scripts insertados por otro script, o su `src`). |

## Mensajes de error

Resuelto (E3 y E8, en `master`): los `TypeError` dicen la operación y el valor, como en los
demás motores (`x is not a function`, `Cannot read properties of undefined (reading 'y')`).
Antes `bf-js` lanzaba `TypeError: Type error.` (reCAPTCHA y la aplicación Vite); hay que volver
a probar esos sitios para ver el mensaje real.

## Pruebas que fallan en `master`

- Ninguna: `master` (`489e6f8`) pasa la verificación completa en el Mac: formato, clippy y
  141 grupos de tests (informe del planificador del 2026-11-17, ronda 4).
