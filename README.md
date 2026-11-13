# Freedom

<img src="crates/bf-shell/assets/mark-256.png" width="96" alt="Logo de Freedom" align="right">

**Navega libre.** Freedom es un navegador web independiente: no está construido sobre Chrome,
Firefox ni Safari, sino sobre un motor propio, hecho desde cero.

> **Vista previa.** Freedom ya abre y muestra páginas web, y su JavaScript está en
> construcción: ejecuta los scripts de las páginas, pero todavía faltan muchas de las funciones
> que usan los sitios. Los sitios para leer (Wikipedia, blogs, noticias, documentación) se ven
> bien; los muy interactivos (redes sociales, correo web, mapas, tiendas) todavía no funcionan
> o se ven incompletos.

## Instalar Freedom

Descarga el instalador para tu sistema (Windows, macOS o Linux) desde
[Releases](https://github.com/ramirezjhulian7/browser-freedom/releases); cada release explica
cómo instalarlo.

También puedes compilarlo desde el código fuente, en cuatro pasos:

1. Instala Rust con el instalador oficial de [rustup.rs](https://rustup.rs) (en Windows,
   acepta las opciones por defecto).
2. Descarga Freedom: en [GitHub](https://github.com/ramirezjhulian7/browser-freedom), «Code →
   Download ZIP» y descomprímelo, o con git:

   ```sh
   git clone https://github.com/ramirezjhulian7/browser-freedom.git
   ```

3. Abre una terminal en esa carpeta y compílalo (tarda unos minutos la primera vez):

   ```sh
   cargo build --release -p bf-shell
   ```

4. Abre Freedom: es `target/release/freedom` (en Windows, `target\release\freedom.exe`; puedes
   abrirlo con doble clic y anclarlo a la barra de tareas).

Para abrir una página o un archivo directamente: `freedom https://es.wikipedia.org` o
`freedom pagina.html`.

## Cómo se usa

- **Ir a un sitio o buscar:** escribe en la barra de arriba (o directamente en la página de
  inicio) y presiona `Intro`. Si lo que escribes no parece una dirección, Freedom lo busca en
  Google.
- **En macOS** los atajos usan `Cmd` (⌘) donde Windows y Linux usan `Ctrl`: `⌘T`, `⌘C`,
  `⌘L`… La tabla de abajo tiene las dos columnas.
- **Pestañas:** `Ctrl+T` abre una y `Ctrl+W` la cierra. El clic central o `Ctrl+clic` en un
  enlace lo abre en otra pestaña.
- **Copiar y pegar:** arrastra para seleccionar texto; doble clic selecciona una palabra y
  triple clic un párrafo. `Ctrl+C` copia y `Ctrl+V` pega, también en la barra de direcciones y
  en los formularios. El clic derecho muestra más opciones (copiar enlaces, guardar imágenes).
- **Zoom:** `Ctrl` `+`, `Ctrl` `−` y `Ctrl` `0`, o `Ctrl` + rueda del ratón.
- **Tema claro u oscuro:** Freedom sigue el de tu sistema.

| Windows y Linux | macOS | Qué hace |
|---|---|---|
| `Ctrl+L` | `⌘L` | Ir a la barra de direcciones |
| `Ctrl+T` · `Ctrl+W` | `⌘T` · `⌘W` | Pestaña nueva · cerrar pestaña |
| `Ctrl+N` | `⌘N` | Ventana nueva |
| `Ctrl+Tab` · `Ctrl+1…9` | `⌃Tab` · `⌘1…9` | Cambiar de pestaña |
| `Alt+←` · `Alt+→` | `⌘[` · `⌘]` | Atrás · adelante |
| `Ctrl+R` · `F5` | `⌘R` · `F5` | Recargar |
| `Esc` | `Esc` | Detener la carga |
| `Alt+Inicio` | `⇧⌘H` | Página de inicio |
| `Ctrl+H` · `Ctrl+J` | `⌘Y` · `⌘J` | Historial · descargas |
| `Ctrl+D` · `Ctrl+Shift+B` | `⌘D` · `⇧⌘B` | Guardar marcador · ver los marcadores |
| `Ctrl+F` | `⌘F` | Buscar en la página |
| `Ctrl++` · `Ctrl+−` · `Ctrl+0` | `⌘+` · `⌘−` · `⌘0` | Acercar · alejar · tamaño normal |
| `Ctrl+A` · `Ctrl+C` · `Ctrl+V` | `⌘A` · `⌘C` · `⌘V` | Seleccionar todo · copiar · pegar |
| `Espacio` · `Mayús+Espacio` | `Espacio` · `⇧+Espacio` | Bajar · subir una pantalla |

En macOS el historial va en `⌘Y` porque `⌘H` oculta la aplicación (y `⌘Q` la cierra), como en
cualquier programa del Mac.

### Tu privacidad

- El historial se conserva al cerrar Freedom. Puedes borrarlo desde la página del historial
  (`Ctrl+H`, `⌘Y` en macOS): la última hora, el día de hoy o todo.
- Las cookies de sesión se borran al cerrar. Las cookies persistentes (las que el sitio guarda
  con fecha de caducidad) se conservan hasta que caducan, como en cualquier navegador, así que
  algunas sesiones iniciadas en los sitios siguen abiertas al volver.
- Freedom guarda en tu equipo, como cualquier navegador:
  - en la carpeta del perfil:
    - el historial, los marcadores, las descargas, las opciones y las ventanas y pestañas
      abiertas, que vuelven a abrirse al arrancar (se puede desactivar en las opciones);
    - las cookies persistentes y la lista de sitios que exigen HTTPS (HSTS), en
      `cookies.txt` y `hsts.txt`;
  - la caché de páginas e imágenes descargadas (hasta 512 MB);
  - el almacenamiento local de los sitios (`localStorage` e IndexedDB).

  Las carpetas de cada sistema están en [Plataformas](#plataformas).

  Para borrar todo eso, elimina esas carpetas con Freedom cerrado. Fuera de eso, Freedom solo
  escribe archivos cuando tú lo pides, como al guardar una imagen o descargar un archivo.
- No envía datos de uso: solo se conecta a los sitios que visitas (y a Google cuando buscas).
- Se presenta ante los sitios como lo que es, Freedom: no se hace pasar por Chrome ni por
  otro navegador.

### Qué funciona y qué todavía no

- **Funciona:**
  - **Páginas:** HTML y CSS, con su texto, tablas y la mayoría de los diseños, incluido el
    texto de derecha a izquierda (árabe, hebreo) y el vertical (japonés, chino).
  - **Imágenes:** PNG, JPEG, GIF y APNG animados, WebP, BMP, ICO y SVG.
  - **Movimiento:** animaciones, transiciones y transformaciones de CSS.
  - **Scripts:** los de las páginas, con buena parte de las funciones del navegador que usan.
  - **Navegación:** enlaces, pestañas y varias ventanas, atrás y adelante, buscar en la
    página, zoom, y copiar y pegar.
  - **Formularios:** buscar, escribir y enviar.
  - **Lo que se guarda:** historial, marcadores, descargas de archivos y abrir otra vez las
    pestañas de la última vez.
  - **Seguridad:** las páginas `https` no cargan scripts ni otro contenido activo por `http`
    (contenido mixto), y se respetan las reglas de Content Security Policy que manda el sitio
    sobre qué scripts, imágenes, conexiones y marcos puede usar.
  - **En macOS:** se instala como `Freedom.app`.
- **Todavía no:**
  - parte de las funciones de JavaScript que usan los sitios;
  - video y audio (no se reproducen);
  - imágenes AVIF;
  - imprimir;
  - lectores de pantalla: Freedom calcula qué es cada elemento y cómo se llama para la
    accesibilidad, pero todavía no se lo pasa al sistema, así que VoiceOver no puede leer las
    páginas.

  El plan está en
  [`docs/roadmap.md`](docs/roadmap.md).

### ¿Algo no funciona?

Abre un reporte en
[GitHub Issues](https://github.com/ramirezjhulian7/browser-freedom/issues) con la dirección
de la página y lo que esperabas ver. Si apareció una página de error, copia el detalle que
muestra.

---

## Para desarrolladores

Freedom tiene un motor 100% propio, escrito en Rust. No usa Chromium, WebKit, Gecko ni Servo:
HTML, DOM, CSS, layout, pintura, URLs, red y JavaScript están escritos desde cero. Cada subsistema sigue su especificación (WHATWG, W3C, IETF, Unicode, ECMA) al pie de la
letra, y la compatibilidad se mide con las suites de pruebas oficiales.

El repositorio y los crates conservan el prefijo histórico `bf-` (de *browser-freedom*); el
ejecutable se llama `freedom` y es el binario del paquete `bf-shell`.

La **fase 1** está cerrada: el navegador carga y muestra páginas reales sin JavaScript (Google,
Wikipedia, Hacker News, example.com) en una ventana navegable con barra de URL, enlaces,
historial, scroll y zoom.

Las **fases 2 a 4** cumplen 43 de sus 45 criterios. Faltan dos:
- `html/syntax/parsing` sin ningún fallo; está en 99.9 %.
- Los reftests de css-flexbox con palabras clave de tamaño; pasan 86 de 108.

Lo que ya está:
- El motor de JavaScript (`bf-js`) pasa el 98.8 % de test262 y ejecuta los scripts de las
  páginas en un hilo por pestaña, con workers.
- Las interfaces del DOM (con Shadow DOM y custom elements), de HTML, del navegador, de red y
  del CSSOM (`bf-web`), con la carga de imágenes y CSS Font Loading.
- Tablas, grid, tamaños intrínsecos, escritura vertical, bidi, `transform`, transiciones y
  animaciones de CSS.
- HTTP/2 con caché HTTP en disco, SVG y WebP, y scroll containers con `sticky`.

La **fase 5** (accesibilidad, procesos con sandbox y rendimiento) está a medias:
- el rol, el nombre y los estados de cada elemento y el árbol de accesibilidad ya se
  calculan, pero no se exponen al sistema;
- CSP (también `img-src` y el evento `securitypolicyviolation`) y contenido mixto ya se
  aplican;
- desplazar Wikipedia cuesta 8.0 ms por cuadro a densidad 1 y 17.7 ms a densidad 2 (meta:
  16.7 ms, 60 fps), sin confirmar con la máquina libre;
- faltan los procesos separados con sandbox.

Las cifras son del Mac. Lo hecho y lo que falta, apartado por apartado, está en
[`docs/roadmap.md`](docs/roadmap.md) (tabla "Avance por criterios").

## Estado por subsistema

La última columna dice de dónde sale cada cifra. Las de WPT y la ejecución de test262 son del
Mac (macOS, Apple M4, desde el 2026-11-12); las de Windows quedan como histórico en esos mismos
archivos. Las suites de los parsers (tokenizador, CSS Syntax, URL, test262-parser-tests y
test262 solo parsing) corren dentro de `cargo test --workspace`, que está en verde en `master`.
Sus recuentos son los de la última corrida con resumen y no se han vuelto a anotar en el Mac.

| Subsistema | Spec | Suite oficial | Resultado | Fuente |
|---|---|---|---|---|
| Tokenizador HTML | HTML §13.2.5 | html5lib-tests (tokenizer) | **7045** pasan, 0 fallan (3 *stale*) | `cargo test -p bf-html --test html5lib_tokenizer` |
| Tree construction | HTML §13.2.6, §13.4 | árboles de WPT `html/syntax/parsing`, sin JavaScript | **3900** pasan, 0 fallan (4 omitidos) | [`docs/wpt-status.md`](docs/wpt-status.md), "E3, ronda 6" |
| Parser con scripts | HTML §13.2, `document.write` | WPT `html/syntax/parsing` (testharness.js) | **8155 / 8166** subtests (99.9 %) | [`docs/roadmap.md`](docs/roadmap.md), 2.3 |
| CSS Syntax | CSS Syntax 3 | css-parsing-tests | **253** pasan, 0 fallan (24 *stale*) | `cargo test -p bf-css --test css_parsing_tests` |
| URL e IDNA | URL Standard, UTS #46 | WPT `url/resources` | **1444** pasan, 0 fallan; los 7 casos de `percent-encoding.json` también pasan (Mac) | `cargo test -p bf-url --test wpt_url`; [`docs/roadmap.md`](docs/roadmap.md), 3.5 |
| Layout y pintura | CSS 2.2, Backgrounds 3, Flexbox 1, Grid 2, Tables 3, Transforms, Writing Modes, SVG 2… | WPT reftests (30 grupos) | **9935 / 12 548 ejecutables (79.2 %)** | [`docs/wpt-status.md`](docs/wpt-status.md), "Reftests en macOS" |
| Reftests con JavaScript | — | WPT reftests `needs-js` de la fase 1 | **93.1 %** de los no manuales se ejecuta | [`docs/roadmap.md`](docs/roadmap.md), 2.3 |
| APIs web | DOM, Shadow DOM, HTML, CSSOM, Fetch, Encoding | WPT testharness.js | `html/dom` **97.2 %**, `dom/nodes` **92.8 %**, `dom/events` **80.2 %**, `css/cssom` **81.6 %**, `fetch/api/basic` **89.4 %**, `encoding` **97.5 %** | [`docs/roadmap.md`](docs/roadmap.md), 2.2, 2.3 y 3.5 |
| Parser de JavaScript | ECMA-262 2026 §12–16, Annex B | test262-parser-tests | **3332** pasan, 0 fallan (48 *stale*) | `cargo test -p bf-js --test parser_tests` |
| Parser de JavaScript | ECMA-262 2026 | test262 (solo parsing) | **51 490** pasan, 0 fallan (527 omitidos: propuestas) | [`docs/test262-status.md`](docs/test262-status.md) |
| Accesibilidad | HTML-AAM, WAI-ARIA 1.2, AccName 1.2, Core-AAM | WPT `html-aam`, `wai-aria` | **93.4 %** (830 / 889) y **95.7 %** (563 / 588); con el árbol de accesibilidad, que no se expone al sistema | `docs/pendiente/mac/e1.md`, "Ciclo de cierre, ronda 3" |
| Seguridad | CSP 3, Mixed Content 2 | WPT `content-security-policy`, `mixed-content` | **231 / 1782** subtests (con `img-src` y `securitypolicyviolation`); casi todos los demás necesitan cabeceras o subdominios de `wptserve` | `docs/pendiente/mac/e6.md`, "Ciclo de cierre, ronda 3" |
| Motor JavaScript | ECMA-262 2026 | test262 (ejecución) | `language/` **99.95 %**, `built-ins/` **99.99 %**; total **98.8 %** (50 594 / 51 220) | [`docs/test262-status.md`](docs/test262-status.md); `built-ins/` en [`docs/roadmap.md`](docs/roadmap.md), 2.1 |

- *stale*: casos cuya expectativa quedó desactualizada respecto del spec vigente. Se listan en
  cada test con el motivo.
- *omitidos*: casos de scripting o de encodings multibyte.
- Lo que no pasa de `encoding/legacy-mb-*` son los tests que cargan un `<iframe>` o envían un
  formulario a uno; por eso la cifra de `encoding` se da sin ellos.

El detalle de los reftests por directorio, con la cifra de Windows al lado, está en
[`docs/wpt-status.md`](docs/wpt-status.md), sección "Reftests en macOS". Mediciones posteriores
a esa tabla, ya en `master`:
- `css/css-flexbox` 780 / 949 (E5, ronda 6, `docs/pendiente/mac/e5-flexbox-subconjuntos.md`);
- `css/css-backgrounds` 581 / 615, 94.5 % (E2, ronda 5, `docs/pendiente/mac/e2.md`).

### Qué implementa cada crate

- **`bf-url`**:
  - URL Standard completo: la máquina de estados con *state override*, los setters de la API
    `URL`, serialización y origin;
  - hosts IPv4 (con formas legacy), IPv6 y opacos;
  - dominios con UTS #46 (CheckBidi, CheckJoiners) y Punycode propios;
  - `application/x-www-form-urlencoded`, `URLSearchParams` y `data:` URLs.
- **`bf-infra`**:
  - `ResourceLoader` por `Url` y `FileLoader` (`file:`, `data:` y prefijos montados);
  - WHATWG Encoding: 228 etiquetas, BOM, UTF-8, UTF-16 y los 28 single-byte. Los multibyte
    se reconocen y se decodifican como windows-1252 (fase 3).
  - `identity`: cómo se presenta Freedom a los sitios (`User-Agent`, Client Hints,
    `navigator.platform`); ver [Identidad](CONTRIBUTING.md#identidad-del-navegador).
- **`bf-net`**:
  - HTTP/1.1 sobre TCP y TLS (rustls, webpki-roots, SNI, ALPN `http/1.1`), con timeouts de
    10 s (conexión) y 30 s (lectura) y pool keep-alive;
  - cuerpos con `Content-Length`, chunked y hasta EOF; gzip, deflate y br;
  - redirecciones de Fetch (hasta 20) y cookies RFC 6265bis en memoria;
  - detección del encoding de documentos: BOM → `Content-Type` → prescan de `<meta>` →
    windows-1252.
- **`bf-html`**:
  - tokenizador (§13.2.5) y tree builder (§13.2.6, los 21 insertion modes, contenido foráneo);
  - parseo de fragmentos (§13.4);
  - parser XML para XHTML.
- **`bf-dom`**: arena de nodos, mutaciones y el comportamiento de
  `select`/`option`/`selectedcontent` (selectedness, opciones, etiquetas).
- **`bf-css`**:
  - CSS Syntax 3 (con la gramática de nesting) y Selectors 4 (parser, especificidad, matching);
  - Media Queries 4;
  - el modelo de hojas: `@media`, `@supports`, `@layer`, `@import`, `@namespace`,
    `@font-face`, `@keyframes` y `@page`.
- **`bf-style`**:
  - cascada (Cascade 5 con capas, `revert` y `revert-layer`) y custom properties con `var()`;
  - CSS Values 4 (`calc()`, `min()`, `max()`, `clamp()`) y CSS Color 4 (incluido
    `light-dark()`);
  - un registry único de propiedades;
  - la hoja UA transcrita de HTML §15 y los presentational hints;
  - `<link rel=stylesheet>` y `@import`.
- **`bf-font`**:
  - fuentes del sistema (Windows, Linux, macOS) y `@font-face` (`url()`, `local()`,
    `unicode-range`);
  - matching de CSS Fonts 4 §5, fallback por carácter, shaping con `rustybuzz` y contornos.
- **`bf-image`**: PNG, JPEG y GIF (primer cuadro) con sniffing de firmas.
- **`bf-layout`**:
  - árbol de cajas; BFC con colapso de márgenes y `clear`; floats;
  - inline: quiebre de líneas, `vertical-align`, `white-space`, `text-align`;
  - posicionamiento `relative`, `absolute` y `fixed`;
  - reemplazados con relación de aspecto;
  - Flexbox 1 §9 completo;
  - controles de formulario de HTML §15.5 y `fieldset`/`legend` (§15.3.13);
  - `align-content` en block containers.
- **`bf-paint`**:
  - lista de visualización según CSS 2.2 Apéndice E: stacking contexts, `opacity`, recortes,
    `position: fixed` fuera del scroll;
  - bordes con todos los estilos y radios, decoraciones de texto;
  - imágenes y capas de `background-image`;
  - partes nativas de los controles;
  - rasterizado con `tiny-skia` y el harness de reftests de WPT.
- **`bf-shell`**: el navegador (ejecutable `freedom`). Un hilo por pestaña carga, ejecuta los
  scripts y maqueta; la ventana, cuya interfaz dibuja el propio motor, muestra el resultado:
  panel del motor, superposición de cajas y páginas `about:` (ver [La interfaz](#la-interfaz)).
- **`bf-js`** (fase 2, en curso): el motor de ECMAScript 2026:
  - lexer (§12) con identificadores Unicode, números correctamente redondeados, templates y
    re-escaneo de regex dirigido por el parser;
  - parser con las cover grammars, ASI, Annex B y todos los early errors;
  - validador de la sintaxis de RegExp (§22.2.1, modos `u` y `v`);
  - AST con la forma de ESTree (`bf-js dump-ast archivo.js [--module]`);
  - `JsString` UTF-16;
  - compilador a bytecode con resolución de ámbitos, VM de pila y heap con GC mark-sweep
    sobre arenas indexadas (raíces explícitas y `HandleScope`);
  - built-ins: Object, Function, Boolean, Symbol, Error y los NativeError, Number, Math,
    String (la ruta sin RegExp), Array, Iterator (con los iterator helpers, `concat`, `zip`,
    `chunks`/`windows`), JSON, Reflect, Proxy y las funciones globales (`parseInt`,
    `encodeURI`, `escape` de Annex B…);
  - módulos ECMAScript (§16.2): Source Text Module Records, ResolveExport con ciclos y
    ambigüedad, Link y Evaluate (Tarjan del spec), bindings de import vivos, el namespace
    exótico, `export * as ns`, módulos JSON (`with { type: "json" }`), `import.meta`,
    `import()` dinámico (también desde scripts y `eval`) y top-level await;
  - `bf-js run archivo.js [--module]` ejecuta un script o un módulo (con sus imports desde
    el sistema de archivos) con `print`.
- **`bf-web`** (fase 2, en curso): el puente entre `bf-js` y la plataforma web. Las interfaces
  WebIDL del DOM, de HTML, del BOM (`navigator`, `location`, `history`, storage…) y del CSSOM
  como clases host de JavaScript, más el runner de testharness.js de WPT.

## Arquitectura

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/arquitectura-dark.png">
  <img src="docs/arquitectura-light.png" alt="Arquitectura de Freedom: la ventana manda la URL al hilo de la pestaña, que la carga (bf-net), la parsea (bf-html) ejecutando los scripts (bf-js y bf-web), calcula estilos (bf-css y bf-style), maqueta (bf-layout, con bf-font y bf-image) y pinta (bf-paint); la lista de visualización vuelve a la ventana.">
</picture>

La versión interactiva, con zoom, búsqueda, recorridos guiados («Cargar una página» y
«JavaScript») y enlaces al código de cada parte, es
[`docs/arquitectura.html`](docs/arquitectura.html): descárgala y ábrela en un navegador. Se
genera desde [`docs/arquitectura.json`](docs/arquitectura.json) con asimetrix-arq-generator
y la identidad de Freedom (ver [`tools/arquitectura/`](tools/arquitectura/README.md)).

Freedom trabaja con **dos clases de hilos**:

- **La interfaz** (`bf-shell`, `window/`): la ventana, el cromo, la entrada del usuario y el
  raster de la lista de visualización. Nunca espera a la red ni a los scripts.
- **Un hilo por pestaña** (`window/worker.rs`): dueño del documento y de su motor de
  JavaScript (`script.rs`). Carga, parsea ejecutando los scripts, calcula estilos, maqueta y
  arma la lista de visualización; corre los temporizadores y, si los scripts cambian el DOM,
  vuelve a maquetar (como mucho un cuadro cada 16 ms). Los clics y las teclas le llegan como
  eventos del DOM (`window/events.rs`).

El flujo de una página:
1. La ventana manda la URL al hilo de la pestaña.
2. `bf-net` la resuelve con `bf-url`, hace el fetch (HTTP/1.1, TLS) y elige el encoding.
3. `bf-html` parsea a un árbol `bf-dom`. Cada `<script>` lo ejecuta `bf-js` con las
   interfaces de `bf-web` (DOM, BOM, CSSOM); `localStorage` se guarda por origen en disco.
4. `bf-style` carga las hojas (UA, autor, `@import`) con `bf-css` y calcula los valores
   computados de cada elemento.
5. `bf-layout` construye el árbol de cajas y produce un árbol de fragmentos con geometría
   absoluta. Para eso usa `bf-font` (métricas y glifos) y los tamaños de `bf-image`.
6. `bf-paint` lo convierte en una lista de visualización, que vuelve a la ventana: ahí se
   rasteriza y se compone con el cromo.

### Dependencias entre crates

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/dependencias-dark.png">
  <img src="docs/dependencias-light.png" alt="Dependencias entre crates: bf-shell usa bf-paint y bf-web; bf-paint usa bf-font, bf-html y bf-image; bf-font usa bf-layout, que usa bf-style, que usa bf-css, que usa bf-dom; bf-html usa bf-dom; bf-web usa bf-html, bf-layout, bf-js y bf-net. bf-infra y bf-url los usan casi todos.">
</picture>

Cada flecha va de un crate al que usa directamente, y solo se muestran las dependencias
necesarias: si A usa B y B usa C, la flecha de A a C se omite. `bf-infra` y `bf-url` los usan
casi todos y van aparte. Versión interactiva, con enlaces al `Cargo.toml` de cada crate:
[`docs/dependencias.html`](docs/dependencias.html) (se genera desde
[`docs/dependencias.json`](docs/dependencias.json), igual que el diagrama de arquitectura).

## Reglas del proyecto

- **Motor 100% propio.** Está prohibido depender de:
  - Chromium/Blink, WebKit, Gecko, Servo y sus crates (`html5ever`, `cssparser`,
    `selectors`, `style`, `script`, `webrender`);
  - motores JavaScript (`v8`, `boa`, `deno_core`, `rquickjs`, `quickjs`, SpiderMonkey, JSC);
  - `reqwest`, `hyper`, `ureq`, `url`, `idna`, `image`, `encoding_rs`, `taffy` y cualquier
    crate que implemente parsing de HTML/CSS/JS, URLs, encodings o layout;
  - `fontdb`, `font-kit`, `cosmic-text` y `skia-safe`: la base de fuentes, el matching, la
    lista de visualización y el pipeline de pintura son propios.
- **Crates de plataforma permitidos** (reemplazables):
  - ventana y presentación: `winit`, `softbuffer` (`wgpu` también permitido);
  - raster: `tiny-skia`;
  - TLS: `rustls` (con `ring` como proveedor criptográfico), `webpki-roots`,
    `rustls-pki-types`;
  - descompresión HTTP: `flate2`, `brotli`;
  - códecs de imagen: `png`, `zune-jpeg`, `gif`;
  - NFC para IDNA: `unicode-normalization`;
  - portapapeles y diálogos de archivo: `arboard`, `rfd`;
  - fuentes: `ttf-parser`, `rustybuzz`;
  - utilidades: `serde`/`serde_json` (solo tests y configuración), `tokio`, `thiserror`, `log`.

  Hoy se usan todos menos `wgpu`, `tokio`, `serde`, `thiserror` y `log` (`serde_json`, solo
  en los tests).
- **La compatibilidad se mide con suites oficiales**: html5lib-tests, web-platform-tests y
  (fase 2) test262. Ninguna funcionalidad se da por terminada sin su suite.
- **Spec literal**: el código sigue el texto del spec y cita la sección. Si un test contradice
  el spec vigente, se documenta en [`docs/wpt-status.md`](docs/wpt-status.md); no se agrega a
  una lista de omitidos sin explicación.
- `#![forbid(unsafe_code)]` en todos los crates.
- **Identidad propia.** Freedom da a los sitios los mismos datos que Chrome (sistema,
  arquitectura, idiomas, Client Hints), pero se identifica como Freedom, producto y motor: sin
  `AppleWebKit`, `Chrome` ni `Safari` en el `User-Agent`, sin marca `Chromium`, sin
  `vendor` de Google. Todo sale de `bf_infra::identity` (ver
  [`CONTRIBUTING.md`](CONTRIBUTING.md#identidad-del-navegador)).

Ver [`CONTRIBUTING.md`](CONTRIBUTING.md) para el flujo de trabajo.

Los instaladores y el proceso de release están en [`packaging/`](packaging/README.md).

## Compilar

Requiere Rust estable (edición 2024) y Python 3 solo para regenerar tablas.

```sh
cargo build --workspace --release
```

El ejecutable queda en `target/release/freedom` (`freedom.exe` en Windows).

## Plataformas

Freedom compila y funciona en Windows, macOS y Linux con el mismo código; lo que cambia es
dónde guarda sus datos, los atajos y las fuentes.

| Qué | Windows | macOS | Linux |
|---|---|---|---|
| Perfil: historial, marcadores, descargas, opciones, sesión, cookies, HSTS | `%LOCALAPPDATA%\Freedom` | `~/Library/Application Support/Freedom` | `$XDG_DATA_HOME/freedom` o `~/.local/share/freedom` |
| Caché HTTP | `%LOCALAPPDATA%\Freedom\cache` | `~/Library/Caches/Freedom` | `$XDG_CACHE_HOME/freedom` o `~/.cache/freedom` |
| `localStorage` · IndexedDB | `%APPDATA%\freedom\{storage,idb}` | `~/Library/Application Support/Freedom/{storage,idb}` | `~/.freedom/{storage,idb}` |
| Descargas y capturas | `%USERPROFILE%\Downloads` | `~/Downloads` | `XDG_DOWNLOAD_DIR` o `~/Downloads` |
| "Ver en carpeta" | Explorador, con el archivo seleccionado | Finder, con el archivo seleccionado (`open -R`) | la carpeta (`xdg-open`) |
| Fuentes | `%WINDIR%\Fonts` y las del usuario | `/System/Library/Fonts` (con `Supplemental`), `/Library/Fonts`, `~/Library/Fonts` | `/usr/share/fonts`, `/usr/local/share/fonts`, `~/.fonts`, `~/.local/share/fonts` |

`FREEDOM_PROFILE` cambia la carpeta del perfil (también la de las cookies y HSTS) y
`FREEDOM_DOWNLOADS` la de descargas, en cualquier sistema.

**Atajos.** El modificador primario es `Cmd` (⌘) en macOS y `Ctrl` en Windows y Linux; la tabla
está en `crates/bf-shell/src/window/keys.rs` y los menús y las páginas internas escriben cada
atajo como se usa en el sistema (`Ctrl+D` o `⌘D`). `Ctrl+Tab` cambia de pestaña en todos. En
macOS `⌘H` y `⌘Q` son del sistema (ocultar y salir), por eso el historial es `⌘Y`, y atrás y
adelante son `⌘[` y `⌘]` (`⌥←`/`⌥→` mueven el cursor por palabras). En las sesiones sin
ventana, `key:mod+t` usa el primario del sistema (`key:ctrl+…` y `key:cmd+…` son literales).

**Pantallas Retina y de alta densidad.** La ventana se pinta en píxeles físicos (el factor de
escala de la pantalla por el zoom), y ese producto es el `devicePixelRatio` de la página.

**Varios worktrees de git.** WPT y test262 pesan varios GB y no se versionan: en un worktree
nuevo se enlazan los del checkout principal en vez de descargarlos otra vez.

```sh
git worktree add ../browser-freedom-otro -b otra-rama
cd ../browser-freedom-otro
git submodule update --init                       # html5lib-tests, css-parsing-tests, test262-parser-tests
ln -s "$(git rev-parse --path-format=absolute --git-common-dir)/../tests/wpt" tests/wpt
ln -s "$(git rev-parse --path-format=absolute --git-common-dir)/../tests/test262" tests/test262
```

`.gitignore` cubre `tests/wpt` y `tests/test262` sean carpetas o enlaces, así que git no los ve.

Los enlaces son de solo lectura en la práctica: el checkout principal es el que se actualiza
con `tools/fetch-wpt.sh` y `tools/fetch-test262.sh`.

## Uso

`<url>` es una URL `http(s)://`, `file://`, `data:` o `about:`, o una ruta local.

```sh
# Ventana navegable (sin argumentos abre la página de inicio); --version imprime la versión
freedom
freedom https://es.wikipedia.org/wiki/Navegador_web
freedom open https://es.wikipedia.org/wiki/Navegador_web

# PNG de 1024×768 con tiempos por fase en stderr
freedom render https://www.google.com -o google.png --width 1024 --height 768
freedom render about:home -o home.png
freedom render "about:engine?https://www.google.com" -o motor.png --height 1100

# Volcados para depurar
freedom dump-dom https://example.com
freedom dump-css estilos.css
freedom dump-style pagina.html --selector "nav a"
freedom dump-layout pagina.html --width 800 --system-fonts

# La lógica de la ventana sin abrir una: un PNG por paso (con el cromo) y el estado en stdout
freedom session about:home "type:google.com" "key:enter" "key:mod+i" "key:f12"     "move:600,300" -o sesion
```

### La interfaz

- **Pestañas**: `Ctrl+T` abre una, `Ctrl+W` la cierra, `Ctrl+Tab`/`Ctrl+1…9` cambian; el clic
  central o `Ctrl+clic` en un enlace lo abre en otra pestaña, y cada pestaña conserva su
  historial, su scroll y su panel del motor.
- **Formularios**: los campos de texto y `<textarea>` se editan (caret, selección con Mayús,
  `Ctrl+A/C/X/V`), `Enter` en un campo envía el formulario (GET o POST `urlencoded`) y los
  botones de envío funcionan.
- **Selección de texto**: arrastrar selecciona, doble clic una palabra y triple clic el
  párrafo; `Ctrl+A` selecciona toda la página y `Ctrl+C` (o «Copiar» en el menú contextual)
  copia, con los saltos de línea suaves como espacios y un `\n` entre bloques. Dentro de un
  campo, arrastrar selecciona su texto. En sesiones sin ventana: `drag:x,y>x,y`,
  `dblclick:x,y` y `tripleclick:x,y`.
- **Menú contextual** (clic derecho): abrir enlaces o imágenes en otra pestaña, copiar sus
  direcciones, guardar una imagen o la página con un diálogo del sistema, cortar/copiar/pegar
  en un campo, ver el código fuente (`about:source?<url>`) e inspeccionar las cajas.
- **Logo**: `assets/freedom.png`. La marca vectorizada (`Icon::Brand`), sin fondo, es
  el icono de la ventana (barra de tareas, `Alt+Tab`) y la de la barra y las pestañas;
  `crates/bf-shell/assets/mark-256.png` es la misma marca sobre transparente para
  `about:home`.

## La interfaz (detalle)

La interfaz muestra lo que el motor hace con cada página, en lugar de imitar el cromo de otros
navegadores. La dibujan el propio motor y el propio rasterizador. La barra, la línea de estado,
el panel y las páginas internas son HTML generado con plantillas de `crates/bf-shell/pages/`.
Los iconos son paths. Todos los colores salen de un único `Theme`
(`crates/bf-shell/src/theme.rs`), y la identidad sale del logo — un marco con la esquina
superior derecha abierta por donde escapa un pájaro:

- **Paleta oscura («tinta»):** marco `#0E1117` (franja de pestañas y línea de estado), cromo
  `#161A22` (barra, pestaña activa, páginas internas), superficie `#1D222C`, texto
  `#ECEEF3`/`#8D94A6` y un único acento, el ámbar del logo `#F5A524`.
- **Paleta clara («papel»):** marco `#E8EAEF`, cromo `#F7F8FA`, superficie `#FFFFFF`, texto
  `#171A21`/`#5E6574`; el mismo ámbar, y como texto (enlaces) un ámbar tostado `#9A5B00` que
  cumple contraste AA.
- **Formas:** cuadrados redondeados como el marco del logo (botones, campo de URL, tarjetas).
- **El marco abierto:** el motivo del logo repetido en la interfaz — el trazo ámbar de la
  pestaña activa no llega a su esquina derecha, y los monogramas de `about:home` y la marca de
  `about:error` son marcos ámbar sin la esquina superior derecha (`.frame` en `style.css`).
- **Elección del tema:** sigue al del sistema; `BF_THEME=light|dark` lo fuerza.
- **Tipografía:** `system-ui`, 13 px en el cromo y 12 px en la línea de estado.

Sus partes:

- **Barra superior** (72 px: franja de pestañas de 34 y barra de herramientas de 38):
  - las pestañas: la activa se une a la barra; cada una lleva la marca (páginas `about:`), un
    globo (las demás) o un indicador ámbar que gira mientras carga;
  - la marca, ← → y ↻ (que pasa a ✕ mientras carga);
  - el campo de URL: sin foco, formateado (candado en `https`, aviso en `http`, la marca en
    `about:`, host destacado, esquema y ruta en gris); con foco, una lupa, borde ámbar y halo;
  - una línea de progreso ámbar a todo el ancho mientras carga;
  - el botón del **panel del motor**, con un punto ámbar si la página usó algo que el motor
    aún no soporta.
- **Panel del motor** (320 px, `Ctrl+I`):
  - tiempos por fase con barras: fetch, parse, estilo, imágenes, layout, pintura y raster;
  - el documento: URL final, redirecciones, HTTP, tipo, encoding y de dónde salió (BOM,
    cabecera, `<meta>`, default), modo quirks, tamaño, nodos DOM, hojas y reglas, imágenes,
    cookies;
  - lo que el motor ignoró, con contadores de `bf-style`, `bf-layout` y `bf-image`:
    `<script>`, propiedades CSS desconocidas con su frecuencia, valores no aceptados,
    at-rules sin efecto, `display` degradados, formatos de imagen sin decodificador y
    controles sin interactividad;
  - las fuentes resueltas;
  - botones para guardar PNG, volcar el layout y volcar el DOM (en la carpeta de
    descargas).
- **Línea de estado** (24 px): el destino del enlace bajo el puntero; zoom, viewport en px CSS
  y tiempo total de carga.
- **Menú contextual**: tarjeta con sombra, resaltado ámbar tenue y el atajo de cada acción a la
  derecha.
- **Superposición de cajas** (`F12`):
  - los fragmentos sobre la página: bloques en ámbar, inline en verde, texto en gris;
  - el fragmento bajo el puntero, resaltado y descrito como `<tag.class> tipo x,y w×h`.
- **Páginas internas**:

  | Página | Contenido |
  |---|---|
  | `about:home` | Inicio: búsqueda, accesos, historial reciente y la tabla de atajos. Escribir en ella va a la barra; lo que no parece una dirección se busca en Google. |
  | `about:engine` | El panel como página completa (`about:engine?<url>` analiza esa URL). |
  | `about:wpt` | El estado de compatibilidad de `docs/wpt-status.md`. |
  | `about:error` | Error de carga (DNS, TLS, timeout, conexión, HTTP 4xx/5xx), con "Reintentar". |
  | `about:` | El índice de las páginas internas. |

  Solo usan CSS que el motor soporta, y un test lo verifica.

| Atajo | Acción |
|---|---|
| `Ctrl+L`, `F6`, clic en el campo | Ir a la barra de dirección |
| `Intro` en la barra | Navegar (sin esquema, `https://`; una ruta, `file://`; si no, buscar) |
| `Ctrl+I` | Panel del motor |
| `F12` | Superposición de cajas |
| `Ctrl+R`, `F5` | Recargar |
| `Esc` | Detener la carga o volver a la página |
| `Alt+←` / `Alt+→`, botones ← →, botones laterales del ratón | Atrás / adelante |
| `Ctrl +`, `Ctrl -`, `Ctrl 0`, `Ctrl`+rueda | Zoom |
| `Ctrl+S` | Guardar un PNG de la ventana (con el cromo) |
| `Ctrl+Shift+D` | Volcar el layout |
| `Ctrl+H` | Inicio |
| rueda, flechas, `Re Pág`/`Av Pág`, `Inicio`/`Fin`, `Espacio`/`Mayús+Espacio` | Desplazar |

La carga (red, parseo, estilo, imágenes y layout) corre en un hilo de trabajo, así que la
ventana nunca se bloquea.

## Pruebas

| Suite | Ubicación | Cómo se obtiene |
|---|---|---|
| html5lib-tests (tokenizador) | `tests/html5lib-tests` | submódulo de git |
| css-parsing-tests | `tests/css-parsing-tests` | submódulo de git |
| WPT: `css`, `html`, `svg`, `dom`, `url`, `fetch`, `xhr`, `IndexedDB`, `workers`… (lista en `tools/fetch-wpt.sh`) | `tests/wpt` | `tools/fetch-wpt.sh` o `tools/fetch-wpt.ps1` |
| test262-parser-tests | `tests/test262-parser-tests` | submódulo de git |
| test262 (`harness`, `test/language`, `test/built-ins`, `test/annexB`, `test/intl402`) | `tests/test262` | `tools/fetch-test262.sh` o `tools/fetch-test262.ps1` |

WPT pesa varios GB. `tools/fetch-wpt` hace un clon parcial y sparse de los directorios que se
usan, en el commit pineado en `tools/wpt-commit.txt`. `tools/fetch-test262` hace lo mismo con
test262 (`tools/test262-commit.txt`). Ni `tests/wpt/` ni `tests/test262/` se versionan.

```sh
git submodule update --init                 # html5lib-tests, css-parsing-tests, test262-parser-tests
sh tools/fetch-wpt.sh                       # o: powershell -File tools/fetch-wpt.ps1
sh tools/fetch-test262.sh                   # o: powershell -File tools/fetch-test262.ps1

cargo test --workspace                      # todas las pruebas (sin los reftests), con muestras
BF_FULL=1 cargo test --workspace            # lo mismo, con las suites de test262 enteras
cargo test -p bf-html --test html5lib_tokenizer -- --nocapture       # resumen por archivo
cargo test -p bf-html --test wpt_tree_construction -- --nocapture
cargo test -p bf-css --test css_parsing_tests -- --nocapture
cargo test -p bf-url --test wpt_url -- --nocapture
cargo test -p bf-js --test parser_tests -- --nocapture         # test262-parser-tests
BF_FULL=1 cargo test -p bf-js --test test262_parse -- --nocapture   # parsing de test262 (escribe docs/test262-status.md)
cargo test -p bf-js --release --test fuzz -- --ignored --nocapture   # 1000 mutaciones sin pánico
cargo run --release -p bf-js --bin test262 -- tests/test262 --out docs/test262-status.md   # ejecución de test262
cargo test -p bf-js --release --test test262_run -- --ignored --nocapture   # regresión de language/
cargo run --release -p bf-web --bin wpt-testharness -- --out docs/wpt-status.md   # WPT testharness.js (dom, html/dom, url, encoding…)
cargo run --release -p bf-web --bin wpt-testharness -- html/syntax/parsing --threads 3   # un directorio, con 3 hilos
cargo test -p bf-web --release --test wpt_testharness -- --ignored --nocapture   # WPT testharness.js sin pánicos
cargo test -p bf-layout --test layout       # 102 casos propios (BF_LAYOUT_PRINT=1 imprime)
cargo test -p bf-paint --test raster        # 14 PNG de referencia (UPDATE_EXPECTED=1 regenera)
cargo test -p bf-paint --test wpt_reftests -- --ignored --nocapture  # reftests (~1,5 min)
cargo run --release -p bf-paint --bin wpt-reftests -- css/css-flexbox   # un subconjunto
REFTEST_GROUPS=fase1 cargo test --release -p bf-shell --test wpt_reftests_js -- --ignored --nocapture   # reftests con JavaScript (~30 s)
cargo test --release -p bf-shell --test wpt_reftests_js -- --ignored --nocapture   # los mismos, en todos los grupos (~2 min)
```

Sin la variable `BF_FULL`, los tests de `bf-js` que recorren corpus de test262
(`test262_parse`, `parser_tests` y `regexp_property_escapes`) prueban una muestra determinista:
1 de cada 50 archivos en orden de ruta, para que `cargo test --workspace` tarde minutos y no
horas. `test262_parse` lee su muestra de `crates/bf-js/tests/data/test262-sample.txt` en vez
de recorrer los ~52 000 archivos del árbol (recorrerlo cuesta minutos con el disco cargado).
Con `BF_FULL=1` recorren todo, y solo entonces `test262_parse` escribe su sección de
[`docs/test262-status.md`](docs/test262-status.md) y regenera ese índice (hay que hacerlo al
cambiar `tools/test262-commit.txt`; con un índice de otro commit, la muestra se saca
recorriendo el árbol). Los tests `#[ignore]` (reftests, fuzz,
regresión de test262) siguen igual: se corren con `--ignored`.

**El runner de testharness.js** (`crates/bf-web/src/bin/wpt-testharness.rs`):
- Acepta directorios de `tests/wpt` como argumentos, `--filter <texto>`, `--threads <n>`,
  `--timeout <s>` (10 por defecto), `--list-failures`, `--results <tsv>` y `--out <md>`.
- Corre cada variante `<meta name=variant>` de un `.html` (por ejemplo, los `?file=…` de
  `html5lib_*`).
- No corre los tests `-manual`, porque necesitan a una persona.
- Cada documento corre en su hilo. Si no devuelve el control en el tiempo límite más 10 s,
  cuenta como `NORESULT` y la corrida sigue.

**Pruebas propias de lo añadido en el Mac:**

| Qué | Prueba |
|---|---|
| Carga de imágenes: `load`, `error` y `decode()` de `<img>`; `loading=lazy`; el `load` de `<iframe>` que espera a su documento | `cargo test -p bf-web --test img_load` |
| Una sola decodificación por imagen | `cargo test -p bf-image --test shared_cache` y `cargo test -p bf-shell --test image_decode_once` |
| CSS Font Loading (`document.fonts`, `FontFace`) | `cargo test -p bf-web --test font_loading` |
| Parser HTML: adoption agency sin ciclos a través de shadow roots y templates, scripts SVG, CR LF partido entre dos `document.write` | `cargo test -p bf-html --test robustness` |
| Instalar una ventana con el GC recolectando en cada asignación | `cargo test -p bf-web --test gc_rooting` |
| Scripts en documentos XHTML y SVG mientras se parsean | `cargo test -p bf-shell --test xhtml_scripts` |
| La rueda sobre `<textarea>` y list box | `cargo test -p bf-shell --test wheel` |
| Envío de formularios POST a un servidor local | `cargo test -p bf-shell --test forms` |
| Google, Wikipedia y Hacker News sin pánicos (usa la red) | `cargo test -p bf-shell --test real_sites -- --ignored` |

En el Mac las pruebas se corren con `export CARGO_BUILD_JOBS=3` y las mediciones de WPT con
3 hilos, porque la máquina se comparte entre varios ejecutores.

Los reftests no hacen fallar `cargo test`: escriben el resumen en
[`docs/wpt-status.md`](docs/wpt-status.md) y dejan en `target/reftests/` las imágenes de cada
fallo (`-actual`, `-expected`, `-diff`) y el detalle por test (`results.tsv`).

**Reftests con JavaScript.** El harness de `bf-paint` salta los reftests con `<script>` o
atributos `on*`, en el test o en una referencia, porque `bf-paint` no depende del motor de
JavaScript. Los corre `freedom wpt-reftests-js` (`crates/bf-shell/src/reftest_js.rs`):
- carga cada documento con sus scripts, sirviendo `tests/wpt` como `wptserve`;
- corre el bucle de eventos con un reloj virtual;
- dispara `TestRendered` y espera a que la raíz pierda `class="reftest-wait"`, con 10 s
  reales y 60 s virtuales como límite;
- pinta como la ventana y compara con las reglas de `bf_paint::reftest` (`match`,
  `mismatch`, `fuzzy`).

Un pánico o un tiempo agotado cuentan como `error`.

Opciones:
- `REFTEST_GROUPS`: directorios de `tests/wpt` separados por comas, o `fase1` para los 13 de
  la fase 1. Sin la variable, todos los de `GROUPS`.
- `REFTEST_FILTER`: solo las rutas que contienen ese texto.
- `REFTEST_THREADS`: hilos (3 por defecto).

La tabla separa los `needs-js` sin referencia (testharness.js o manuales, que este runner no
corre) de los reftests con JavaScript. De los 2114 `needs-js` de la fase 1, 852 son manuales.
Entre los dos runners se ejecuta el 93.1 % de los 1262 restantes (`docs/roadmap.md`, 2.3). El detalle queda en `target/reftests-js/results.tsv`,
con las imágenes de los fallos al lado. También se puede llamar directamente:
`freedom wpt-reftests-js --wpt tests/wpt --out target/reftests-js`.

Tablas generadas (con `python -I`):

| Script | Fuente | Salida |
|---|---|---|
| `tools/gen_entities.py` | `crates/bf-html/data/entities.json` | entidades nombradas de HTML |
| `tools/gen_idna.py` | `crates/bf-url/data/` (IDNA 18.0, bidi, joining) | `crates/bf-url/src/tables.rs` |
| `tools/gen_encodings.py` | `crates/bf-infra/data/` (índices de WHATWG Encoding) | `crates/bf-infra/src/encoding_tables.rs` |
| `tools/gen_unicode_id.py` | `crates/bf-js/data/` (UCD 18.0: ID_Start/ID_Continue, Zs, alias de propiedades) | `crates/bf-js/src/lexer/unicode_tables.rs` |

Verificación completa antes de integrar cambios:

```sh
cargo build --workspace --release
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo test -p bf-paint --test wpt_reftests -- --ignored --nocapture
```
