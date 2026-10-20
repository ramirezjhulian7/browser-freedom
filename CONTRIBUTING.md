# Contribuir a browser-freedom

## La regla: spec literal + suite oficial

1. **Se implementa el texto del spec**, no el comportamiento observado de un navegador. El
   código cita la sección que implementa (`// HTML §13.2.6.4.7`, `/// CSS 2.2 §10.3.3`) y
   usa los nombres de los algoritmos del spec. Si algo no se implementa, se deja un
   `TODO(etiqueta)` con la fase del [roadmap](docs/roadmap.md) que lo resuelve.
2. **La suite oficial decide**: html5lib-tests, web-platform-tests, css-parsing-tests y
   (fase 2) test262. Una funcionalidad se da por terminada cuando sus tests pasan. Los
   números de cada suite se reportan antes y después del cambio.
3. **Ningún test se omite en silencio.** Si un test contradice el spec vigente o su
   referencia está mal:
   - se documenta en [`docs/wpt-status.md`](docs/wpt-status.md) (sección "Tests con
     expectativa dudosa", generada desde `KNOWN_ISSUES` en `crates/bf-paint/src/reftest.rs`);
   - o se marca como *stale* en el harness, con el motivo.

   No se agrega a una lista de omitidos sin explicación.
4. **Sin regresiones**: un cambio no baja ninguna suite. Si un test que "pasaba" deja de
   pasar porque antes pasaba por accidente, se explica en el cambio y en `wpt-status.md`.
5. **Motor propio**: solo los crates de plataforma listados en el [README](README.md#reglas-del-proyecto).
   Todo crate usa `#![forbid(unsafe_code)]`.

Antes de entregar un cambio:

```sh
cargo build --workspace --release
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo test -p bf-paint --test wpt_reftests -- --ignored --nocapture   # si toca estilo, layout o pintura
```

## Cómo añadir una propiedad CSS

Todas las propiedades viven en un único registry en `crates/bf-style/src/properties/`.

1. **El tipo del valor**, en `types.rs`:
   - para una lista de keywords, `keyword_enum!(Nombre { Variante = "keyword", ... })` y
     `keyword_prop!(Nombre)`;
   - para un valor compuesto, un tipo propio que implemente `PropValue` (`parse` desde
     `&[ComponentValue]` y `to_css` para la serialización de CSSOM).

   Los valores computados (longitudes absolutas, colores resueltos) se calculan en `compute`
   si el valor lo necesita.
2. **La entrada del registry**, en la macro `longhands!` de `mod.rs`, con el nombre, el campo
   de `ComputedStyle`, el tipo, el valor inicial (como texto CSS, que se parsea con el propio
   tipo), si se hereda y si es animable:

   ```rust
   Appearance "appearance" appearance: Appearance = "none", inherited: false, animatable: false;
   ```

   Con eso la propiedad se parsea, entra a la cascada (incluidos `inherit`, `initial`,
   `unset`, `revert`, `revert-layer` y `var()`), aparece en `dump-style` y se serializa.
3. **Shorthands y alias**, en `shorthands.rs`: un `ShorthandDef` con sus longhands y la
   función que expande el valor, o `alias!("nombre-legado" => Longhand)` para alias como
   `-webkit-appearance` o las propiedades lógicas.
4. **La hoja UA**, si el spec le da valores por defecto: `crates/bf-style/ua/html.css`
   (transcripción de HTML §15, con la sección citada) o `hints.css`/`src/hints.rs` para los
   presentational hints.
5. **El uso**, en `bf-layout` o `bf-paint`, leyendo `style.<campo>`.
6. **Tests**: casos en `crates/bf-style/tests/computed.rs` (parseo y valor computado), un caso
   de layout en `crates/bf-layout/tests/cases/NNN-nombre.html` (con su bloque
   `<!-- expect ... -->`, verificado a mano; `BF_LAYOUT_PRINT=1` imprime el volcado) o un PNG en
   `crates/bf-paint/tests/raster.rs` (`UPDATE_EXPECTED=1` lo genera; revisarlo a ojo), y los
   reftests de WPT del módulo correspondiente.

## Cómo añadir un directorio de WPT

Los reftests se corren con el harness de `crates/bf-paint/src/reftest.rs`. Sirve `tests/wpt`
como `http://web-platform.test/`, renderiza test y referencia a 800×600 con las fuentes del
sistema más Ahem, y compara píxeles con la `meta name=fuzzy` del test.

1. **Descargar el directorio**: agregarlo a `DIRS` en `tools/fetch-wpt.sh` y a `$Dirs` en
   `tools/fetch-wpt.ps1`, junto con sus directorios de soporte y referencias (por ejemplo,
   `css/css-foo/support`) si están fuera de él. Volver a correr el script: hace un sparse
   checkout del commit pineado en `tools/wpt-commit.txt`.
2. **Registrar el grupo**: agregar la ruta a `GROUPS` en `crates/bf-paint/src/reftest.rs`.
   Cada grupo es una fila de la tabla de `docs/wpt-status.md`.
3. **Probar el subconjunto**:

   ```sh
   cargo run --release -p bf-paint --bin wpt-reftests -- css/css-foo
   ```

   Los fallos quedan en `target/reftests/<ruta>-{actual,expected,diff}.png` y el detalle en
   `target/reftests/results.tsv`.
4. **Regenerar el estado**: la corrida completa
   (`cargo test -p bf-paint --test wpt_reftests -- --ignored --nocapture`) reescribe
   `docs/wpt-status.md`. Se commitea junto con el cambio y se actualiza la tabla del
   README.
5. **Analizar los fallos por causa** (tablas, writing modes, gradientes, ...) y anotar las
   referencias dudosas en `KNOWN_ISSUES`.

Para las suites que no son reftests (como `url/resources`, `html/syntax/parsing`), el harness
vive en el crate que implementa el spec (`crates/<crate>/tests/`). Imprime una tabla por
archivo con `passed`/`stale`/`skipped`/`failed`, y falla si `failed > 0`.

## Estilo

- Comentarios y documentación en español; los identificadores siguen los nombres del spec en
  inglés cuando los hay (`collapse_white_space`, `selectedness_setting`).
- `rustfmt.toml` del repositorio y clippy sin warnings; los `#[allow]` se limitan a una función
  (hoy solo `clippy::too_many_arguments` en tres funciones de layout y raster).
- Tablas generadas: con los scripts de `tools/` (`python -I`), nunca a mano. Los datos de
  origen se versionan en `crates/<crate>/data/`.
