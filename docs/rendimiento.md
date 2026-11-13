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
