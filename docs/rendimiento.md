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
- `perf imágenes pedidas: N (M <img> perezosas aplazadas)` (E2): cuántas imágenes pide cada
  maqueta (`bf_paint::load_images_in`); las `loading=lazy` lejanas no se piden.
- `perf primera pintura: X desde la navegación (raster R)`: el final del primer raster de la
  página nueva en la ventana (`App::frame`), incluido el raster.

Para comparar la cascada con y sin el índice de reglas en el mismo binario y con la misma
carga: `BF_NO_RULE_INDEX=1` usa el recorrido completo de antes.

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

- `perf raster: N cuadros incrementales, M en tiles enteros, K de una pasada (H hilos)`: cómo
  se pintó cada cuadro (ver abajo).
- `perf grupos: margen … ; fixed y=… alto=…, sticky y=… alto=…`: los grupos que se repintan
  aparte al desplazar.

## Cómo pinta la ventana (desde la ronda 7)

`App::frame` ajusta el scroll a px de dispositivo enteros y pinta la página en **tiles fijos
en la página** (`bf_paint::raster`, 256 × 64 px CSS por tile, en px de dispositivo):

1. **Desplazamiento incremental** (`scroll_frame`): si el cuadro anterior es de la misma
   lista, escala y tamaño, se corren sus filas y se pintan solo los tiles de la franja que
   aparece y los de los grupos `fixed` y `sticky`, donde estaban y donde quedan.
2. **Tiles enteros en paralelo** (`render_tiled`): para la primera pintura, una lista nueva
   (cambio de página, animaciones, el hilo del documento rehízo la maqueta) o un salto de más
   de un cuadro. Los tiles se reparten entre los núcleos (`set_raster_threads`, hasta 8), y en
   cada tile se saltan los ítems que no lo tocan.
3. **Una pasada** (`render_scrolled`) si la lista no admite tiles (un `filter`, grupos
   `fixed`/`sticky` anidados) o el scroll no cae en px enteros.

Encima de la página, el cuadro de la ventana lleva las barras de desplazamiento, los
resaltados de buscar y de la selección y el cursor: se pintan con `render_over`, que
rasteriza y compone solo la caja que cubren sus ítems (antes, una capa del tamaño del viewport
compuesta entera en cada cuadro: 14 ms a escala 1 y 50-68 ms a escala 2 en Wikipedia). La
barra de la ventana se guarda pintada (`App::bar_cache`) y solo se vuelve a pintar si cambia
su maqueta, el tema, el DPI o, mientras algo carga, la fase de la animación (antes 4 ms a
escala 1 y 13-17 ms a escala 2 en cada cuadro).

Las dos primeras dan exactamente los mismos píxeles (`crates/bf-paint/tests/scroll_raster.rs`).
Con la tercera difieren solo en el antialiasing de las curvas que cruzan el borde de un tile:
tiny-skia corta el camino en el borde del pixmap y eso cambia la subdivisión de la curva. En la
página de prueba son 9 canales de 307 200 a escala 1 y 63 de 1 228 800 a escala 2, con una
diferencia máxima de 13. Los reftests siguen usando `render_scrolled`. `render_over` da los
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

Mediana por cuadro de 120 cuadros de 40 px en `https://es.wikipedia.org/wiki/Navegador_web`
(área de página 1180 × 704 px CSS). La máquina estuvo muy cargada todo el tiempo: las cifras
absolutas cambian mucho con la carga, por eso el A/B se midió con los dos binarios seguidos.

**Cuadros por segundo** (1000 / mediana) al cierre 1: **53 fps a escala 1** (18,8 ms) y
**17 fps a escala 2** (57,8 ms), con la máquina a carga 57-85. La meta de 60 fps (16,7 ms) no
se cumple todavía; a escala 1 el raster incremental (15,3 ms) ya cabe y el resto (unos
3,5 ms) es armar el cuadro de la ventana.

**Primera pintura** al cierre 1 (`eb7e5a3` + `master` `71444d2`, caché de disco caliente,
tres veces seguidas): 1311, 1205 y 1162 ms (mediana **1205 ms**, raster 18-22 ms) con carga
30-53; con carga 97-99, 2586, 1725 y 1975 ms. La meta de 500 ms no se cumple. En una carga
típica (la segunda): red/caché 287 ms, parseo 334, scripts 26, hojas 95, cascada 238,
imágenes 11, maqueta 168, lista 9 (4049 nodos).
