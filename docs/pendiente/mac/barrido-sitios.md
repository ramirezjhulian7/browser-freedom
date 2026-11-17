# Barrido de sitios reales (E3, 2026-11-17)

Qué le falta a Freedom para el uso diario, visto en 27 sitios públicos y conocidos, sin iniciar
sesión en ninguno. No se guardaron en el repo ni HTML, ni JavaScript, ni capturas de los sitios.

## Método

- **Binario:** release de `army/e3` en `942c376`, que es `master` `489e6f8` con el paso `dump`.
  Todavía no llevaba los arreglos de abajo.
- **Orden de cada sitio:**
  `BF_TRACE=1 freedom session <url> wait wait wait dump -o <carpeta fuera del repo>/<sitio>`.
  - Los sitios se cargaron de uno en uno, con un tope de 150 s.
  - La caché de disco y las cookies eran las del usuario. Salvo Wikipedia, eran primeras
    visitas.
  - La ventana medía 1180 × 800.
- **Carga de la máquina:** entre 23 y 48 durante el barrido, porque otro ejecutor compilaba.
  Los tiempos son indicativos: con la máquina libre bajan mucho (la primera pintura de
  Wikipedia sale en 944 ms aquí y en 328-363 ms en la medida del planificador).
- **Clasificación:** dos subagentes clasificaron las 27 capturas del último paso y las 27
  consolas y trazas.
- **Desglose de la carga** (`perf carga` y `perf parseo`), en ms: "red" es el documento.
  "Buscar scripts" es lo que el parser espera a los `<script src>`. "Árbol" es el
  tokenizador y el tree builder, con lo que el parser hace dentro.

## Los sitios

Ningún sitio tumbó la pestaña ni dio un pánico. Los tiempos están en ms.

| Sitio | Tipo | 1.ª pintura | Lo más lento | Consola | Captura |
|---|---|---:|---|---|---|
| `es.wikipedia.org/wiki/Navegador_web` | enciclopedia | 944 | — | limpia | bien; dos barras de desplazamiento sueltas (logo y barra del artículo) |
| `en.wikipedia.org/wiki/Web_browser` | enciclopedia | 1771 | — | limpia | sin globo ni logo (sale el texto alternativo); la imagen de la ficha en blanco; las mismas barras sueltas |
| `www.bbc.com/news` | noticias | 4387 | buscar scripts 2175 | limpia | **sin estilos**: el `<style>` de styled-components queda vacío (sus reglas entran por `insertRule`); logo gigante |
| `elpais.com` | noticias (es) | 5015 | red 2013 (redirige a `/america-colombia/`), buscar scripts 1330 | `accounts.google.com/gsi/client` falla | ~280 px en blanco arriba (anuncio); el selector de edición encima del logo; desborde horizontal; una foto sin imagen |
| `www.eltiempo.com` | noticias (es) | 19 742 | scripts 10 064, buscar scripts 2497 | `gsi/client` falla | ~310 px en blanco arriba con una barra suelta; un icono roto; desborde horizontal |
| `www.nytimes.com` | noticias | 38 702 | árbol 11 726, cascada 6209, scripts 5265 | 5 *custom elements* "ya definidos" (módulos evaluados dos veces); 5 promesas sin manejar | sin logo; controles de vídeo sin estilo encima del texto; bloques de carga |
| `www.google.com` | buscador | 671 | — | limpia | bien (portada básica) |
| `duckduckgo.com/?q=navegador+web` | buscador | 5349 | buscar scripts 2626 | limpia | **sin caja de búsqueda ni resultados**; menú fuera de pantalla; desborde horizontal |
| `www.bing.com` | buscador | 782 | — | limpia | bien; el pie gris sobre gris tapa los controles; un icono cortado; desborde horizontal |
| `developer.mozilla.org/es/docs/Web/HTML` | documentación | 3167 | scripts 2380 | limpia | bien; la caja de búsqueda sin lupa ni texto |
| `doc.rust-lang.org/book/` | documentación | 1010 | — | limpia | bien |
| `docs.python.org/3/` | documentación | 603 | — | `select.add is not a function` | bien (falta el selector de versión) |
| `github.com/rust-lang/rust` | código | 18 573 | maqueta 8274, cascada 5248, scripts 2622 | limpia | bien salvo las columnas de último commit y fecha, que siguen cargando |
| `gitlab.com/gitlab-org/gitlab` | código | 7472 | scripts 3474, buscar scripts 2646 | `SVGElement is not defined` (4) | **sin barra superior**; árbol de archivos y README en carga; avatar en blanco |
| `news.ycombinator.com` | foro | 568 | — | limpia | bien |
| `www.reddit.com/r/rust/` | foro | 159 (reto) | — | 3 módulos `data:` bloqueados por CSP | pasó el reto JS; tarjetas vacías y barra lateral en carga; un avatar con texto alternativo |
| `stackoverflow.com/questions` | foro | 237 (reto) | — | `403` | **bloqueado**: "Just a moment..." de Cloudflare en blanco |
| `www.amazon.com` | comercio | 766 | — | limpia | cabecera bien; 3 de 4 tarjetas del carrusel en blanco; **sin filas de productos** |
| `www.mercadolibre.com.co` | comercio (es) | 12 033 | buscar scripts 3461, árbol 1304, escrituras de caché 2749 | `bases.min.js` bloqueado por CSP | bien al cargar; **a los 17 s la pestaña pasa a "Esquema no soportado"** (`meli://…`); historial vacío |
| `www.openstreetmap.org` | mapas | 1676 | — | limpia | **el mapa es todo gris**, sin teselas ni controles; cabecera partida en dos líneas |
| `www.youtube.com` | vídeo | 28 185 | scripts 25 509 | `window.SVGElement.prototype` (no existe `SVGElement`); `Script terminado: tiempo agotado` | **solo el esqueleto de carga** |
| `vimeo.com` | vídeo | 7354 | buscar scripts 3412, scripts 2400 | limpia | **la zona principal en blanco**; desborde horizontal |
| `excalidraw.com` | aplicación | 3415 | scripts 2671 | limpia | **en blanco**: `#root` vacío |
| `react.dev` | aplicación (docs) | 10 658 | maqueta 5552, buscar scripts 1579 | limpia | bien salvo la lupa del buscador, de ~140 px; desborde horizontal |
| `www.rae.es` | institucional (es) | 28 054 | árbol 19 879, buscar scripts 5513; 60 maquetas forzadas por el CSSOM (15,9 s) | limpia | debajo de la barra del diccionario todo en blanco (el texto está en la maqueta: ¿blanco sobre fondos que no cargaron?); desborde horizontal |
| `www.gov.co` | gobierno (es) | 3417 | buscar scripts 1262 | aviso del sitio | **en blanco**: `html` y `body` miden 0 de alto (*web components* que no dibujan) |
| `www.apple.com/es/` | comercio (es) | 3730 | scripts 1315 | `DOMMatrix is not a constructor` | **sin barra de navegación** (sigue con `no-js`); falta la imagen del producto |

