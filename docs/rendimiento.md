# Rendimiento: cómo medir

Los dos criterios de la fase 5 del roadmap: **primera pintura** de Wikipedia en menos de
500 ms y **60 cuadros por segundo** al desplazarla. Las dos se miden con
`freedom session` (la lógica de la ventana sin abrir una) y `BF_TRACE=1`, en release.

## Preparación

```sh
cargo build --release -p bf-shell
PERFIL=/tmp/perfil-medicion          # cualquier carpeta; conserva cookies e historial
URL=https://es.wikipedia.org/wiki/Navegador_web
# Una vez, para llenar la caché de disco (~/Library/Caches/Freedom en macOS):
FREEDOM_PROFILE=$PERFIL ./target/release/freedom session $URL -o /tmp/calentar
uptime                               # anotar la carga junto a cada cifra
```

## Primera pintura

```sh
FREEDOM_PROFILE=$PERFIL BF_TRACE=1 ./target/release/freedom session $URL -o /tmp/fp 2>&1 | grep perf
```

Imprime tres líneas (los tiempos entre corchetes son segundos desde la navegación):

- `perf navegación <url>`: `App::start_load_full`, el comienzo.
- `perf carga <url>: red/caché · parseo · scripts · hojas · cascada · imágenes · maqueta ·
  lista (N nodos)`: lo mide el hilo de la pestaña al terminar el primer layout.
  - **red/caché**: descargar o validar el documento (`Loaded::fetch`).
  - **parseo**: el parser HTML sin el tiempo de los scripts (`Loaded::parse` − scripts).
  - **scripts**: compilar y evaluar los scripts del documento (`ScriptEnv::run_time`).
  - **hojas**: hojas externas y `@font-face` (`PageTimings::style` − cascada).
  - **cascada**: `style_document_with_sheets` (`PageTimings::cascade`).
  - **imágenes**, **maqueta**, **lista**: las fases de `bf_paint::prepare_page_animated`.
- `perf primera pintura: X desde la navegación (raster R)`: el final del primer raster de la
  página nueva en la ventana (`App::frame`), incluido el raster.

Para comparar la cascada con y sin el índice de reglas en el mismo binario y con la misma
carga: `BF_NO_RULE_INDEX=1` usa el recorrido completo de antes.

Con `BF_TRACE=1` cada cascada completa imprime además `perf cascada: emparejar … (índices …,
N reglas) · declaraciones … · valores …; de eso, pseudoelementos … (E elementos, P llamadas de
pseudoelementos, K sin content)` (`bf_style::cascade::Timings`):

- **emparejar**: `RuleIndex::matching` en cada grupo de hojas (con `:host`, `::slotted()` y
  los hints), y armar los índices.
- **declaraciones**: atributo `style`, hints, ordenarlas y expandir los shorthands (`expand`
  vuelve a analizar el valor de cada declaración en cada elemento).
- **valores**: los valores computados y la herencia (`Styler::compute`, `cascaded_value`).
- **pseudoelementos**: lo que de todo eso fue para `::before`, `::after` y `::marker`;
  *sin content* son los `::before`/`::after` a los que no les aplica ninguna declaración
  `content` ni `all` y no se calculan.

La cascada corre dos veces al cargar Wikipedia: la primera la pide
`bf_web::html::legacy_factories::check_lazy_images` durante el parseo (cuenta en **parseo**),
la segunda es la de `prepare_page_animated` (la de **cascada**).

### Por función, con el muestreador de macOS

Para repartir **parseo**, **red/caché**, **hojas** o **maqueta** por funciones sin tocar el
código, `sample` muestrea el proceso cada 1 ms. La sesión empieza en una página local con un
enlace y espera ~1 s, así el muestreo ya corre cuando empieza la navegación:

```sh
echo '<a href="'$URL'">Ir al artículo</a>' > /tmp/ir.html
FREEDOM_PROFILE=$PERFIL BF_TRACE=1 ./target/release/freedom session /tmp/ir.html \
    wait wait wait wait link:Ir scrollbench:200:40 -o /tmp/fp &
sample $(pgrep -n -f 'freedom session /tmp/ir.html') 4 1 -file /tmp/muestras.txt
```

El hilo del documento es `freedom-pestana-1`; cada muestra es ~1 ms.

## Desplazamiento

```sh
FREEDOM_PROFILE=$PERFIL BF_TRACE=1 ./target/release/freedom session $URL \
    scrollbench:120:40 scale:2 scrollbench:120:40 -o /tmp/scroll 2>&1 | grep perf
```

- `scrollbench:<cuadros>:<dy>` desplaza `dy` px por cuadro y mide el trabajo de la ventana en
  cada uno (lo que hace `redraw`: el scroll al hilo del documento, lo que llegó de él, lo
  `sticky` y el raster completo del cuadro). Imprime mediana, p95, máximo, el raster mediano
  de la página, lo que tarda lo de antes del cuadro (el scroll al hilo del documento, recibir
  y lo `sticky`) y cuántos cuadros pasan de 16,7 ms (60 por segundo). Lo que queda de la
  mediana es armar el cuadro de la ventana: la barra, las barras de desplazamiento y las
  copias. También la lista: ítems, capas (opacidad, máscara, efecto), `fixed` y `sticky`.
- `scale:2` rasteriza a 2 px de dispositivo por px lógico (pantalla Retina). Sin ventana el
  tamaño lógico es 1180 × 800 (el área de la página, 1180 × 704).

Con `BF_TRACE=1`, `scrollbench` imprime además:

- `perf raster: N cuadros incrementales, M en tiles enteros, K de una pasada (H hilos); L con
  los sticky en capa propia`: cómo se pintó cada cuadro (ver abajo).
- `perf grupos: margen … ; fixed x=… y=… ancho×alto, sticky x=… y=… ancho×alto`: los grupos
  que se tratan aparte al desplazar (px CSS de la página).

## Cómo pinta la ventana (desde la ronda 7)

`App::frame` ajusta el scroll a px de dispositivo enteros y pinta la página en **tiles fijos
en la página** (`bf_paint::raster`, 256 × 64 px CSS por tile, en px de dispositivo):

1. **Desplazamiento incremental** (`TiledFrame::scrolled`): si el cuadro anterior es de la
   misma lista, escala y tamaño, se corren sus filas (en el mismo pixmap) y se pintan solo los
   tiles de la franja que aparece y los de los grupos `fixed`, donde estaban y donde quedan.
   Los grupos `sticky` van **en capa propia** cuando se puede (desde el cierre 3): los tiles se
   pintan sin ellos y cada grupo se pinta aparte en una capa que no depende del scroll (solo
   de la fracción de px de dispositivo de su desplazamiento), que se reutiliza y se compone
   encima en cada cuadro, recortada a los `overflow` rectangulares de sus ancestros. Se puede
   si el grupo solo está dentro de recortes rectangulares (no de una capa de opacidad, una
   máscara, un efecto ni un recorte redondeado) y nada pintado después de él en la lista cae
   encima en ese cuadro: primero con la caja conservadora de cada ítem y, si toca, con la de
   los contornos de sus glifos (`display_list::ink_bounds`). Si no, sus tiles se repintan
   donde estaba y donde queda, como antes. En Wikipedia, el índice (351 × 495 px) y la barra
   de arriba van en capa propia en todos los cuadros.
2. **Tiles enteros en paralelo** (`TiledFrame::render`): para la primera pintura, una lista nueva
   (cambio de página, animaciones, el hilo del documento rehízo la maqueta) o un salto de más
   de un cuadro. Los tiles se reparten entre los núcleos (`set_raster_threads`, hasta 8), y en
   cada tile se saltan los ítems que no lo tocan.
3. **Una pasada** (`render_scrolled`) si la lista no admite tiles (un `filter`, grupos
   `fixed`/`sticky` anidados) o el scroll no cae en px enteros.

