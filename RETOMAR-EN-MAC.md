# Retomar Freedom en el Mac

Estado actual del proyecto, para quien lo continúe sin el historial de las sesiones anteriores.
El desarrollo se hace en un Mac (macOS, Apple M4) desde el 2026-11-12. Si eres una sesión de
Claude Code, lee este archivo entero; la sección 9 dice por dónde seguir y la 10 trae el
prompt para empezar.

El hash de `master` cambia en cada integración: mira `git log --oneline -1 master`.

## 1. Qué es y dónde está

- **Freedom** es un navegador con motor 100 % propio en Rust: HTML, CSS, layout, JavaScript,
  red y DOM. Son 15 crates (`crates/bf-*`); el ejecutable es `freedom`, del paquete
  `bf-shell`.
- **Repositorio:** `https://github.com/ramirezjhulian7/browser-freedom`. La rama base es
  `master` y lleva todo lo integrado.
- **Plan y estado:**
  - [`docs/roadmap.md`](docs/roadmap.md): criterios de cada fase, tabla de avance y un bloque
    **Estado** por apartado.
  - [`docs/wpt-status.md`](docs/wpt-status.md) y
    [`docs/test262-status.md`](docs/test262-status.md): las suites oficiales.
  - [`docs/rendimiento.md`](docs/rendimiento.md): cómo medir la primera pintura y el
    desplazamiento.
  - [`docs/faltantes-web.md`](docs/faltantes-web.md): lo que falla en sitios reales.
  - [`docs/pendiente/mac/`](docs/pendiente/mac/): una nota por ejecutor, con lo hecho, lo
    medido y lo que falta en cada ronda.
  - [`docs/pendiente/`](docs/pendiente/): las notas de la etapa anterior, en Windows. Son
    historia.

## 2. Entorno del Mac

- **Rust 1.100.0**, instalado con rustup. Antes de compilar en una terminal nueva:
  `. "$HOME/.cargo/env"`.
- **Submódulos** (html5lib-tests, css-parsing-tests, test262-parser-tests):
  `git submodule update --init`.
- **WPT y test262** pesan varios GB y no se versionan. Solo existen en el checkout principal
  (`tests/wpt` y `tests/test262`), descargados con `sh tools/fetch-wpt.sh` y
  `sh tools/fetch-test262.sh` en los commits de `tools/wpt-commit.txt` y
  `tools/test262-commit.txt`.
- **Worktrees:** cada ejecutor trabaja en su worktree (`../browser-freedom-army-e<n>`, rama
  `army/e<n>`). `tests/wpt` y `tests/test262` son allí enlaces simbólicos al checkout
  principal. Cómo crearlos: [`README.md`](README.md), sección "Plataformas", "Varios worktrees
  de git".
- **Rutas del perfil en macOS:** el perfil está en `~/Library/Application Support/Freedom`, la
  caché en `~/Library/Caches/Freedom` y las descargas en `~/Downloads`. La tabla completa está
  en `README.md`, sección "Plataformas".

## 3. Compilar, probar y abrir el navegador

```sh
export CARGO_BUILD_JOBS=3
cargo build --release -p bf-shell           # deja target/release/freedom
cargo run --release -p bf-shell             # abre la ventana con la página de inicio
./target/release/freedom https://es.wikipedia.org
```

Sin ventana, para capturas y pruebas:

```sh
./target/release/freedom render https://es.wikipedia.org -o wikipedia.png
./target/release/freedom session about:home "type:google.com" "key:enter" -o sesion
```

Los comandos de cada suite (html5lib, WPT testharness, reftests con y sin JavaScript,
test262) están en `README.md`, sección "Pruebas".

## 4. Verificación completa

Es la que corre el planificador en `master` después de integrar:

```sh
export CARGO_BUILD_JOBS=3
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

`BF_FULL=1 cargo test --workspace` recorre test262 entero (lento). Los reftests de WPT no
hacen fallar `cargo test`: se corren aparte con `--ignored` (ver "Pruebas" en el `README.md`).

## 5. Reglas del proyecto (no cambian)

- **Motor 100 % propio.** No se cambia a V8, QuickJS ni Boa, y rige `#![forbid(unsafe_code)]`.
- **Sin crates externos nuevos.** Los de plataforma permitidos están en el `README.md`.
- **Spec literal**, con la sección citada en el código. La suite oficial (WPT, test262)
  manda.