Resumen: 4 sitios se ven bien (Google, Hacker News, Rust Book, documentación de Python). Otros
9 tienen defectos menores: las dos Wikipedias, El País, El Tiempo, Bing, MDN, GitHub, Reddit
y react.dev. 12 tienen defectos graves, 1 está bloqueado (Stack Overflow) y 1 terminaba en una
página de error (Mercado Libre, ya arreglado). Ningún sitio muestra texto de `<noscript>`.

## Carencias, por número de sitios

Dueño propuesto: **E1** JavaScript, DOM y BOM; **E2** CSS, maqueta, pintado e imágenes; **E3**
red, caché, cookies, redirecciones, CSP, cabeceras y shell. "Arreglado" quiere decir en
`army/e3`, con test (más abajo).

| # | Carencia | Sitios | Dueño | Estado |
|---:|---|---|---|---|
| 1 | El JavaScript de la página no termina de montarla: zonas o páginas en blanco, esqueletos de carga que no se resuelven | 12: excalidraw, gov.co, YouTube, DuckDuckGo, Vimeo, GitLab, OpenStreetMap, Apple, Amazon, NYT, GitHub (columnas), Reddit | E1 | Sin diagnosticar uno a uno. Casi ninguno deja un error en la consola: hace falta ver qué espera cada uno (`dump` deja el DOM y la consola) |
| 2 | El parser espera a cada `<script src>` y los pedía de uno en uno: más de 1 s buscando scripts | 10: RAE 5,5 s, Mercado Libre 3,5, Vimeo 3,4, GitLab 2,6, DuckDuckGo 2,6, El Tiempo 2,5, BBC 2,2, react.dev 1,6, El País 1,3, gov.co 1,3 | E3 | **Arreglado**: descarga especulativa (HTML §13.2.8) |
| 3 | Desborde horizontal de la página | 7: El País, El Tiempo, DuckDuckGo, Bing, react.dev, RAE, Vimeo | E2 | — |
| 4 | Maquetas forzadas por el CSSOM (cada consulta de un script hace cascada y maqueta completas) | 7: NYT 17,0 s en 4, RAE 15,9 s en 60, El Tiempo 8,5 s en 19, Mercado Libre 3,5 s, GitLab 2,0 s, Amazon 1,4 s, Apple 1,2 s | E2 (con E1) | — (estilo y maqueta incrementales) |
| 5 | Imágenes que faltan o rotas | 6: Wikipedia en inglés (logo SVG que queda como bloque, imagen de la ficha), El País, Reddit, Amazon, Apple, RAE (probable) | E2 | — |
| 6 | Solapes de texto o cajas | 6: NYT, El País, react.dev, OpenStreetMap, Reddit, YouTube | E2 | — |
| 7 | Escrituras de la caché de disco en el camino de la carga (cada una recorría la carpeta entera) | 5 con más de 1 s: react.dev 4,7 s, Mercado Libre 2,7, RAE 2,2, Vimeo 1,6, NYT 1,4 | E3 | **Arreglado** |
| 8 | Cascada o maqueta patológicas en la primera maqueta | 3: GitHub (cascada 5,2 s, maqueta 8,3 s), react.dev (maqueta 5,6 s), NYT (cascada 6,2 s) | E2 | — |
| 9 | Ejecución de JavaScript lenta | 3: YouTube 25,5 s (con un script que agota el tiempo), El Tiempo 10,1 s, NYT 5,3 s | E1 | — |
| 10 | Barras de desplazamiento sueltas en contenedores | 3: las dos Wikipedias, El Tiempo | E2 | — |
| 11 | Iconos rotos o que faltan | 3: MDN, El Tiempo, Bing | E2 | — |
| 12 | Falta la interfaz `SVGElement` | 2: GitLab, YouTube (`window.SVGElement.prototype`) | E1 | — |
| 13 | CSP: los módulos importados (`import()`, `import`) se pedían sin el nonce del script que los importa, y `'strict-dynamic'` los bloqueaba | 2: Reddit, Mercado Libre | E3 | **Arreglado** |
| 14 | El `User-Agent` de Freedom recibe 403 de Google (`accounts.google.com/gsi/client`; con uno de Chrome, 200) | 2: El País, El Tiempo | decisión del usuario (`bf-infra/src/identity.rs` no se toca) | — |
| 15 | Elementos gigantes | 2: BBC (logo), react.dev (lupa: un SVG sin tamaño) | E2 | — |
| 16 | Protección anti-bots | 0: Stack Overflow parecía bloqueado, pero el reto de Cloudflare se supera en ~5,6 s y el barrido solo esperaba ~1 s (ver "Ronda 6"). Amazon (202) y Reddit también pasaron el suyo | — | **Diagnosticado**: no falta nada estándar |
| 17 | CSS-in-JS por `insertRule` que no se aplica | 1: BBC (styled-components) | E2 | — |
| 18 | Un módulo se evalúa dos veces (con y sin `crossorigin`): "Custom element … is already defined" | 1: NYT (5 módulos) | E1 | — |
| 19 | `DOMMatrix` no es constructor | 1: Apple | E1 | — |
| 20 | `HTMLSelectElement.prototype.add` | 1: documentación de Python | E1 | — |
| 21 | Una navegación a un esquema de otra aplicación (`meli://`) reemplazaba la página por "Esquema no soportado" | 1: Mercado Libre | E3 | **Arreglado** |
| 22 | Un `location.reload()` durante la primera carga dejaba la pestaña sin entrada del historial (`url` vacía, `historial=1/0`) | 1: Mercado Libre (primera visita) | E3 | **Arreglado** |
| 23 | `sessionStorage` no sobrevive a `location.reload()` en una página `file:` (una página que recarga una vez con una marca en `sessionStorage` recarga sin fin) | visto con un fixture, no en un sitio | E1 | — |

