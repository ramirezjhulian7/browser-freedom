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
| 16 | Protección anti-bots | 1 bloqueado: Stack Overflow (Cloudflare, 403; el reto no dibuja nada). Amazon (202) y Reddit pasaron el suyo | E1 (JS del reto) y E3 (cabeceras) | — |
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
