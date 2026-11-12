# Estado de WPT

## testharness

Generado por `cargo run --release -p bf-web --bin wpt-testharness -- --out docs/wpt-status.md`.

- Commit de WPT: `4ad507b28db0a636f528def9161d109fe11b93a3`; directorios: `streams`, `css/cssom`; 493 documentos en 10 s.
- Cada `*.html` con `testharness.js`, `*.any.js` (variante `window`) y `*.window.js` corre en un agente nuevo: el parser con los scripts clásicos (inline y `src` desde `tests/wpt` montado en `http://web-platform.test:8000/`), el bucle de eventos con reloj virtual y un `testharnessreport.js` propio que pasa los resultados al runner. Límite: 10 s por documento (60 s con `<meta name=timeout content=long>` o `META: timeout=long`), un hilo por documento.
- `archivos OK`: el harness terminó con estado OK (no implica que pasen todos sus subtests). Detalle por subtest: `target/wpt-testharness/results.tsv`.

| Directorio | archivos | archivos OK | subtests | pass | fail | timeout | notrun | % pass |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| css/cssom | 261 | 237 | 1990 | 1553 | 427 | 2 | 8 | 78.0% |
| streams | 7 | 5 | 63 | 62 | 1 | 0 | 0 | 98.4% |
| streams/piping | 39 | 39 | 690 | 684 | 6 | 0 | 0 | 99.1% |
| streams/readable-byte-streams | 33 | 33 | 747 | 732 | 15 | 0 | 0 | 98.0% |
| streams/readable-streams | 60 | 60 | 1091 | 1038 | 53 | 0 | 0 | 95.1% |
| streams/transferable | 11 | 10 | 82 | 66 | 16 | 0 | 0 | 80.5% |
| streams/transform-streams | 34 | 34 | 400 | 397 | 3 | 0 | 0 | 99.2% |
| streams/writable-streams | 48 | 48 | 591 | 585 | 6 | 0 | 0 | 99.0% |
| **Total** | 493 | 466 | 5654 | 5117 | 527 | 2 | 8 | 90.5% |

Las causas más frecuentes de los archivos sin estado OK:

- 14: `TIMEOUT `
- 3: `ERROR Uncaught ReferenceError: host is not defined`
- 2: `ERROR Uncaught NetworkError: Failed to execute 'importScripts': The script at 'http://web-platform.test:80`
- 1: `ERROR Test named 'font-variant-ligatures: none serialization with non-default value for another longhand'`
- 1: `ERROR Uncaught ReferenceError: divNonConstructed is not defined`
- 1: `ERROR Uncaught ReferenceError: frm is not defined`
- 1: `ERROR Uncaught ReferenceError: kNonGenericFontFamilyKeywords is not defined`
- 1: `ERROR Uncaught ReferenceError: outer is not defined`