## Arreglado en `army/e3`

- **Descarga especulativa** (HTML §13.2.8), en `bf-shell/src/preload.rs::speculative_fetch`.
  - Con la respuesta del documento, sus hojas y scripts se piden en paralelo con el loader del
    documento, que junta las peticiones iguales.
  - Test: `bf-shell/tests/speculative_fetch.rs`.
  - Medido en primera visita, con carga 63-77 y una corrida, "buscar scripts" en ms:

    | Sitio | Antes | Después |
    |---|---:|---:|
    | BBC | 1704 | 256 |
    | GitLab | 3032 | 1002 |
    | El Tiempo | 2300 | 670 |
    | Vimeo | 2209 | 1054 |
    | DuckDuckGo | 2384 | 1363 |

- **Escrituras de la caché fuera del camino** (`bf-net/src/cache.rs`).
  - Todo lo que se guarda va al hilo de escritura.
  - La carpeta se recorre para recortarla cada `DISK_LIMIT / 32` bytes escritos, no en cada
    escritura.
  - `HttpCache::flush_all` al salir.
  - Test: `new_responses_are_written_off_the_path`.
  - Medido en primera visita con la carpeta de caché del usuario (2262 archivos) clonada, y
    carga 50:

    | Sitio | Escrituras en el camino, antes | Después | Trabajo en segundo plano |
    |---|---|---:|---:|
    | go.dev | 75 en 3353 ms | 0 | 84 ms |
    | svelte.dev | 47 en 982 ms | 0 | 76 ms |
    | w3.org | 8 en 324 ms | 0 | 36 ms |

- **Nonce de CSP en los módulos importados** (HTML §8.1.5.5, *descendant script fetch
  options*; CSP 3 §6.7.1.1).
  - Afecta a `bf-infra` (`ResourceLoader::load_script`), `bf-js` (`ModuleLoader::load_from`),
    `bf-web` (las opciones de fetch de cada script) y `bf-shell`.
  - Test: `bf-shell/tests/csp_modules.rs`.
- **Esquemas de otras aplicaciones** (HTML §7.4.2.2): la página se queda y la barra de estado
  lo dice. Test: `bf-shell/tests/external_schemes.rs`.
- **`location.reload()` con una carga en curso:** repite esa navegación. Test:
  `bf-shell/tests/reload_during_load.rs`.
- **`FREEDOM_NET_LOG`:** anota también las peticiones que fallan, con el error y el tiempo.
- **El paso `dump` de `freedom session`:** deja el DOM, el árbol de maqueta y la consola de
  la pestaña. Con él salió este barrido (`docs/rendimiento.md`, "Volcado de una sesión").

## Ronda 6 (2026-11-17)

### El reto de Cloudflare (Stack Overflow)

**Causa demostrada: no está bloqueado, es lento.** Con `freedom session
https://stackoverflow.com/questions` y 60 esperas (~20 s), el reto se supera y Stack Overflow
carga entero (captura con la lista de preguntas, sus hojas y sus scripts).

Lo que hace el reto en Freedom. Los tiempos son desde el inicio, con el registro de red de
este bloque y una instrumentación temporal de las llamadas a las APIs del host, sin commit:

| Tiempo | Qué pasa |
|---:|---|
| 0,25 s | `GET /questions` → **403** con "Just a moment...". Es la respuesta normal del reto, no un rechazo. |
| | La página trae su CSP en un `<meta>`: `script-src` con nonce y `'unsafe-eval'`; `connect-src`, `frame-src` y `child-src` a `challenges.cloudflare.com`; `worker-src blob:`. |
| 0,5 s | `orchestrate/precursor_interstitial/v1` (200, 623 KB ofuscados, con nonce) |
| | Usa `history.replaceState`, `createElement` y `appendChild`, `Performance.getEntries` y `PerformanceObserver.observe`, y `XMLHttpRequest` con `timeout`, `ontimeout`, `onerror`, `setRequestHeader` y `onreadystatechange`. |
| 1,4 s | primer `XMLHttpRequest` |
| 2,8 s | `scripts/precursor/…/main.js` |
| 3,5 s | segundo `XMLHttpRequest` y un `Worker` dedicado (`blob:`), que termina con `terminate` a los 4,4 s |
| 4,3 s | la página del reto, traducida: "Un momento…", "Verificación de seguridad en curso" |
| 5,3 s | `BroadcastChannel.postMessage` |
| 5,6 s | `POST /questions?__cf_chl_f_tk=…` → 200: Stack Overflow con sus hojas y scripts |

- **Ninguna excepción del motor.** La instrumentación que anotaba cada error creado por el
  motor, también los que el script atrapa, no vio ninguno durante el reto.
