# browser-freedom

Un navegador web con un motor 100% propio, escrito en Rust.

browser-freedom no usa Chromium, WebKit, Gecko ni Servo: HTML, DOM, CSS, layout, pintura,
URLs, red y (en la fase 2) JavaScript están escritos desde cero. Cada subsistema sigue su
especificación (WHATWG, W3C, IETF, Unicode, ECMA) al pie de la letra, y la compatibilidad se
mide con las suites de pruebas oficiales.

La **fase 1** está cerrada: el navegador carga y muestra páginas reales sin JavaScript (Google,
Wikipedia, Hacker News, example.com) en una ventana navegable con barra de URL, enlaces,
historial, scroll y zoom. El plan de las fases siguientes está en
[`docs/roadmap.md`](docs/roadmap.md).

## Estado por subsistema

| Subsistema | Spec | Suite oficial | Resultado |
|---|---|---|---|
| Tokenizador HTML | HTML §13.2.5 | html5lib-tests (tokenizer) | **7041** pasan, 0 fallan (3 *stale*, 4 irrepresentables) |
| Tree construction | HTML §13.2.6, §13.4 | WPT `html/syntax/parsing` | **3898** pasan, 0 fallan (6 omitidos) |
| CSS Syntax | CSS Syntax 3 | css-parsing-tests | **253** pasan, 0 fallan (24 *stale*) |
| URL e IDNA | URL Standard, UTS #46 | WPT `url/resources` | **1444** pasan, 0 fallan (7 omitidos: encoders multibyte) |
| Layout y pintura | CSS 2.2, Backgrounds 3, Flexbox 1, Selectors 4 | WPT reftests (13 directorios) | **3811 / 5067 ejecutables (75.2%)** |

- *stale*: casos cuya expectativa quedó desactualizada respecto del spec vigente. Se listan en
  cada test con el motivo.
- *irrepresentables*: surrogates sueltos, que un `String` de Rust no admite. Llegan con el
  `DomString` WTF-8 de la fase 2.
- *omitidos*: casos de scripting o de encodings multibyte.

Detalle de los reftests por directorio (fuente: [`docs/wpt-status.md`](docs/wpt-status.md)):

| Directorio | pass | fail | needs-js | % de los ejecutables |
|---|---:|---:|---:|---:|
| css/CSS2/box-display | 65 | 21 | 53 | 75.6% |
| css/CSS2/margin-padding-clear | 575 | 107 | 57 | 84.3% |
| css/CSS2/normal-flow | 621 | 72 | 162 | 89.5% |
| css/CSS2/floats | 70 | 30 | 25 | 70.0% |
| css/CSS2/floats-clear | 152 | 59 | 36 | 72.0% |
| css/CSS2/positioning | 459 | 54 | 65 | 89.5% |
| css/CSS2/linebox | 167 | 23 | 53 | 87.4% |
| css/CSS2/text | 335 | 46 | 182 | 87.9% |
| css/CSS2/colors | 11 | 8 | 3 | 57.9% |
| css/CSS2/borders | 425 | 79 | 258 | 84.3% |
| css/css-backgrounds | 259 | 357 | 334 | 42.0% |
| css/css-flexbox | 565 | 384 | 478 | 59.5% |
| css/selectors | 107 | 13 | 404 | 89.2% |