- **Identidad propia.** `crates/bf-infra/src/identity.rs` es la única fuente y no se toca.
  Nada imita a Chrome. Si una página se rompe por detectar el agente de usuario, se anota en
  `docs/roadmap.md`, sección "Compatibilidad por UA".
- **Preferencias del usuario:**
  - Cuando dice "ábrelo", solo hay que abrir el navegador; las pruebas las hace él.
  - Antes de que se agote el límite de uso, cada ejecutor deja commit y su nota al día.

## 6. Cómo se trabaja: planificador, ejecutores y territorios

El trabajo se reparte con el mod "army" de Claude Code. Una sesión planificadora asigna
bloques a seis ejecutores; cada uno trabaja en su worktree y su rama `army/e<n>`.

- **El planificador** no edita archivos del repo. Integra cada rama en `master` con
  `git merge --no-ff` y corre la verificación completa (sección 4).
- **Cada ejecutor:**
  - empieza cada bloque con `git merge master` y resuelve sus propios conflictos;
  - corre solo los tests y el clippy de los crates que toca;
  - deja su nota en `docs/pendiente/mac/e<n>.md` y hace commit antes de reportar.
- **Reglas del ciclo:**
  - **Nada de `git stash`**: la pila de stash es común a todos los worktrees, y otro
    ejecutor puede sacar lo que guardaste. Para comparar antes y después, usa un parche de
    `git diff`.
  - `export CARGO_BUILD_JOBS=3`, y las mediciones con 3 hilos (`--threads 3`,
    `--test-threads=3`). La máquina se comparte.
  - Nada de `git push` ni de abrir ventanas. Nunca se borra un `index.lock`: se espera y se
    reintenta.
  - Si se cambia un tipo público, `cargo check --workspace --all-targets`. Si se toca
    `bf-style`, también los tests de `bf-layout`, `bf-paint`, `bf-web` y `bf-shell`.
  - **Medidas de tiempo.** Con seis ejecutores compilando y midiendo a la vez, la carga
    llegó a 200 y las cifras de tiempo dejaron de ser comparables entre corridas: el mismo
    binario dio una primera pintura de 1,5 s (mediana) con carga 13 y de 4,3 s con carga 64-72
    (`docs/pendiente/mac/e3.md`, "Ciclo de cierre, ronda 5"). Un antes y después se mide alternando los dos
    binarios (5 corridas de cada uno), con el mínimo y la mediana, la carga de `uptime` en
    cada corrida y el tiempo de CPU de `/usr/bin/time -l`, que depende menos de la carga.

Territorios del ciclo del Mac (cada ronda el planificador fija además qué archivos puede tocar
cada uno):

| Ejecutor | Territorio | Nota |
|---|---|---|
| E1 | DOM, Shadow DOM, custom elements, formularios, workers y accesibilidad (`bf-dom`, `bf-web/src/{dom,html,workers}`) | [`e1.md`](docs/pendiente/mac/e1.md) |
| E2 | Imágenes y su carga, SVG, parser XML, fondos, scroll y overflow (`bf-image`, `bf-svg`, imágenes de `bf-paint` y `bf-web`) | [`e2.md`](docs/pendiente/mac/e2.md) |
| E3 | JavaScript (`bf-js`), parser HTML (`bf-html`) y documentación de estado (`docs/roadmap.md`) | [`e3.md`](docs/pendiente/mac/e3.md) |
| E4 | Pintura y raster, animaciones, BOM e hilo de la pestaña (`bf-paint`, `bf-web/src/bom`, `bf-shell/src/window`) | [`e4.md`](docs/pendiente/mac/e4.md) |
| E5 | Layout y texto (`bf-layout`, `bf-font`), CSS Font Loading | [`e5.md`](docs/pendiente/mac/e5.md) y [`e5-flexbox-subconjuntos.md`](docs/pendiente/mac/e5-flexbox-subconjuntos.md) |
| E6 | Shell, red y plataforma macOS (`bf-net`, `bf-shell`, `packaging/`) | [`e6.md`](docs/pendiente/mac/e6.md) |

## 7. Qué hay en `master`

El estado apartado por apartado, con sus cifras, está en `docs/roadmap.md`. A grandes rasgos:

- **Motor de documentos:** HTML, CSS, flex (con los tamaños intrínsecos de §9.9.1), grid
  con subgrid y lanes, tablas, tamaños intrínsecos, escritura vertical, bidi, cortes de
  línea, scroll con `sticky`, SVG, WebP y `object-fit`. `:popover-open`: los popovers
  cerrados no se pintan. El parser HTML sigue a los nodos que un script adopta en otro
  documento.
- **JavaScript propio:** test262 al 98.8 % (50 597 pases), con Intl y Temporal. El código
  compilado se guarda en `bf_js::code_cache`: el JavaScript propio de la ventana se compila
  una vez por proceso (E3).
- **Plataforma web:**
  - DOM con Shadow DOM y custom elements;
  - fetch, XHR, streams, IndexedDB, workers, Service Workers y Cache API;
  - transiciones, animaciones y `transform`;
  - del ciclo del Mac: carga de imágenes con sus eventos, CSS Font Loading
    (`document.fonts`), el `load` de `<iframe>` que espera a su documento y `pageshow` tras
    el `load`;
  - `loading=lazy` completo: el motor decide qué imagen espera, la pide al desplazar la
    ventana y el pintado no descarga ni las lejanas ni dos veces las cercanas (E2). Las
    perezosas cercanas se revisan con la maqueta del navegador (HTML §8.1.7.3) y aparecen
    un cuadro después de la primera pintura (E3);
  - `IntersectionObserver` y `ResizeObserver` (E2; WPT `intersection-observer` 222 / 315 y
    `resize-observer` 94 / 115);
  - `FontFace.load()` descarga por red también en la ventana (E6);
  - `<link rel=stylesheet>` dispara `load` y `document.styleSheets` tiene reglas en la
    ventana (E2).
- **Estilo y maqueta:** estilo compartido entre elementos y cascada en varios hilos, y
  `bf_style::sheet_cache`, una caché de hojas ya parseadas (E4); caché de líneas idéntica bit
  a bit y shaping con caras y planes guardados (E5, `3087f82`; sus reftests de flotantes,
  líneas y tablas dan el mismo estado en los 7519, comparados por E3). Las pistas de presentación
  quedan debajo de las capas de autor (`@layer`) y las imágenes con `width` y `height`
  reservan su hueco con esa proporción (E2).
- **Accesibilidad:** el motor calcula el rol, el nombre y los estados de cada elemento y arma
  el árbol de accesibilidad del documento (`html-aam` 93.4 %, `wai-aria` 95.7 %). El árbol
  todavía no se expone a macOS.
- **Red y seguridad:** HTTP/2, caché HTTP en disco, HSTS, Referrer-Policy, Content Security
  Policy (con `img-src` y el evento `securitypolicyviolation`) y bloqueo de contenido mixto.
  Con una VPN que da una ruta IPv6 sin salida, la conexión prueba IPv4 primero (RFC 6724).
- **Pintura:** raster en tiles de la página, en paralelo y con desplazamiento incremental; el
  cuadro de la ventana se reutiliza y los `sticky` van en capa propia. Desplazar Wikipedia va
  a **60 fps**: mediana de 3.7-4.0 ms por cuadro a densidad 1 y de 5.2-6.1 ms a densidad 2
  (medida del planificador sobre `276fa07`, con los ejecutores casi parados; criterio
  cumplido). `monospace` y `<pre>` salen a 13 px.
- **Primera pintura de Wikipedia: 328-363 ms**, criterio cumplido (medida del planificador
  sobre `489e6f8`, release, caché caliente, siete corridas; meta < 500 ms). La red se lleva
  250-277 ms; parseo, estilos, maqueta y pintado, unos 80-90. Mientras se valida el documento,
  la precarga ya parsea las hojas guardadas, compila los scripts y el JavaScript de la ventana
  (E3). El desglose está en `docs/roadmap.md`, fase 5, "Rendimiento".
- **Shell:** pestañas, historial, marcadores, descargas, búsqueda en página, zoom, atajos con
  `Cmd` y pantallas Retina. Una pestaña que falla muestra una página de error.
- **Empaquetado:** `Freedom.app` firmada ad hoc con su `.dmg`
  (`sh packaging/macos/build.sh --native`; detalle en `packaging/README.md`).
- **Runners de WPT:** `wpt-testharness` (testharness.js) y `freedom wpt-reftests-js`
  (reftests con JavaScript). `html/syntax/parsing` está en 8164 / 8166 (los 2 que faltan no se
  pueden cumplir), `custom-elements` en 3556 / 4172 y `event-loops` en 75 %.

