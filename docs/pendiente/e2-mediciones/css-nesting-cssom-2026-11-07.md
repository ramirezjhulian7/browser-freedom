# Estado de WPT

## testharness

Generado por `cargo run --release -p bf-web --bin wpt-testharness -- --out docs/wpt-status.md`.

- Commit de WPT: `4ad507b28db0a636f528def9161d109fe11b93a3`; directorios: `css/css-nesting`, `css/cssom`; 285 documentos en 3 s.
- Cada `*.html` con `testharness.js`, `*.any.js` (variante `window`) y `*.window.js` corre en un agente nuevo: el parser con los scripts clásicos (inline y `src` desde `tests/wpt` montado en `http://web-platform.test:8000/`), el bucle de eventos con reloj virtual y un `testharnessreport.js` propio que pasa los resultados al runner. Límite: 10 s por documento (60 s con `<meta name=timeout content=long>` o `META: timeout=long`), un hilo por documento.
- `archivos OK`: el harness terminó con estado OK (no implica que pasen todos sus subtests). Detalle por subtest: `target/wpt-testharness/results.tsv`.

| Directorio | archivos | archivos OK | subtests | pass | fail | timeout | notrun | % pass |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| css/css-nesting | 24 | 23 | 126 | 100 | 20 | 0 | 6 | 79.4% |
| css/cssom | 261 | 237 | 1990 | 1555 | 425 | 2 | 8 | 78.1% |
| **Total** | 285 | 260 | 2116 | 1655 | 445 | 2 | 14 | 78.2% |

Las causas más frecuentes de los archivos sin estado OK:

- 13: `TIMEOUT `
- 3: `ERROR Uncaught ReferenceError: host is not defined`
- 1: `ERROR Test named 'Outer selectorText text mutation with inner style rule' specified 1 'cleanup' function,`
- 1: `ERROR Test named 'font-variant-ligatures: none serialization with non-default value for another longhand'`
- 1: `ERROR Uncaught ReferenceError: divNonConstructed is not defined`
- 1: `ERROR Uncaught ReferenceError: frm is not defined`
- 1: `ERROR Uncaught ReferenceError: kNonGenericFontFamilyKeywords is not defined`
- 1: `ERROR Uncaught ReferenceError: outer is not defined`