`needs-js` son los tests de testharness.js o con `<script>`: se ejecutarán cuando exista el
motor JavaScript.

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
- **`bf-shell`**: el binario. Carga en un hilo de trabajo y muestra el resultado en una
  ventana con barra de URL propia (ver [Uso](#uso)).
- **`bf-js`**: reservado para el motor ECMAScript de la fase 2.

## Arquitectura

```
 bf-url ─► bf-net ─► bf-html ─► bf-dom ─► bf-css / bf-style ─► bf-layout ─► bf-paint ─► bf-shell
  (URL)   (Fetch,    (parser)   (árbol)   (sintaxis, cascada,  (cajas,      (display     (ventana,
          HTTP/TLS,                        valores computados)  fragmentos)  list, raster) navegación)
          encoding)                                                 ▲
                                               bf-font ─────────────┤ (métricas, shaping)
                                               bf-image ────────────┘ (tamaños naturales; pixmaps a bf-paint)
 bf-infra: tipos comunes, ResourceLoader, encodings (lo usan todos)       bf-js: fase 2
```

El flujo de una página:
1. `bf-net` resuelve la URL con `bf-url`, hace el fetch y elige el encoding.
2. `bf-html` parsea a un árbol `bf-dom`.
3. `bf-style` carga las hojas (UA, autor, `@import`) con `bf-css` y calcula los valores
   computados de cada elemento.
4. `bf-layout` construye el árbol de cajas y produce un árbol de fragmentos con geometría
   absoluta. Para eso usa `bf-font` (métricas y glifos) y los tamaños de `bf-image`.
5. `bf-paint` lo convierte en una lista de visualización y la rasteriza.
6. `bf-shell` compone la página con la barra y la muestra.

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
  - fuentes: `ttf-parser`, `rustybuzz`;
  - utilidades: `serde`/`serde_json` (solo tests y configuración), `tokio`, `thiserror`, `log`.

  Hoy se usan todos menos `wgpu`, `tokio`, `serde`, `thiserror` y `log`.
- **La compatibilidad se mide con suites oficiales**: html5lib-tests, web-platform-tests y
  (fase 2) test262. Ninguna funcionalidad se da por terminada sin su suite.
- **Spec literal**: el código sigue el texto del spec y cita la sección. Si un test contradice
  el spec vigente, se documenta en [`docs/wpt-status.md`](docs/wpt-status.md); no se agrega a
  una lista de omitidos sin explicación.
- `#![forbid(unsafe_code)]` en todos los crates.

Ver [`CONTRIBUTING.md`](CONTRIBUTING.md) para el flujo de trabajo.

## Compilar

Requiere Rust estable (edición 2024) y Python 3 solo para regenerar tablas.

```sh
cargo build --workspace --release
```

El binario queda en `target/release/bf-shell` (`bf-shell.exe` en Windows).

## Uso

`<url>` es una URL `http(s)://`, `file://` o `data:`, o una ruta local.

```sh
# Ventana navegable
bf-shell open https://es.wikipedia.org/wiki/Navegador_web

# PNG de 1024×768 con tiempos por fase en stderr
bf-shell render https://www.google.com -o google.png --width 1024 --height 768
bf-shell render pagina.html -o pagina.png --scale 2

# Volcados para depurar
bf-shell dump-dom https://example.com
bf-shell dump-css estilos.css
bf-shell dump-style pagina.html --selector "nav a"
bf-shell dump-layout pagina.html --width 800 --system-fonts

# La lógica de la ventana sin abrir una: un PNG por paso y el estado en stdout
bf-shell session https://es.wikipedia.org/wiki/Navegador_web "key:pagedown" \
    "link:World Wide Web" "key:alt+left" "key:ctrl+l" "type:example.com" "key:enter" -o sesion
```

En la ventana (`open`):

| Acción | Teclas / ratón |
|---|---|
| Enfocar la barra y seleccionar todo | `Ctrl+L`, `F6` o clic en el campo |
| Navegar a lo escrito | `Intro`; sin esquema se antepone `https://`; una ruta local se abre como `file://` |
| Editar el campo | escribir, `Retroceso`, `Supr`, flechas, `Inicio`/`Fin` (con `Mayús`, seleccionando), `Ctrl+A` |
| Volver a la página | `Esc` |
| Seguir un enlace | clic (el cursor cambia a una mano y la barra muestra el destino) |
| Atrás / adelante | `Alt+←` / `Alt+→`, botones ◀ ▶, botones laterales del ratón |
| Recargar | `Ctrl+R`, `F5`, botón ⟳ |
| Desplazar | rueda, flechas, `Re Pág`/`Av Pág`, `Inicio`/`Fin`, espacio (`Mayús+espacio` sube) |
| Zoom | `Ctrl +`, `Ctrl -`, `Ctrl 0`, `Ctrl`+rueda |

La carga (red, parseo, estilo, imágenes, layout) corre en un hilo de trabajo, así que la
ventana no se bloquea. Los errores de red se muestran como una página de error interna.

## Pruebas

| Suite | Ubicación | Cómo se obtiene |
|---|---|---|
| html5lib-tests (tokenizador) | `tests/html5lib-tests` | submódulo de git |
| css-parsing-tests | `tests/css-parsing-tests` | submódulo de git |
| WPT: `html/syntax/parsing`, `url/resources` y los reftests de CSS | `tests/wpt` | `tools/fetch-wpt.sh` o `tools/fetch-wpt.ps1` |

WPT pesa varios GB. `tools/fetch-wpt` hace un clon parcial y sparse de los directorios que se
usan, en el commit pineado en `tools/wpt-commit.txt`. `tests/wpt/` no se versiona.

```sh
git submodule update --init                 # tests/html5lib-tests y tests/css-parsing-tests
sh tools/fetch-wpt.sh                       # o: powershell -File tools/fetch-wpt.ps1

cargo test --workspace                      # todas las pruebas (sin los reftests)
cargo test -p bf-html --test html5lib_tokenizer -- --nocapture       # resumen por archivo
cargo test -p bf-html --test wpt_tree_construction -- --nocapture
cargo test -p bf-css --test css_parsing_tests -- --nocapture
cargo test -p bf-url --test wpt_url -- --nocapture
cargo test -p bf-layout --test layout       # 102 casos propios (BF_LAYOUT_PRINT=1 imprime)
cargo test -p bf-paint --test raster        # 14 PNG de referencia (UPDATE_EXPECTED=1 regenera)
cargo test -p bf-paint --test wpt_reftests -- --ignored --nocapture  # reftests (~1,5 min)
cargo run --release -p bf-paint --bin wpt-reftests -- css/css-flexbox   # un subconjunto
```

Los reftests no hacen fallar `cargo test`: escriben el resumen en
[`docs/wpt-status.md`](docs/wpt-status.md) y dejan en `target/reftests/` las imágenes de cada
fallo (`-actual`, `-expected`, `-diff`) y el detalle por test (`results.tsv`).

Tablas generadas (con `python -I`):

| Script | Fuente | Salida |
|---|---|---|
| `tools/gen_entities.py` | `crates/bf-html/data/entities.json` | entidades nombradas de HTML |
| `tools/gen_idna.py` | `crates/bf-url/data/` (IDNA 18.0, bidi, joining) | `crates/bf-url/src/tables.rs` |
| `tools/gen_encodings.py` | `crates/bf-infra/data/` (índices de WHATWG Encoding) | `crates/bf-infra/src/encoding_tables.rs` |

Verificación completa antes de integrar cambios:

```sh
cargo build --workspace --release
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo test -p bf-paint --test wpt_reftests -- --ignored --nocapture
```