Las cifras de esta sección salen de `docs/roadmap.md` y `docs/test262-status.md`, con su
fuente en cada apartado, y de [`docs/rendimiento.md`](docs/rendimiento.md). **Avance por
criterios: 88.9 %** de las fases 2 a 5 (91.1 % contando la fase 1). Lo que falta está en la
lista "Criterios no cumplidos" del roadmap. Dos de los cuatro criterios que quedan no se
cumplirán por decisión del usuario, y los otros dos siguen abiertos (sección 9a).

## 8. Ramas en integración

`git log --oneline master..army/e<n>` muestra qué trae cada rama que `master` no tiene, y su
nota (`git show army/e<n>:docs/pendiente/mac/e<n>.md`) dice qué está hecho y qué falta.

## 9. Lo primero al retomar

Estado del 2026-11-17 a las 19:00, cuando el usuario paró la tanda nueva (por mensaje
directo; el mod ya no reparte bloques). `master` está en `1a4c877`:
- las rondas 8 de E1, E2 y E3;
- WOFF y WOFF2 y el primer paso del estilo incremental (E2);
- AES, PBKDF2 y HKDF en `crypto.subtle` (E1).

**Avance: 88.9 %** (91.1 % con la fase 1): 47 de 51 criterios.

Lo primero, en este orden:
1. **Integrar `army/e3`** (9b): la maqueta de los scripts con las fuentes web, las
   sugerencias de recursos y las conexiones de la primera visita. Ya tiene `master`
   `1a4c877` fusionado, con su conflicto resuelto en `bf-web/src/cssom/mod.rs`, y verificado.
2. **Las aplicaciones de JavaScript pesado** (E1). Son la mayor parte de los defectos graves
   de los barridos: YouTube, Vimeo, Reddit y los esqueletos de GitHub y GitLab. Antes, el
   pánico "Gc liberado" de GitHub en `run_animation_frames` (quinto barrido).
3. **Imágenes que no salen** (E2, sin diagnosticar): Apple (la portada mide 0 de ancho),
   GitLab y los iconos de los controles de OpenStreetMap.
4. **Estilo incremental** (E2): el primer paso, solo para cambios de atributo, está en
   `master`. Falta medir cuánto baja el tiempo de scripts y extenderlo a cambios del árbol.

### 9a. Las decisiones del usuario

Decididas el 2026-11-17 (roadmap, "Lo que decidió el usuario y lo que sigue abierto"):
- **No se excluyen los 12 tests que ningún navegador cumple** (2 de `html/syntax/parsing` y 10
  reftests de flexbox de 3.6). Los criterios 2.3 y 3.6 siguen sin cumplirse.
- **No se añade `accesskit`.** El lector de pantalla sigue sin cumplirse: el árbol de
  accesibilidad del motor no llega a `NSAccessibility`.

Siguen abiertas:
- **Fuzzer de IPC:** pide separar el motor en procesos (navegador y contenido).
- **El `User-Agent` de Freedom y Google:** `accounts.google.com/gsi/client` responde 403 al
  de Freedom. La regla es no imitar a Chrome, y `identity.rs` no se toca; el planificador lo
  deja así salvo que el usuario diga otra cosa.

### 9b. Las ramas y lo que deja cada ejecutor

- **`army/e1`:** su ronda 8 está en `master` (`fcdb9c7`):
  - `crypto.subtle` con AES-CBC, AES-CTR y AES-GCM, PBKDF2 y HKDF (Amazon);
  - la consola dice qué script se cortó por tiempo agotado.

  Tenía en curso, sin commit, cambios en `custom_elements`; ver su nota.
- **`army/e2`:** en `master` están su ronda 8 (`element.style`, el mapa de OpenStreetMap y
  una sola descarga por imagen) y la tanda nueva (`1a4c877`): WOFF y WOFF2, con los
  descompresores de `bf-net` registrados desde el shell, y la cascada incremental del CSSOM
  tras cambios de atributo.
