# Retomar Freedom en un Mac

Este archivo sirve para continuar el proyecto en otra máquina sin el historial de la sesión anterior. Lo escribió el planificador el 2026-11-12, al cerrar el ciclo en Windows. Léelo entero antes de tocar nada; si eres una sesión de Claude Code, la sección 8 resume qué se espera de ti.

## 1. Qué es y dónde está

- **Freedom**: navegador con motor 100 % propio en Rust (HTML, CSS, layout, JavaScript, red, DOM), en 15 crates (`crates/bf-*`).
- **Repositorio**: `https://github.com/ramirezjhulian7/browser-freedom`
- **Rama base**: `master`. Lleva todo lo integrado.
- **Ramas `army/e1` … `army/e12`**: trabajo de cada bloque. `git log --oneline origin/master..origin/army/e<n>` muestra lo que una rama tiene y `master` no.
- **Plan y estado**: `docs/roadmap.md` (al día al 2026-11-12), `docs/wpt-status.md`, `docs/test262-status.md`, `docs/perf.md` y un archivo por bloque en `docs/pendiente/`.

## 2. Preparar el Mac (una sola vez)

```sh
# Herramientas
xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh     # en Windows se usó rustc 1.98.1
git config --global core.autocrlf input                           # el repo usa LF (.gitattributes)

# Código
git clone https://github.com/ramirezjhulian7/browser-freedom.git
cd browser-freedom
git submodule update --init          # html5lib-tests, css-parsing-tests, test262-parser-tests
sh tools/fetch-wpt.sh                # clon parcial de WPT en tests/wpt (commit en tools/wpt-commit.txt)
sh tools/fetch-test262.sh            # test262 en tests/test262 (commit en tools/test262-commit.txt)

# Primera comprobación
cargo build --release -p bf-shell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace               # sin BF_FULL: las suites pesadas corren una muestra
```

Abrir el navegador y capturar sin ventana:

```sh
cargo run --release -p bf-shell -- open https://es.wikipedia.org
./target/release/freedom render https://www.whatismybrowser.com/es/ -o x.png --height 4200
```

`BF_FULL=1 cargo test --workspace` corre test262 entero (lento). Los comandos de medición de WPT y test262 están en la sección "Pruebas" del `README.md`.

## 3. Qué esperar al compilar en macOS

El motor se desarrolló solo en Windows y nunca se ha compilado en macOS. Los crates de plataforma (`winit`, `softbuffer`, `arboard`, `rfd`, `rustls`) son multiplataforma, así que debería compilar. Lo que hay que revisar en la primera sesión:

- **Rutas del perfil.** El código lee `%LOCALAPPDATA%`, `%APPDATA%` y `%USERPROFILE%` y tiene una alternativa bajo `$HOME` para otros sistemas. Conviene pasarlas a las rutas propias de macOS (`~/Library/Application Support/Freedom`, `~/Library/Caches/Freedom`, `~/Downloads`). Archivos: `crates/bf-font/src/db.rs` (fuentes del sistema), `crates/bf-net/src/cache.rs` y `crates/bf-net/src/lib.rs` (caché HTTP, HSTS, cookies), `crates/bf-shell/src/script.rs` (localStorage, IndexedDB), `crates/bf-shell/src/window/mod.rs` (carpeta de usuario), `crates/bf-web/src/storage/mod.rs`, y los módulos de perfil, historial, marcadores y descargas de `bf-shell`. `docs/pendiente/e6.md` y `docs/pendiente/e11.md` listan las rutas con su equivalente.
- **Fuentes.** `bf-font` busca las fuentes del sistema; en macOS están en `/System/Library/Fonts` y `/Library/Fonts`. Los reftests usan Ahem (`tests/wpt/fonts`), pero los que dependen de fuentes del sistema pueden cambiar de resultado. No es una regresión: vuelve a medir y anota la línea base del Mac en `docs/wpt-status.md`.
- **Tests con rutas o tiempos.** Algunos tests abren servidores locales o escriben en la carpeta temporal. Si alguno falla solo en macOS, arréglalo en el test, no en el motor.
- **Atajos.** El shell usa `Ctrl`; en macOS lo natural es `Cmd`. Queda como mejora, no bloquea.
- **Nombre del binario.** `target/release/freedom` (sin `.exe`).

Trata la primera ronda en el Mac como una tarea propia: que compile, que pasen fmt, clippy y tests, y anotar la línea base. Un solo ejecutor basta.

## 4. Reglas del proyecto (no cambian)

