# Estado de WPT

## testharness

Generado por `cargo run --release -p bf-web --bin wpt-testharness -- --out docs/wpt-status.md`.

- Commit de WPT: `4ad507b28db0a636f528def9161d109fe11b93a3`; directorios: `IndexedDB`; 626 documentos en 80 s.
- Cada `*.html` con `testharness.js`, `*.any.js` (variante `window`) y `*.window.js` corre en un agente nuevo: el parser con los scripts clásicos (inline y `src` desde `tests/wpt` montado en `http://web-platform.test:8000/`), el bucle de eventos con reloj virtual y un `testharnessreport.js` propio que pasa los resultados al runner. Límite: 10 s por documento (60 s con `<meta name=timeout content=long>` o `META: timeout=long`), un hilo por documento.
- `archivos OK`: el harness terminó con estado OK (no implica que pasen todos sus subtests). Detalle por subtest: `target/wpt-testharness/results.tsv`.

| Directorio | archivos | archivos OK | subtests | pass | fail | timeout | notrun | % pass |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| IndexedDB | 623 | 580 | 3800 | 3633 | 105 | 35 | 27 | 95.6% |
| IndexedDB/crashtests | 3 | 3 | 3 | 3 | 0 | 0 | 0 | 100.0% |
| **Total** | 626 | 583 | 3803 | 3636 | 105 | 35 | 27 | 95.6% |

Las causas más frecuentes de los archivos sin estado OK:

- 19: `TIMEOUT `
- 6: `ERROR Error in remote http://web-platform.test:8000/IndexedDB/structured-clone.any.js: Uncaught ReferenceE`
- 6: `ERROR Uncaught ReferenceError: DOMMatrix is not defined`
- 5: `NORESULT tiempo agotado`
- 2: `ERROR Uncaught NetworkError: Failed to execute 'importScripts': The script at 'http://web-platform.test:80`
- 2: `ERROR Uncaught {message: "assert_true: transaction.abort event has fired expected true got undefined", sta`
- 1: `ERROR Error in remote http://web-platform.test:8000/IndexedDB/idbobjectstore_createIndex.any.worker.js: Un`
- 1: `ERROR Error in remote http://web-platform.test:8000/IndexedDB/idbtransaction_abort.any.worker.js: Uncaught`