El cuadro de la ventana reutiliza su pixmap (`App::spare_frame`), copia la página encima y
rellena con el color del cromo solo lo que la página no cubre (`fill_outside`); la barra opaca
se copia sin componer.

Encima de la página, el cuadro de la ventana lleva las barras de desplazamiento, los
resaltados de buscar y de la selección y el cursor: se pintan con `render_over`, que
rasteriza y compone solo la caja que cubren sus ítems (antes, una capa del tamaño del viewport
compuesta entera en cada cuadro: 14 ms a escala 1 y 50-68 ms a escala 2 en Wikipedia). La
barra de la ventana se guarda pintada (`App::bar_cache`) y solo se vuelve a pintar si cambia
su maqueta, el tema, el DPI o, mientras algo carga, la fase de la animación (antes 4 ms a
escala 1 y 13-17 ms a escala 2 en cada cuadro).

Las dos primeras dan exactamente los mismos píxeles (`crates/bf-paint/tests/scroll_raster.rs`;
`sticky_layers_match_full_frame` desplaza con las capas `sticky` reutilizadas y compara cada
cuadro con el entero). Con la tercera difieren en el antialiasing de las curvas que cruzan el
borde de un tile (tiny-skia corta el camino en el borde del pixmap y eso cambia la subdivisión
de la curva) y, con un `sticky` translúcido en capa propia, en el redondeo de componer la capa
en 8 bits en vez de pintar directo sobre la página. En la página de prueba son 641 canales de
307 200 a escala 1 y 3040 de 1 228 800 a escala 2 (0,2 %), con una diferencia máxima de 13;
antes de las capas `sticky`, 9 y 63. Los reftests siguen usando `render_scrolled`. `render_over` da los
mismos píxeles que la capa entera compuesta (`render_over_matches_full_layer`).

## Resultados

| Fecha | Qué | Escala 1 | Escala 2 | Carga |
|---|---|---:|---:|---|
| 2026-11-12, ronda 5 | antes del índice y las capas | 267 ms (raster 236) | 1136 ms (raster 1008) | 55-59 |
| 2026-11-12, ronda 5 | índice de reglas, capas vacías, copia directa | 124 ms (raster 108) | 482 ms (raster 420) | 20-36 |
| 2026-11-12, ronda 7, A/B | antes (`master` `b4a3b30`) | 233,5 ms (raster 199,1) | 665,2 ms (raster 581,0) | 72 |
| 2026-11-12, ronda 7, A/B | tiles, incremental y en paralelo | 35,8 ms (raster 17,3) | 167,3 ms (raster 91,0) | 58 |
| 2026-11-13, cierre 1 | `eb7e5a3` + `master` `71444d2` | 36,2 ms (raster 16,9) | 173,6 ms (raster 99,5) | 30-49 |
| 2026-11-13, cierre 1 | capas con `render_over`, barra en caché | 18,8 ms (raster 15,3) | 57,8 ms (raster 42,5) | 57-85 |
| 2026-11-13, cierre 1 | lo mismo, repetido | 20,0 ms (raster 16,3) | 74,6 ms (raster 51,0) | 97-99 |
| 2026-11-13, cierre 3 | antes (`master` `21ea345`) | 20,9 ms (raster 17,1) | 59,1 ms (raster 47,0) | 15-25 |
| 2026-11-13, cierre 3 | `sticky` en capa propia, cuadro reutilizado | 8,0 ms (raster 5,5) | 17,7 ms (raster 10,8) | 21-36 |
| 2026-11-13, cierre 3, A/B ×3 | antes (`21ea345`) | 7,4 ms (raster 6,3) | 37,4 ms (raster 29,7) | 17-57 |
| 2026-11-13, cierre 3, A/B ×3 | después | 2,7 ms (raster 2,0) | 6,3 ms (raster 4,1) | 17-57 |