- Motor 100 % propio. No se cambia a V8, QuickJS ni Boa. `#![forbid(unsafe_code)]`.
- Sin crates externos nuevos sin pedirlo. Los de plataforma permitidos están en el `README.md`.
- Spec literal con la sección citada en el código; la suite oficial manda (WPT, test262).
- **Identidad propia**: `crates/bf-infra/src/identity.rs` es la única fuente y no se toca. Nada imita a Chrome (`window.chrome`, plugins falsos, `vendor: "Google Inc."`). Si una página se rompe por detectar el agente de usuario, se anota en `docs/roadmap.md`, sección "Compatibilidad por UA".
- Antes de integrar en `master`: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` y `cargo test --workspace`.
- Preferencias del usuario: cuando dice "ábrelo", solo hay que abrir el navegador; las pruebas las hace él. Antes de que se agote el límite de uso se para, cada ejecutor deja commit y su `docs/pendiente/e<n>.md` al día, y se guarda el estado.

## 5. Qué hay en `master`

Motor de documentos (HTML, CSS, flex, grid con subgrid y lanes, tablas, sizing), texto bidireccional y cortes de línea Unicode, scroll real con sticky, `text-overflow`, SVG y WebP, JavaScript propio (test262 95 %, Intl, Temporal), DOM con Shadow DOM y custom elements, namespaces y parser XML, fetch, XHR, streams, IndexedDB entre pestañas, workers, Service Workers y Cache API, iframes con cookies de terceros, HTTP/2, caché HTTP, autenticación HTTP, HSTS, y el shell con pestañas, historial, marcadores, descargas, búsqueda en página, zoom de texto y opciones.

Prueba de aceptación de la fase, cumplida: `https://www.whatismybrowser.com/es/` reporta JavaScript, cookies y cookies de terceros habilitados. Identifica a Freedom como "Netscape Navigator 5", consecuencia esperada de la identidad propia.

Hallazgos de la última prueba real del usuario (2026-11-12), para comprobar en el Mac:

- Una pestaña cayó por un pánico al usar un nodo de otro documento. Arreglado (E1).
- Faltaban `Image` y `PerformanceObserver`. Añadidos (E3).
- Salían `TypeError` sin mensaje. Añadidos mensajes en los sitios más llamados (E3).
- Pendiente: que un pánico en una página muestre una página de error en vez de dejar la pestaña muerta (capturar el pánico en el hilo de la pestaña, `crates/bf-shell/src/window/worker.rs`).

## 6. Cómo se trabaja: planificador y ejecutores

El desarrollo se hizo con el mod "army" de Claude Code: una sesión planificadora reparte bloques a varias sesiones ejecutoras, cada una en su worktree y su rama `army/e<n>`.

- El planificador no edita archivos del repo. Reparte bloques, revisa, integra con `git merge --no-ff` por hash y corre la verificación completa en `master`.
- Los ejecutores solo corren los tests y clippy de los crates que tocan. Entregan `FINAL(E<n>) …` con el hash, tras `git merge master`.
- Si un merge choca, lo resuelve el ejecutor dueño de la rama, no el planificador.
- No se mezcla nada en `master` mientras corre una verificación.
- Estado de cada bloque en `docs/pendiente/e<n>.md`; solo quien tenga el encargo escribe en `docs/roadmap.md`.

**Lección principal de Windows**: doce ejecutores en 16 núcleos se estorban. Con la máquina libre la verificación completa tarda entre 5 y 15 minutos; con doce compilando, hasta 100. En un Mac M4 con 24 GB, usa 5 o 6 ejecutores como máximo (cada compilación del crate web ocupa unos 700 MB) y reparte por tandas: los que compilan `bf-web` o `bf-shell` son los caros.

Reparto de territorios que se venía usando:

| Bloque | Territorio |
|---|---|
| E1 | DOM: `bf-dom`, Shadow DOM, custom elements, serializador |
| E2 | `bf-web/src/{net,storage,cssom}`, CSS moderno en `bf-css` y `bf-style/values` |
| E3 | `bf-web/src/workers` (workers, Service Workers, Cache API), APIs sueltas de página |
| E4 | `bf-web/src/bom`, hilo de la pestaña en `bf-shell`, pintura (`builder`, `display_list`), animaciones |
| E5 | Layout: flex, grid, bloque |
| E6 | `bf-net` y `bf-shell/src/load.rs` |
| E7 | `bf-image`, `bf-svg`, efectos visuales en `bf-paint` |
| E8 | `bf-js` |
| E9 | Texto: `inline`, bidi, cortes de línea, escritura vertical, `bf-font` |
| E10 | Encodings, parser XML, namespaces, selectores |
| E11 | Shell de uso diario: cromo, historial, marcadores, descargas, opciones |
| E12 | Scroll, overflow, sticky, `object-fit`, imágenes responsivas |

Con 6 ejecutores en el Mac, agrupa: DOM y workers (E1+E3), red y almacenamiento (E2+E6), pintura y animación (E4+E7), layout y texto (E5+E9+E12), JavaScript (E8), shell y plataforma macOS (E11+E10).

