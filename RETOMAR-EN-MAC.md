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
- **JavaScript propio:** test262 al 98.8 %, con Intl y Temporal.
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
    un cuadro después de la primera pintura (E3).
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
- **Primera pintura de Wikipedia:** 542-665 ms en cinco corridas (misma medida); la meta es
  menos de 500 ms. La red sola se lleva 247-315 ms; el desglose está en `docs/roadmap.md`,
  fase 5, "Rendimiento".
- **Shell:** pestañas, historial, marcadores, descargas, búsqueda en página, zoom, atajos con
  `Cmd` y pantallas Retina. Una pestaña que falla muestra una página de error.
- **Empaquetado:** `Freedom.app` firmada ad hoc con su `.dmg`
  (`sh packaging/macos/build.sh --native`; detalle en `packaging/README.md`).
- **Runners de WPT:** `wpt-testharness` (testharness.js) y `freedom wpt-reftests-js`
  (reftests con JavaScript). `html/syntax/parsing` está en 99.9 % y `event-loops` en 75 %.

Las cifras de esta sección salen de `docs/roadmap.md` y `docs/test262-status.md`, con su
fuente en cada apartado, y de [`docs/rendimiento.md`](docs/rendimiento.md). **Avance por
criterios: 84.7 %** de las fases 2 a 5 (87.8 % contando la fase 1). Lo que falta está en la
lista "Criterios no cumplidos" del roadmap; cinco puntos esperan una decisión del usuario
(roadmap, "Decisiones pendientes del usuario").

## 8. Ramas en integración

Al 2026-11-13, `master` está en `03bbd8a`. `army/e4`, `army/e5` y `army/e6` ya están
integradas (hasta `276fa07`: raster a densidad 2, cascada, parseo de CSS, maqueta y
precarga de la caché); `276fa07` está en GitHub y `03bbd8a` (solo documentación de E4) falta
subirlo.

`git log --oneline master..army/e<n>` muestra qué trae cada rama que todavía no está en
`master`, y su nota (`git show army/e<n>:docs/pendiente/mac/e<n>.md`) dice qué está hecho y
qué falta. En curso al escribir esto:

| Rama | Qué trae | Nota |
|---|---|---|
| `army/e2` | `IntersectionObserver` y `ResizeObserver` | `docs/pendiente/mac/e2.md` de la rama, cuando la entregue |
| `army/e6` | Diagnóstico de lo que le falta a `asimetrix.co` | `docs/pendiente/mac/e6.md` de la rama, cuando la entregue |
| `army/e1`, `army/e3`, `army/e4`, `army/e5` | Primera pintura (ver la sección 9) | la de cada rama |

## 9. Lo primero al retomar

**Objetivo del ciclo: la primera pintura de Wikipedia por debajo de 500 ms** (hoy, 542-665
ms). La red no baja de unos 250 ms, así que el resto tiene que caber en unos 200. Dos vías:

- abaratar la cascada y la maqueta;
- adelantar a la espera de la red lo que no depende de la respuesta: instalar la ventana,
  parsear las hojas que están en caché y compilar los scripts que están en caché
  (`bf_js::code_cache` y `bf_web::precompile_window_scripts`, de E3; `preload.rs`, de E6,
  es donde se llamarían).

Por orden:

1. Integrar las ramas que estén listas y correr la verificación completa (sección 4).
2. Medir la primera pintura con el método de [`docs/rendimiento.md`](docs/rendimiento.md),
   con la máquina libre, y actualizar el criterio de la fase 5 en `docs/roadmap.md`.
3. Las decisiones pendientes del usuario (roadmap, "Decisiones pendientes del usuario"):
   lector de pantalla, fuzzer de IPC, los 8 tests de CSS Flexbox 2 de 3.6, los 2 subtests de
   `html/syntax/parsing` que no se pueden cumplir y el suelo de red de la primera pintura.

## 10. Prompt para pegar en Claude Code

```
Continúa el proyecto Freedom (navegador con motor propio en Rust) en este Mac.
Lee primero RETOMAR-EN-MAC.md en la raíz del repo, luego docs/roadmap.md (tabla
"Avance por criterios" y "Criterios no cumplidos") y las notas de docs/pendiente/mac/.
Comprueba con git log qué ramas army/e<n> tienen trabajo que master no tiene, y sigue el
orden de la sección 9 ("Lo primero al retomar"). No cambies las reglas del proyecto (sección 5) ni las del ciclo (sección 6). Dime el
estado y propón el siguiente reparto entre los seis ejecutores.
```