Mediana por cuadro de 120 cuadros de 40 px en `https://es.wikipedia.org/wiki/Navegador_web`
(área de página 1180 × 704 px CSS). La máquina estuvo muy cargada todo el tiempo: las cifras
absolutas cambian mucho con la carga, por eso el A/B se midió con los dos binarios seguidos.

**Cuadros por segundo** al cierre 3: con la máquina a carga 21-36, **125 fps a escala 1**
(8,0 ms) y **56 fps a escala 2** (17,7 ms); en el A/B final, con los otros ejecutores ya
parados (carga 17-57 bajando), 370 y 159 fps (2,7 y 6,3 ms, mediana de 3 corridas alternando
los binarios: antes 7,4 y 37,4 ms). La meta de 60 fps se cumple a escala 1 con cualquier carga
medida y a escala 2 cuando la máquina no está saturada. En el A/B, a escala 2 el binario de
antes recibe una lista nueva (1530 ítems) porque Wikipedia rehace la maqueta tras sus eventos
`scroll`; el de después termina los 120 cuadros antes de que llegue.

**Cuadros por segundo** (1000 / mediana) al cierre 1: **53 fps a escala 1** (18,8 ms) y
**17 fps a escala 2** (57,8 ms), con la máquina a carga 57-85. La meta de 60 fps (16,7 ms) no
se cumple todavía; a escala 1 el raster incremental (15,3 ms) ya cabe y el resto (unos
3,5 ms) es armar el cuadro de la ventana.

**Primera pintura** al cierre 1 (`eb7e5a3` + `master` `71444d2`, caché de disco caliente,
tres veces seguidas): 1311, 1205 y 1162 ms (mediana **1205 ms**, raster 18-22 ms) con carga
30-53; con carga 97-99, 2586, 1725 y 1975 ms. La meta de 500 ms no se cumple. En una carga
típica (la segunda): red/caché 287 ms, parseo 334, scripts 26, hojas 95, cascada 238,
imágenes 11, maqueta 168, lista 9 (4049 nodos).

**Primera pintura y cascada** al cierre 3 (A/B, 5 corridas alternando los binarios, carga
17-20; mediana):

| | antes (`21ea345`) | después |
|---|---:|---:|
| primera pintura | 1100,5 ms | 1076,4 ms |
| red/caché | 661,2 | 684,3 |
| parseo | 154,6 | 135,3 |
| hojas | 37,6 | 41,6 |
| **cascada** | **105,3** | **76,4** |
| maqueta | 68,4 | 78,6 |

Con la máquina cargada (carga 31-62, el mismo A/B): cascada 387,7 → 256,0 ms y primera
pintura 2143 → 1954 ms. Por partes, después y con carga 16: emparejar 46-50 ms, declaraciones
10, valores 19-21; los pseudoelementos, 13-14 ms (4034 de 4444 sin `content`, sin calcular).
La meta de 500 ms no se cumple: solo la red se lleva ~660 ms (ver la nota de E4,
`docs/pendiente/mac/e4.md`, cierre 3, para el reparto por funciones).

## Parseo de la primera pintura (E3, cierre ronda 5)

Con `BF_TRACE=1`, después de `perf carga` sale una línea con el detalle de **parseo**
(`Engine::parse`, `crates/bf-shell/src/script.rs`):

```
perf parseo <url>: ventana · parser (tokenizador y árbol · buscar scripts · link · img ·
microtasks · write) · después · copia
```

- **ventana**: instalar la ventana del motor (`bf_web::install`: las clases del DOM y las
  fábricas en JavaScript de streams, IndexedDB, Service Workers, Cache y Font Loading).
- **parser**: el parser HTML con lo que hace dentro. Entre paréntesis, lo que pasa en el
  navegador (`bf_web::bom::script::ParseProfile`): buscar los scripts externos, los `<link>` y
  los `<img>` que inserta el parser, los puntos de control de microtasks y
  `document.write`. **tokenizador y árbol** es lo que queda: el parser propiamente dicho.
- **después**: los `<meta>` de CSP, `sync_frames` (los marcos y las imágenes perezosas).
- **copia**: la copia del documento que va al navegador.