## 7. Siguiente trabajo, por prioridad

1. **Portar a macOS** (sección 3) y fijar la línea base de mediciones.
2. **Arreglar el test rojo de `master`** (sección 9).
3. **Integrar lo que quedó en ramas** (sección 9): cada rama hace `git merge master`, pasa sus tests y entrega FINAL.
4. **Robustez en páginas reales**: página de error cuando una pestaña falla, páginas de error de red con la causa (`NetErrorKind` de `bf-net` ya existe), mover nodos entre ventanas (`adoptNode`), mensajes de error de las conversiones de `bf-js`.
5. **Animaciones y transiciones CSS** y `transform` (rama de E4).
6. **Escritura vertical** (E9), `border-image`, `filter`, `clip-path` (E7), `<picture>` y `srcset` (E12), sintaxis relativa de color y `@container` (E2).
7. **Más adelante**: `<video>` y `<audio>`, aceleración por GPU, modelo de procesos con sandbox, accesibilidad. Están en la fase 5 de `docs/roadmap.md`.

## 8. Prompt para pegar en Claude Code en el Mac

```
Continúa el proyecto Freedom (navegador con motor propio en Rust) en este Mac.
Lee primero RETOMAR-EN-MAC.md en la raíz del repo, luego docs/roadmap.md y los
archivos de docs/pendiente/. Primera tarea: que el proyecto compile y pase
cargo fmt --all --check, cargo clippy --workspace --all-targets -- -D warnings y
cargo test --workspace en macOS; adapta las rutas del perfil a macOS y anota la
línea base de mediciones. Hay un test rojo conocido (sección 9). No cambies las
reglas del proyecto (sección 4). Cuando esté en verde, dime el estado y propón
el reparto para 5 o 6 ejecutores.
```

## 9. Cierre en Windows (2026-11-12)

**`master`** quedó en `9d64fdd` más el commit que añade este archivo, subido a GitHub.

- Verificación de `9d64fdd`: `cargo fmt --all --check` limpio, clippy limpio, y `cargo test --workspace` en verde salvo **un test**: `cargo test -p bf-paint --test svg_image markers_on_vertices`. Los marcadores SVG en los vértices de una línea no se pintan.
- Ese test pasa en `9a10f87` y falla desde `9d7735d`, que es la mezcla de la rama de E10 (`crates/bf-html/src/xml.rs` reescrito: namespaces, DOCTYPE interno, `ATTLIST`). Lo más probable es que haya cambiado cómo llegan los atributos o el namespace de `<marker>` en un SVG leído como XML, o la búsqueda por `id` de `url(#m)`. No se investigó más.
- `9a10f87` es el último commit con toda la suite en verde.
- Las mezclas de E7, E2, E3 y E11 posteriores a `9a10f87` entraron con `cargo check --workspace` en vez de con FINAL verificado por su ejecutor, porque el usuario pidió subir lo que hubiera. La verificación completa de arriba las cubre.

**Ramas con trabajo que `master` no tiene** (todas en GitHub):

| Rama | Qué trae | Estado |
|---|---|---|
| `army/e4` (`cdda3ce`) | `transform`, `requestAnimationFrame`, marcos en el runner de WPT, animaciones, `PushSticky` | La más valiosa. Choca con `master` en `crates/bf-paint/src/builder.rs` y `display_list.rs`. Animaciones sin compilar en `bf-web`/`bf-shell` |
| `army/e12` (`c80024f`) | `object-fit` y `object-position` (97,8 % de sus tests), más `css-overflow` | Verificado en crates ligeros. Choca en `crates/bf-style/src/properties/mod.rs` |
| `army/e2` (`bc461c5`) | CSS moderno: `color-mix()`, espacios de color, `env()`, `@property`, funciones matemáticas (`css-color` 78,4 %) | FINAL en `69f7142`, verificado en `bf-css` y `bf-style`. Mezcla limpio |
| `army/e1` (`955ab6b`) | Mover nodos entre ventanas | A medias |
| `army/e5` (`8255eab`) | Posición estática de absolutos en flex, `safe` | A medias |
| `army/e8` (`8e1da61`) | Robustez de `bf-js`, Temporal en `Intl.DateTimeFormat`, ShadowRealm | A medias |
| `army/e9` (`fee7222`) | Escritura vertical | A medias y sin medir |
| `army/e11` (`05f245a`) | Notas de rutas de Windows y pendiente al día | Solo documentación sobre lo ya integrado |

Los últimos commits de `army/e4`, `army/e5`, `army/e8` y `army/e9` los hizo el planificador con lo que esos ejecutores dejaron sin commit al cerrar. Están sin compilar ni probar.

Cada `docs/pendiente/e<n>.md` de su rama dice qué está hecho, qué está a medias y los próximos pasos.
