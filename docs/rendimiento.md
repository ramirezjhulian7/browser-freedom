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

## Desplazamiento

```sh
FREEDOM_PROFILE=$PERFIL BF_TRACE=1 ./target/release/freedom session $URL \
    scrollbench:120:40 scale:2 scrollbench:120:40 -o /tmp/scroll 2>&1 | grep perf
```

- `scrollbench:<cuadros>:<dy>` desplaza `dy` px por cuadro y mide el trabajo de la ventana en
  cada uno (lo que hace `redraw`: el scroll al hilo del documento, lo que llegó de él, lo
  `sticky` y el raster completo del cuadro). Imprime mediana, p95, máximo, el raster mediano
  y cuántos cuadros pasan de 16,7 ms (60 por segundo). También la lista: ítems, capas
  (opacidad, máscara, efecto), `fixed` y `sticky`.
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

Las dos primeras dan exactamente los mismos píxeles (`crates/bf-paint/tests/scroll_raster.rs`).
Con la tercera difieren solo en el antialiasing de las curvas que cruzan el borde de un tile:
tiny-skia corta el camino en el borde del pixmap y eso cambia la subdivisión de la curva. En la
página de prueba son 9 canales de 307 200 a escala 1 y 63 de 1 228 800 a escala 2, con una
diferencia máxima de 13. Los reftests siguen usando `render_scrolled`.

## Resultados

| Fecha | Qué | Escala 1 | Escala 2 | Carga |
|---|---|---:|---:|---|
| 2026-11-12, ronda 5 | antes del índice y las capas | 267 ms (raster 236) | 1136 ms (raster 1008) | 55-59 |
| 2026-11-12, ronda 5 | índice de reglas, capas vacías, copia directa | 124 ms (raster 108) | 482 ms (raster 420) | 20-36 |
| 2026-11-12, ronda 7, A/B | antes (`master` `b4a3b30`) | 233,5 ms (raster 199,1) | 665,2 ms (raster 581,0) | 72 |
| 2026-11-12, ronda 7, A/B | tiles, incremental y en paralelo | 35,8 ms (raster 17,3) | 167,3 ms (raster 91,0) | 58 |

Mediana por cuadro de 120 cuadros de 40 px en `https://es.wikipedia.org/wiki/Navegador_web`
(área de página 1180 × 704 px CSS). La máquina estuvo muy cargada todo el tiempo: las cifras
absolutas cambian mucho con la carga, por eso el A/B se midió con los dos binarios seguidos.