### Qué había en los 334 ms

En `https://es.wikipedia.org/wiki/Navegador_web` (233 KB de HTML, 4049 nodos), con la caché
de disco caliente, el parseo **no** era el parser: el tokenizador y el tree builder tardan
12-16 ms (8-9 ms el parser solo, medido aparte sobre el mismo HTML), buscar los scripts
menos de 1 ms y los `<img>` 1-2 ms. Lo grande era **después**, 200-320 ms: `sync_frames`
revisaba las 8 imágenes `loading=lazy` para ver si estaban cerca del viewport, y para eso el
motor JS hacía su propia cascada y su propia maqueta del documento entero, justo antes de
que el navegador hiciera las suyas para pintar.

El "lazy load intersection observer" mira la intersección después de maquetar, en *update
the rendering* (HTML §8.1.7.3, §4.8.4.3.9). Ahora, con navegador (una ventana con proveedor
de maquetas), `sync_frames` no la revisa mientras la maqueta del documento de ahora no
exista (`bf_web::cssom::awaiting_shell_layout`). `Engine::set_layout` la revisa siempre con
la maqueta que trae el navegador. Sin navegador (`freedom render`, los runners de WPT) nada
cambia. Lo que se pinta tampoco: `bf_paint` pide todos los `<img>` sin mirar `loading`.

A/B con dos binarios que solo difieren en ese cambio (los dos con las marcas), alternados,
5 corridas cada uno, carga de 1 minuto 13,1-13,6 (de 5 minutos, 37-44):

| | Parseo (mín. · mediana) | después | CPU de usuario (mediana) | Primera pintura (mín. · mediana) |
|---|---|---|---|---|
| antes | 253 · 307 ms | 198-318 ms | 0,82 s | 1314 · 1543 ms |
| después | 50 · 60 ms | 0,2-0,3 ms | 0,63 s | 1075 · 1271 ms |

Binario final (los tres cambios) contra el de antes (solo las marcas), alternados, 5
corridas cada uno, con la máquina mucho más cargada (carga de 1 minuto 64-72, de 5 minutos
50-53):

| | Parseo (mín. · mediana) | CPU de usuario (mediana) | Primera pintura (mín. · mediana) |
|---|---|---|---|
| antes | 917 · 1278 ms | 1,23 s | 3796 · 4262 ms |
| final | 128 · 163 ms | 0,98 s | 2451 · 2592 ms |

### Lo que queda del parseo (unos 60 ms)

- **ventana**, 35-55 ms: tres cuartos son compilar 258 KB de JavaScript propio del motor
  (`streams.js` 105 KB, `idb1.js` + `idb2.js` 87 KB, `service.js` 28 KB, `font_loading.js`
  23 KB, `cache.js` 14 KB) en cada ventana nueva. Ejecutarlo apenas cuenta. El parser de JS
  y el compilador ahora son un 10-15 % más rápidos (el hash rápido de las tablas de
  propiedades en el internado de nombres y en las constantes del compilador; los signos de
  puntuación se buscan solo entre los que empiezan por el carácter leído): en esos 258 KB,
  parseo 17,3 → 15,0 ms y compilación 15,3 → 13,6 ms (medianas de 21, alternados).
- **scripts** (la línea `perf carga`), 12-30 ms.
- **tokenizador y árbol**, 12-16 ms. El tokenizador en modo red guarda una foto de su
  estado en cada token para poder deshacerlo si se acaba la entrada; la foto copiaba el
  nombre del último start tag (una asignación por token). Ahora es compartido: el
  tokenizador sobre el HTML de Wikipedia pasa de 8,5 a 7,6 ms y el parser completo (`feed`
  y `finish`) de 13,1 a 12,0 ms (medianas de 21, alternados).

**red/caché** es ahora la parte mayor de la primera pintura: 620-710 ms en estas corridas
(287 ms en la medida de E4) con la caché de disco caliente.