- **`army/e3`** (sin integrar, con `master` `1a4c877` fusionado; `pendiente/mac/e3.md`,
  "Tanda nueva"):
  - **hecho:**
    - `ShellLayout` mide con las `@font-face` del documento: sin el `ResizeObserver loop`
      de más;
    - `preconnect`, `dns-prefetch`, `preload` de fuentes e imágenes y la cabecera `Link`;
    - los `fetch()` y XHR de los scripts van por la conexión h2 del origen, en vez de abrir
      cada uno una conexión HTTP/1.1. Conexiones de la primera visita: MDN 4 → 2, GitLab
      19 → 4 y El País 28 → 14;
    - test de la reanudación de TLS, que rustls ya hacía;
    - el quinto barrido;
  - **deja pendiente:** que el `<link rel=preload>` de otro origen que procesa el motor
    (`bf-web/src/cssom/sheets.rs`) pida con el cargador de la página, para no descargarlo
    dos veces.

### 9c. `app.asimetrix.co` con sesión iniciada

**Pendiente: solo el usuario puede probarlo**, porque sin entrar no se reproducen ni la pestaña
que se caía (`el realm no tiene WebContext`) ni el `ChunkLoadError`. Lo que cambió desde la
captura:
- E1 quitó los pánicos de `WebContextHandle` y añadió el diagnóstico de caídas (`Esta página
  falló` dice qué pasó; `pendiente/mac/e1.md`, "Sesión nueva, ronda 3");
- E2 hizo que las hojas enlazadas carguen en la ventana, la causa probable del
  `ChunkLoadError` (`pendiente/mac/e2.md`, "Sesión nueva, ronda 1").

En `https://asimetrix.co/` a 1180 px ya no salen el texto del `<noscript>`, las franjas
laterales ni el desborde horizontal; el logo de "Ecosistema" mide 446×208 y el hueco del vídeo
1068×600 (negro: el motor no reproduce vídeo). El titular grande de la portada ya sale: era una
animación de entrada que terminaba entre dos pintados (E2) y un `<iframe>` de YouTube que
retenía la pestaña 2,7 s (E3), los dos arreglados desde `3a7c372`.

### 9d. Uso diario

- **El barrido de sitios** (`docs/pendiente/mac/barrido-sitios.md`) dice qué le falta al
  navegador en 27 sitios públicos conocidos, con las carencias ordenadas por número de sitios
  y un dueño propuesto.
  - Hay cinco barridos, con una columna de cambio entre uno y otro.
  - En el quinto (`master` `fcdb9c7`, 25 esperas), 7 sitios están bien, 13 tienen defectos
    menores y 7 graves.
  - GitHub tumba a veces la pestaña: "Gc liberado" en `bf-js/src/heap.rs`, durante
    `run_animation_frames` (E1). Pasa también con `cb41001`.
- `freedom session <url> wait dump -o <prefijo>` deja el DOM, el árbol de maqueta y la consola
  de cada sitio para depurarlo (`docs/rendimiento.md`, "Volcado de una sesión").
- `FREEDOM_NET_LOG=1` registra:
  - cada petición, también las de los scripts y las que fallan;
  - en `army/e3`, cada conexión nueva (con su TLS y si se reanudó) y cada espera de hueco.

  `FREEDOM_NET_LOG=cabeceras` añade las cabeceras de cada salto.
- **`Freedom.app`:** `sh packaging/macos/build.sh` la deja en `dist/` (0.1.0, universal,
  58 MB; `.dmg` de 26 MB). No está instalada en `/Applications`.

### 9e. Método

- Rondas de dos o tres ejecutores, con `CARGO_BUILD_JOBS=2` y una compilación a la vez por
  ejecutor, para que las medidas de tiempo valgan (sección 6, "Medidas de tiempo"). Con
  `uptime` por encima de 30, las medidas se repiten.
- Si el usuario escribe mientras se entrega un reparto, comprobar que los bloques llegaron:
  el 2026-11-13 se perdió uno.

## 10. Prompt para pegar en Claude Code

```
Continúa el proyecto Freedom (navegador con motor propio en Rust) en este Mac.
Lee primero RETOMAR-EN-MAC.md en la raíz del repo, luego docs/roadmap.md (tabla
"Avance por criterios" y "Criterios no cumplidos") y las notas de docs/pendiente/mac/.
Comprueba con git log qué ramas army/e<n> tienen trabajo que master no tiene, y sigue el
orden de la sección 9 ("Lo primero al retomar"). No cambies las reglas del proyecto (sección 5) ni las del ciclo (sección 6). Dime el
estado y propón el siguiente reparto entre los seis ejecutores.
```