- **Ninguna API ausente.** Ni bloqueos de CSP ni errores en la consola.
- **Por qué el barrido lo dio por bloqueado:** sus 3 esperas (~1 s) cortaban el reto antes del
  primer `XMLHttpRequest`, con la página aún en blanco.

Lo que encontré en mi territorio y arreglé: las peticiones de los scripts (`XMLHttpRequest`,
`fetch()`, `EventSource`) van por `bf_net::streaming` y no salían en `FREEDOM_NET_LOG`. Ahora
salen, también las fallidas (`tests/net_log.rs`).

Queda sin medir si 5,6 s es lo que tarda el reto en otros navegadores (parte son esperas del
propio reto) o lo alarga la velocidad del JavaScript de Freedom (dueño: E1, si se quiere
seguir).

### Un `<iframe>` lento ya no congela su página (`asimetrix.co`)

- **Causa demostrada:** el hilo de la pestaña hacía la navegación del marco de una vez, dentro
  de una tarea: red, parseo y scripts.
  - El `<iframe>` de YouTube, que crea `YT.Player` hacia 1,5 s, lo retenía 2,7 s: 239 ms de
    red y 2500 ms de parseo y scripts.
  - Antes, los scripts del propio padre lo ocupaban de 1,0 a 1,7 s, y el bucle corría todas
    las tareas pendientes antes de pintar.
  - Mientras tanto la ventana no recibía cuadros. El titular, una animación de entrada, salía
    a los ~3,3 s de la primera pintura.
- **Arreglo** (E3):
  - la red del marco va en otro hilo y su documento se le da al parser en trozos, uno por
    tarea (HTML §7.4.2.2 y §13.2.2);
  - el bucle de la pestaña pinta como mucho cada 50 ms de tareas (HTML §8.1.7.3).
- **Ahora:** el marco termina en 17 tareas, y el titular se ve en la primera captura tras la
  primera pintura (0,33 s después).
- **Test:** `bf-shell/tests/slow_frame.rs`, un marco que tarda 2 s y un clic que se atiende
  antes.
- **Queda:** un solo script largo del marco sigue siendo una tarea larga; no se puede partir
  sin pausar el intérprete.

## Segundo barrido (ronda 6, 2026-11-17)

- **Método:** los mismos 27 sitios y los mismos pasos (`wait wait wait dump`), con el release de
  `master` `2238e5c`. Incluye lo del primer barrido; no incluye lo de la ronda 6 (el marco
  lento, el nonce de `<link rel=preload>`).
- **Carga:** 10-15.
- **Caché:** ya tenía los sitios, así que la primera pintura no es de primera visita como en el
  primero y no se puede comparar sin más.
- **Ningún sitio tumbó la pestaña.**
- **Clasificación:** un subagente comparó las capturas del último paso de los dos barridos.

| Sitio | 1.ª pintura (ms), 1.º → 2.º | Errores de consola, 1.º → 2.º | Captura, 2.º frente a 1.º |
|---|---|---|---|
| es.wikipedia.org | 944 → 477 | 0 → 0 | igual (bien) |
| en.wikipedia.org | 1771 → 680 | 0 → 0 | **mejor**: la imagen de la ficha y el candado de "semi-protected" ya salen; el logo sigue cortado |
| bbc.com | 4387 → 1452 | 0 → 0 | igual: sigue sin estilos |
| elpais.com | 5015 → 2211 | 1 → 1 (`gsi/client`, 403) | **mejor** (poco): el selector de edición tiene texto |
| eltiempo.com | 19 742 → 9390 | 1 → 1 (`gsi/client`, 403) | igual |
| nytimes.com | 38 702 → 1749 | 6 → 0 | **peor, pero no es el motor**: el sitio respondió 403 con la verificación de DataDome (`captcha-delivery.com`) en vez de la portada |
| google.com | 671 → 376 | 0 → 0 | igual (bien) |
| duckduckgo.com | 5349 → 2639 | 0 → 0 | igual: sin resultados |
| bing.com | 782 → 512 | 0 → 0 | igual |
| developer.mozilla.org | 3167 → 1692 | 0 → 0 | igual (bien) |
| doc.rust-lang.org | 1010 → 358 | 0 → 0 | igual (bien) |
| docs.python.org | 603 → 700 | 1 → 1 (`select.add`) | igual (bien) |
| github.com | 18 573 → 12 261 | 0 → 0 | igual |
| gitlab.com | 7472 → 2273 | 5 → 5 (`SVGElement`) | **mejor**: el avatar del proyecto ya sale; sigue sin la barra superior |
| news.ycombinator.com | 568 → 505 | 0 → 0 | igual (bien) |
| reddit.com | 159 → 2917 | 9 → 0 (los módulos de CSP) | igual a la vista, sin reto JS esta vez |
| stackoverflow.com | 237 → 206 | 0 → 0 | igual: con 3 esperas el reto de Cloudflare aún no terminó (necesita ~5,6 s) |
| amazon.com | 766 → 1451 | 0 → 0 | **mejor**: las 4 tarjetas del carrusel tienen imagen y texto; aparece un desborde horizontal |
| mercadolibre.com.co | 12 033 → 17 828 | 4 → 4 | **mejor**: ya no termina en "Esquema no soportado". Sus 4 errores eran ahora de `<link rel=preload as=script nonce>`; con la rama `army/e3` de la ronda 6, 0 errores |
| openstreetmap.org | 1676 → 1088 | 0 → 0 | igual: el mapa gris |
| youtube.com | 28 185 → 18 010 | 2 → 2 | igual: esqueletos |
| vimeo.com | 7354 → 2949 | 0 → 0 | igual (el menú salió en inglés: probablemente el sitio) |
| excalidraw.com | 3415 → 2476 | 0 → 0 | igual: en blanco |
| react.dev | 10 658 → 5058 | 0 → 0 | igual: la lupa gigante |
| rae.es | 28 054 → 9617 | 0 → 8 | **mejor**: sale la noticia principal, aunque desplazada a la derecha. Los 8 son avisos de CSP en "solo informe" del sitio, que ahora sirve un reto de Cloudflare; salían duplicados, y con la rama `army/e3` de la ronda 6 salen una vez cada uno (6) |
| gov.co | 3417 → 1585 | 0 → 0 | igual: en blanco |
| apple.com/es | 3730 → 1971 | 5 → 5 (`DOMMatrix`) | dudoso: cambia el color de la franja de financiación |

**Resumen del segundo barrido:**
- 6 mejoran: Wikipedia en inglés, El País, GitLab, Amazon, Mercado Libre y la RAE.
- 19 siguen igual.
- NYT sale peor porque el sitio sirvió su verificación antibots.
- Apple queda dudoso.

## Tercer barrido (ronda 7, 2026-11-17)

- **Método:** los mismos 27 sitios con el release de `master` `3a7c372`, ronda 6 integrada.
  Esta vez con 25 esperas (~8 s) antes del `dump`, porque DuckDuckGo y Stack Overflow necesitan
  unos 7 s. No incluye lo de la ronda 7 en `army/e3`, como el `Accept` de las imágenes.
- **Carga:** 42-68, así que la primera pintura no es comparable.
- **Comparación:** un subagente comparó las capturas del último paso con las del segundo
  barrido. Parte de lo que mejora puede deberse solo a la espera más larga; la columna lo dice.
- **Ningún sitio tumbó la pestaña.**

| Sitio | Errores de consola, 2.º → 3.º | Captura, 3.º frente a 2.º |
|---|---|---|
| es.wikipedia.org | 0 → 0 | **mejor**: el logotipo SVG y el panel "Apariencia" ya salen; la barrita suelta desapareció |
| en.wikipedia.org | 0 → 0 | **mejor**: el logo ya no sale como texto cortado; el panel "Appearance" tiene contenido |
| bbc.com | 0 → 0 | **mejor (grande)**: con estilos (cabecera, menú, titular con foto, columnas). Quedan "NEWS" tapado por la foto y titulares en Times |
| elpais.com | 1 → 1 (`gsi/client`, 403) | **mejor** (poco): sin desborde horizontal; la foto lateral (quizá por la espera) |
| eltiempo.com | 1 → 3 (`gsi/client`; "tiempo agotado"; `reading 'id'`) | **mejor**: imágenes (quizá por la espera), sin barra suelta, flechas del carrusel en su sitio; sigue el desborde |
| nytimes.com | 0 → 0 | igual: la verificación de DataDome en blanco (la IP, ver "Ronda 7") |
| google.com | 0 → 0 | igual (bien) |
| duckduckgo.com | 0 → 1 (`ResizeObserver loop`) | **mejor (grande)**: caja de búsqueda y resultados (por la espera de 8 s; E1 lo había visto) |
| bing.com | 0 → 1 (`reading 'submit'`) | **mejor**: la cabecera completa y el carrusel; sigue el desborde |
| developer.mozilla.org | 0 → 0 | igual (bien) |
| doc.rust-lang.org | 0 → 0 | igual (bien) |
| docs.python.org | 1 → 0 (`select.add`, arreglado) | **mejor**: los selectores de idioma y versión (el de idioma, muy ancho) |
| github.com | 0 → 0 | igual: las columnas del último commit en esqueleto |
| gitlab.com | 5 → 0 (`SVGElement`, arreglado) | **mejor**: la barra superior, la ruta, la estrella y el selector de rama; pero el avatar del proyecto ahora en blanco y el logo como "I." |
| news.ycombinator.com | 0 → 0 | igual (bien) |
| reddit.com | 0 → 0 | igual: esqueletos y números pegados |
| stackoverflow.com | 0 → 0 | **mejor (grande)**: pasó la verificación de Cloudflare (por la espera) y muestra "Newest Questions" |
| amazon.com | 0 → 2 (`crypto.subtle` con AES-CBC: `NotSupportedError`) | **mejor** (poco): la tarjeta 2×2 cabe y sin desborde horizontal |
| mercadolibre.com.co | 4 → 0 | **mejor** (poco): el banner respeta márgenes y esquinas |
| openstreetmap.org | 0 → 0 | **mejor**: la bienvenida, los controles y la escala; el mapa sigue sin teselas (con `master`, el 418; ver "Ronda 7") y los iconos de los controles salen como "?" |
| youtube.com | 2 → 1 (`Script terminado: tiempo agotado`) | igual: esqueletos |
| vimeo.com | 0 → 1 (React #418, hidratación) | **mejor**: la cabecera y el banner; el centro sigue en esqueleto |
| excalidraw.com | 0 → 1 (una promesa rechazada) | igual: "Loading scene…" (las fuentes síncronas, E2 en curso) |
| react.dev | 0 → 0 | **mejor**: la lupa ya no es gigante, sin desborde horizontal |
| rae.es | 8 → 0 | **mejor**: "Dudas rápidas" y logos de tamaño razonable; la noticia destacada sigue cortada a la derecha |
| gov.co | 0 → 1 (`reading 'datoconsultadousuario'`) | **mejor (grande)**: aparece buena parte de la portada; la parte de arriba sigue en blanco |
| apple.com/es | 5 → 3 (`ResizeObserver loop`) | **mejor** (poco): la barra de país; siguen sin el menú global ni la imagen de la portada |

**Resumen del tercer barrido:**
- **18 mejoran y 9 siguen igual; ninguno empeora.**
- **Cambios grandes:**
  - por el motor: la BBC con estilos, la lupa de react.dev, los logos de Wikipedia, los
    desbordes que desaparecieron (El País, Amazon, react.dev, RAE) y gov.co;
  - por la espera más larga: Stack Overflow pasa Cloudflare y salen los resultados de
    DuckDuckGo.
- **Lo que sigue en más sitios:**
  - aplicaciones de JavaScript pesado que se quedan en esqueletos: YouTube, excalidraw, Reddit,
    GitHub, GitLab, Vimeo, Amazon y gov.co (E1, y las fuentes de E2);
  - imágenes o iconos que no salen: OpenStreetMap, GitLab, Apple y Reddit (E2);
  - desbordes: El Tiempo, Bing y Vimeo.
- **Errores nuevos de consola** (de E1; salen porque los scripts corren más tiempo):
  - "tiempo agotado" en El Tiempo y YouTube;
  - `ResizeObserver loop` en DuckDuckGo y Apple;
  - `crypto.subtle` con AES-CBC en Amazon (sin implementar; antes `crypto.subtle` no existía y
    el sitio seguía otro camino).

## Ronda 7 (2026-11-17)

### OpenStreetMap: el 418 de las teselas

**Causa demostrada: el `Accept` de las imágenes (motor, E3, arreglado en `army/e3`).**

- **Lo que manda Freedom de verdad** (`FREEDOM_NET_LOG=cabeceras`, nuevo: las cabeceras de cada
  salto): las teselas llevaban `Referer: https://www.openstreetmap.org/`, la cookie
  `_osm_totp_token`, las Client Hints y **`Accept: */*`**.
- **Lo que hace el servidor** (`curl`, siempre con el `User-Agent` de Freedom):

  | Petición de `tile.openstreetmap.org/5/9/15.png` | Respuesta |
  |---|---|
  | `Referer` de openstreetmap.org, `Accept: */*`, con o sin token y `Sec-Fetch-*` | **418 "Access blocked"**: 6672 bytes, `X-Blocked: Access denied`, `Cache-Control: no-cache`; la misma imagen para todas las teselas |
  | `Referer` de openstreetmap.org, `Accept: image/…`, con o sin token y `Sec-Fetch-*` | la tesela (30 922 bytes, `max-age=89430`) |
  | Sin `Referer` | la tesela |

  La cookie no cambia nada: un token inventado da lo mismo. La nota de E1 de la ronda 6 tenía
  los dos tamaños al revés: los 6672 bytes eran el bloqueo, no la tesela.
- **La regla:** Fetch §4 (el algoritmo *fetch*, el paso de `Accept`) pide, para una petición
  sin `Accept` y destino `image`, `image/png,image/svg+xml,image/*;q=0.8,*/*;q=0.5`; para
  `style`, `text/css,*/*;q=0.1`; para los marcos, el de documento. Freedom mandaba `*/*` en
  los dos caminos: el pintado (`Fetcher::fetch`) y las `<img>` del motor
  (`Fetcher::fetch_request`).
- **Arreglo:** `bf_net::accept_for`. Test: `bf-net/tests/accept.rs`.
- **Después:** las 30 teselas llegan bien (tamaños distintos) y Leaflet las marca
  `leaflet-tile-loaded`.
- **Queda (E2):** el mapa sigue sin verse. Las teselas están cargadas, con `opacity: 1` y
  colocadas con `translate3d` dentro de capas de 0×0. Una tesela suelta con el mismo
  `transform` sí se pinta, y el decodificador PNG con paleta funciona.
- **De paso:** cada tesela se pide dos veces, una el motor y otra el pintado; el `<img>` y el
  pintado no comparten la descarga. Pendiente.

### NYT: la verificación antibots

**No es el motor: la reputación de la IP.**
- Desde el segundo barrido, `www.nytimes.com` responde **403** con la página de DataDome:
  `captcha-delivery.com`, un `<iframe>` de verificación.
- `curl` recibe el mismo 403 con el `User-Agent` de Freedom y con el de `curl`. En el primer
  barrido había respondido 200.
- **En Freedom:**
  - el marco de verificación carga (`perf marco`: red 265 ms, documento en 3 tareas);
  - con más espera, su script manda el `POST` de verificación (200);
  - la portada no se libera en ~7 s y se ve en blanco.
- **Sin ver:** lo que muestra el marco después del `POST` (un captcha u otra cosa).
- **Para medir NYT otra vez** hace falta otra red, o esperar a que DataDome olvide esta IP.

### Apple: "dudoso"

**Es el sitio, no el motor.**
- El color de la franja de financiación cambió en cada barrido (azul, azul marino y ahora gris
  oscuro).
- No depende del tema: con `BF_THEME=light` y `BF_THEME=dark` sale idéntica, gris oscura.
- La página está en una campaña de fondo oscuro (`globalheader-dark` en el `<body>`, "Regalos
  que ilusionan").
- La consola baja de 5 errores a 2 con lo de E1 (`DOMMatrix`). Quedan 2 "Uncaught
  ResizeObserver loop completed with undelivered notifications". Es el error que el estándar
  manda reportar en ese caso (Resize Observer, *deliver resize loop error notification*); que el bucle se dé aquí es cosa de E1
  mirarlo.

## Cuarto barrido (ronda 8, 2026-11-17)

- **Método:** los mismos 27 sitios, con 25 esperas antes del `dump`, como el tercero.
  - Binario: release de `master` `cb41001`, con la ronda 7 integrada: el compilador de E1
    (YouTube), las fuentes web asíncronas de E2 y el `Accept` por *destination* de E3.
  - Después del barrido, `master` pasó a `eb9776d` con la ronda 8 de E2. Esa ronda arregla el
    mapa de OpenStreetMap (`mix-blend-mode` en un grupo aislado), así que ese sitio se volvió a
    pasar con `eb9776d` (última fila de la tabla).
- **Carga:** de 15,8 a 76,5, así que los tiempos no son comparables.
- **Comparación:** un subagente comparó las capturas del paso 26 con las del tercer barrido y
  clasificó cada sitio. Para nytimes, openstreetmap, gov.co y apple-es miró también el DOM, la
  maqueta y la consola.
- **Ningún sitio tumbó la pestaña.**

| Sitio | Errores de consola, 3.º → 4.º | Captura, 4.º frente a 3.º | Estado |
|---|---|---|---|
| es.wikipedia.org | 0 → 0 | igual | menores: los radios de "Apariencia" salen sin marcar |
| en.wikipedia.org | 0 → 0 | igual | menores: los mismos radios sin marcar |
| bbc.com | 0 → 0 | igual | menores: "NEWS" tapado por la foto, titular en una serif genérica, hueco de anuncio |
| elpais.com | 1 → 1 (`gsi/client`, 403) | igual | menores: ~280 px en blanco arriba; el selector de edición tapa la "E" del logo |
| eltiempo.com | 3 → 3 | igual | menores: ~310 px en blanco arriba y desborde horizontal |
| nytimes.com | 0 → 0 | igual | **graves**: la verificación de DataDome en blanco (la IP; "Ronda 7") |
| google.com | 0 → 0 | igual | bien (la página básica que Google entrega a Freedom) |
| duckduckgo.com | 1 → 1 (`ResizeObserver loop`) | igual | bien |
| bing.com | 1 → 1 (`reading 'submit'`) | igual | menores: desborde horizontal; el pie, montado bajo las noticias |
| developer.mozilla.org | 0 → 0 | igual | menores: el botón de búsqueda sin icono; una franja gris vacía |
| doc.rust-lang.org | 0 → 0 | igual | bien |
| docs.python.org | 0 → 0 | igual | bien |
| github.com | 0 → 0 | igual | menores: las columnas del último commit siguen en esqueleto |
| gitlab.com | 0 → 0 | igual | menores: avatar del proyecto en blanco, logo como "I.", último commit en esqueleto |
| news.ycombinator.com | 0 → 0 | igual | bien |
| reddit.com | 0 → 0 | igual | **graves** (en el límite): el feed carga, pero las tarjetas destacadas y la barra lateral son esqueletos y hay números pegados ("161 k2.5 k") |
| stackoverflow.com | 0 → 0 | igual | bien (pasa el reto de Cloudflare) |
| amazon.com | 2 → 2 (`crypto.subtle` con AES-CBC) | igual | menores: la barra del carrusel sobre las tarjetas; "Imprescindibl/es" cortado |
| mercadolibre.com.co | 0 → 0 | igual | menores: asoman las diapositivas vecinas del banner |
| openstreetmap.org | 0 → 0 | igual | **graves** con `cb41001`: las teselas llegan (`leaflet-tile-loaded`), pero el mapa sale en blanco |
| youtube.com | 1 → 2 (`reading 'hostElement'`; `Cannot delete property 'renderGuide'`) | **mejor**: aparece la caja de búsqueda; el reloj de la barra baja de 24,8 s a 7,4 s y ya no hay "tiempo agotado" | **graves**: sin logo ni vídeos, solo esqueletos |
| vimeo.com | 1 → 1 (React #418) | igual | **graves**: el bloque principal y el buscador en esqueleto; desborde horizontal |
| excalidraw.com | 1 → 0 | igual | **graves**: "Loading scene…"; el lienzo no aparece |
| react.dev | 0 → 0 | igual | bien |
| rae.es | 0 → 0 | **mejor**: la primera diapositiva del carrusel (foto, titular y flechas), antes un hueco blanco | menores: la segunda diapositiva asoma cortada a la derecha |
| gov.co | 1 → 1 (`reading 'datoconsultadousuario'`) | igual | **graves**: cabecera, logo, buscador y "Trámites más consultados" miden 0 de alto; un indicador de carga girando |
| apple.com/es | 3 → 7 (`ResizeObserver loop`) | igual | **graves**: el menú global es una franja negra vacía y la imagen de la portada mide 0 de ancho |
| openstreetmap.org con `eb9776d` | 0 | **mejor**: el mapa entero, con sus teselas y rótulos | menores: los iconos de los controles salen como "?" |

**Resumen del cuarto barrido:**
- **Cambio frente al tercero:** 2 mejoran (YouTube y RAE), 25 siguen igual y ninguno empeora.
  Con `eb9776d`, OpenStreetMap también mejora.
- **Recuento con `cb41001`:**
  - **bien, 7:** google, duckduckgo, rust-book, python-docs, hackernews, stackoverflow y
    react;
  - **defectos menores, 12:** las dos Wikipedia, bbc, elpais, eltiempo, bing, mdn, github,
    gitlab, amazon, mercadolibre y rae;
  - **defectos graves, 8:** nytimes, reddit, openstreetmap, youtube, vimeo, excalidraw, gov.co
    y apple-es.
- **Recuento con `master` `eb9776d`:** 7 bien, 13 con defectos menores y 7 con defectos
  graves (OpenStreetMap pasa a menores).
- **Clasificaciones dudosas:**
  - Reddit podría contar como menores, porque el feed principal carga.
  - Google está bien respecto a la página básica que decide entregar, no a la de Chrome.
  - En rust-book no se ve la barra lateral de capítulos; puede que el sitio la oculte a
    1180 px.
- **Lo que sigue en más sitios:**
  - las aplicaciones de JavaScript pesado que se quedan en esqueletos (YouTube, Vimeo, Reddit,
    GitHub, GitLab y excalidraw; E1);
  - gov.co, que falla por un `TypeError` en JavaScript y no por la red (`e3.md`, ronda 8: 88
    respuestas 200/204 y CORS correcto);
  - imágenes e iconos que no salen (Apple, GitLab y los controles de OpenStreetMap; E2);
  - desbordes horizontales (El Tiempo, Bing y Vimeo);
  - `ResizeObserver loop` en Apple, que sube de 3 a 7 avisos.
- **No llegó a los 27 sitios:** la ronda 8. De E2 solo se vio OpenStreetMap con `eb9776d`; lo
  que E1 y E3 tengan sin integrar no está en ningún binario del barrido.

## Quinto barrido (tanda nueva, 2026-11-17)

- **Método:** los mismos 27 sitios con 25 esperas, con el release de `master` `fcdb9c7` (las
  rondas 8 de E1, E2 y E3 integradas). Lo de `army/e3` de esta tanda no está en el binario.
- **Carga:** de 9,9 a 16,4.
- **Comparación:** un subagente comparó las capturas del paso 26 con las del cuarto barrido, y
  para OpenStreetMap también con la del cuarto con `eb9776d`. Las regresiones aparentes se
  repitieron con el binario de `cb41001`.

| Sitio | Errores de consola, 4.º → 5.º | Captura, 5.º frente a 4.º | Estado |
|---|---|---|---|
| es.wikipedia.org | 0 → 0 | igual | menores |
| en.wikipedia.org | 0 → 0 | igual | menores |
| bbc.com | 0 → 0 | igual | menores |
| elpais.com | 1 → 1 | **mejor** (dudoso): sale el aviso de cookies donde había un bloque en blanco | menores |
| eltiempo.com | 3 → 3 | igual | menores: el hueco en blanco de arriba |
| nytimes.com | 0 → 0 | igual | **graves**: la verificación de DataDome en blanco |
| google.com | 0 → 0 | igual | bien |
| duckduckgo.com | 1 → 0 | igual | bien |
| bing.com | 1 → 1 | igual | menores |
| developer.mozilla.org | 0 → 0 | igual | menores |
| doc.rust-lang.org | 0 → 0 | igual | bien |
| docs.python.org | 0 → 0 | igual | bien |
| github.com | 0 → 0 | **peor**: la pestaña cae ("Esta página falló") | **graves** |
| gitlab.com | 0 → 0 | **mejor**: el logo del proyecto, el botón "Código" y "Propietarios de códigos"; la lista de archivos sigue en esqueleto | menores |
| news.ycombinator.com | 0 → 0 | igual | bien |
| reddit.com | 0 → 0 | igual | **graves** (en el límite) |
| stackoverflow.com | 0 → 0 | igual | bien |
| amazon.com | 2 → 0 (AES-CBC, de E1) | **mejor** (poco): la flecha del carrusel y el desplegable de cuenta | menores |
| mercadolibre.com.co | 0 → 0 | igual | menores |
| openstreetmap.org | 0 → 0 | **mejor** que el 4.º con `cb41001`; igual que con `eb9776d`: el mapa entero | menores: iconos de los controles como "?" |
| youtube.com | 2 → 2 | igual | **graves**: esqueletos |
| vimeo.com | 1 → 106 | **peor**: en blanco (`#__next` mide 0 de alto) | **graves** |
| excalidraw.com | 0 → 0 | **mejor**: ya no se queda en "Loading scene…"; salen la barra de herramientas, la bienvenida y el zoom | menores (quizá bien; la letra de la bienvenida no está comprobada) |
| react.dev | 0 → 0 | igual | bien |
| rae.es | 0 → 5 (CSP solo informe) | igual | menores |
| gov.co | 1 → 1 | **peor** (dudoso): una foto sale como su texto alternativo | **graves** |
| apple.com/es | 7 → 8 (`ResizeObserver loop`) | igual (el fondo de la portada, azul en vez de negro) | **graves** |

**Resumen del quinto barrido:**
- **Cambio frente al cuarto:**
  - 5 mejoran (El País, GitLab, Amazon, OpenStreetMap y excalidraw), 19 siguen igual y 3
    empeoran (GitHub, Vimeo y gov.co);
  - **ninguno de los tres empeora por el motor de esta tanda:**
    - **GitHub:** cae con `master` `fcdb9c7`, con `army/e3` y también con `cb41001` (1 de 2;
      en el cuarto barrido no cayó). El pánico es "Gc liberado" (`bf-js/src/heap.rs:127`),
      con `run_animation_frames` en la pila: un callback de `requestAnimationFrame` cuyo
      objeto ya liberó el recolector. Es de `bf-js` (E1), intermitente y anterior a esta
      tanda.
    - **Vimeo:** el binario de `cb41001` da hoy 200 errores ("client-side exception" de
      Next.js y `[object DOMException]`): cambió el sitio, no el motor. Queda sin
      diagnosticar (E1).
    - **gov.co:** una imagen que no salió; puede ser que no hubiera llegado a tiempo.
- **Recuento:** **7 bien, 13 con defectos menores y 7 con defectos graves**, el mismo que el
  cuarto con `eb9776d`. GitHub pasa a graves y excalidraw a menores.
- **Errores de consola nuevos:** los 5 de RAE son avisos de CSP en modo solo informe (el sitio
  manda ahora `Content-Security-Policy-Report-Only`), sin efecto en la página.

## Lo que no encontré

- **"El sitio tardó demasiado"** (un tiempo de lectura agotado en `bf-net`, que vio E4): no
  apareció en ningún sitio, y el registro de red no muestra ninguna petición fallida por
  tiempo.
- **El hueco de 10 s de RAE:** no es la red. No hay ninguna petición en curso ni fallida. Son
  las maquetas forzadas por el CSSOM (fila 4).

## Para repetirlo

```sh
freedom session <url> wait wait wait dump -o /tmp/barrido/<sitio>     # captura, DOM, maqueta, consola
BF_TRACE=1 freedom session <url> wait -o /tmp/x 2>&1 | grep -E "perf (carga|parseo|disco|especulativa)"
FREEDOM_NET_LOG=1 freedom session <url> wait -o /tmp/x 2>&1 | grep "^red:"   # también las fallidas
```

Para una primera visita sin tocar la caché ni las cookies del usuario, `HOME=<carpeta vacía>`
(en macOS la caché sale de `~/Library/Caches/Freedom`).
