# Estado de test262

## Parsing

Generado por `cargo test -p bf-js --test test262_parse -- --nocapture`.

- Commit de test262: `d53ebc8b1931d9f1541e43d59650776e4b766248`
- 51490 tests parseados (sin ejecutar): deben parsear, salvo los `negative: phase: parse`, que deben fallar. Sin flags, cada test se parsea en código no estricto y con `"use strict";` delante; con `module`, como módulo.
- `skipped`: tests de propuestas que agregan sintaxis y todavía no están en el spec:

- `decorators`: stage 3: `@dec` en clases.
- `source-phase-imports`: stage 3: `import source x from`.
- `source-phase-imports-module-source`: stage 3: `import source` (especificador de test262).
- `import-defer`: stage 3: `import defer * as ns from`.
- `export-defer`: stage 2.7: `export defer`.

| Directorio | passed | skipped | failed | % |
|---|---:|---:|---:|---:|
| annexB/built-ins | 241 | 0 | 0 | 100.00% |
| annexB/language | 845 | 0 | 0 | 100.00% |
| built-ins/AbstractModuleSource | 0 | 8 | 0 | 100.00% |
| built-ins/AggregateError | 25 | 0 | 0 | 100.00% |
| built-ins/Array | 3083 | 0 | 0 | 100.00% |
| built-ins/ArrayBuffer | 221 | 0 | 0 | 100.00% |
| built-ins/ArrayIteratorPrototype | 27 | 0 | 0 | 100.00% |
| built-ins/AsyncDisposableStack | 104 | 0 | 0 | 100.00% |
| built-ins/AsyncFromSyncIteratorPrototype | 38 | 0 | 0 | 100.00% |
| built-ins/AsyncFunction | 18 | 0 | 0 | 100.00% |
| built-ins/AsyncGeneratorFunction | 23 | 0 | 0 | 100.00% |
| built-ins/AsyncGeneratorPrototype | 48 | 0 | 0 | 100.00% |
| built-ins/AsyncIteratorPrototype | 13 | 0 | 0 | 100.00% |
| built-ins/Atomics | 389 | 0 | 0 | 100.00% |
| built-ins/BigInt | 77 | 0 | 0 | 100.00% |
| built-ins/Boolean | 51 | 0 | 0 | 100.00% |
| built-ins/DataView | 561 | 0 | 0 | 100.00% |
| built-ins/Date | 594 | 0 | 0 | 100.00% |
| built-ins/DisposableStack | 93 | 0 | 0 | 100.00% |
| built-ins/Error | 93 | 0 | 0 | 100.00% |
| built-ins/FinalizationRegistry | 47 | 0 | 0 | 100.00% |
| built-ins/Function | 509 | 0 | 0 | 100.00% |
| built-ins/GeneratorFunction | 23 | 0 | 0 | 100.00% |
| built-ins/GeneratorPrototype | 61 | 0 | 0 | 100.00% |
| built-ins/Infinity | 6 | 0 | 0 | 100.00% |
| built-ins/Iterator | 654 | 0 | 0 | 100.00% |
| built-ins/JSON | 166 | 0 | 0 | 100.00% |
| built-ins/Map | 204 | 0 | 0 | 100.00% |
| built-ins/MapIteratorPrototype | 11 | 0 | 0 | 100.00% |
| built-ins/Math | 327 | 0 | 0 | 100.00% |
| built-ins/NaN | 6 | 0 | 0 | 100.00% |
| built-ins/NativeErrors | 94 | 0 | 0 | 100.00% |
| built-ins/Number | 340 | 0 | 0 | 100.00% |
| built-ins/Object | 3411 | 0 | 0 | 100.00% |
| built-ins/Promise | 732 | 0 | 0 | 100.00% |
| built-ins/Proxy | 311 | 0 | 0 | 100.00% |
| built-ins/Reflect | 153 | 0 | 0 | 100.00% |
| built-ins/RegExp | 1879 | 0 | 0 | 100.00% |
| built-ins/RegExpStringIteratorPrototype | 17 | 0 | 0 | 100.00% |
| built-ins/Set | 383 | 0 | 0 | 100.00% |
| built-ins/SetIteratorPrototype | 11 | 0 | 0 | 100.00% |
| built-ins/ShadowRealm | 64 | 0 | 0 | 100.00% |
| built-ins/SharedArrayBuffer | 104 | 0 | 0 | 100.00% |
| built-ins/String | 1223 | 0 | 0 | 100.00% |
| built-ins/StringIteratorPrototype | 7 | 0 | 0 | 100.00% |
| built-ins/SuppressedError | 22 | 0 | 0 | 100.00% |
| built-ins/Symbol | 98 | 0 | 0 | 100.00% |
| built-ins/Temporal | 4605 | 0 | 0 | 100.00% |
| built-ins/ThrowTypeError | 14 | 0 | 0 | 100.00% |
| built-ins/TypedArray | 1453 | 0 | 0 | 100.00% |
| built-ins/TypedArrayConstructors | 738 | 0 | 0 | 100.00% |
| built-ins/Uint8Array | 70 | 0 | 0 | 100.00% |
| built-ins/WeakMap | 141 | 0 | 0 | 100.00% |
| built-ins/WeakRef | 29 | 0 | 0 | 100.00% |
| built-ins/WeakSet | 85 | 0 | 0 | 100.00% |
| built-ins/decodeURI | 55 | 0 | 0 | 100.00% |
| built-ins/decodeURIComponent | 56 | 0 | 0 | 100.00% |
| built-ins/encodeURI | 31 | 0 | 0 | 100.00% |
| built-ins/encodeURIComponent | 31 | 0 | 0 | 100.00% |
| built-ins/eval | 10 | 0 | 0 | 100.00% |
| built-ins/global | 29 | 0 | 0 | 100.00% |
| built-ins/isFinite | 15 | 0 | 0 | 100.00% |
| built-ins/isNaN | 15 | 0 | 0 | 100.00% |
| built-ins/parseFloat | 54 | 0 | 0 | 100.00% |
| built-ins/parseInt | 55 | 0 | 0 | 100.00% |
| built-ins/undefined | 8 | 0 | 0 | 100.00% |
| intl402 | 22 | 0 | 0 | 100.00% |
| intl402/Array | 2 | 0 | 0 | 100.00% |
| intl402/BigInt | 11 | 0 | 0 | 100.00% |
| intl402/Collator | 65 | 0 | 0 | 100.00% |
| intl402/Date | 12 | 0 | 0 | 100.00% |
| intl402/DateTimeFormat | 245 | 0 | 0 | 100.00% |
| intl402/DisplayNames | 57 | 0 | 0 | 100.00% |
| intl402/DurationFormat | 110 | 0 | 0 | 100.00% |
| intl402/FallbackSymbol | 2 | 0 | 0 | 100.00% |
| intl402/Intl | 66 | 0 | 0 | 100.00% |
| intl402/ListFormat | 81 | 0 | 0 | 100.00% |
| intl402/Locale | 190 | 0 | 0 | 100.00% |
| intl402/Number | 7 | 0 | 0 | 100.00% |
| intl402/NumberFormat | 251 | 0 | 0 | 100.00% |
| intl402/PluralRules | 53 | 0 | 0 | 100.00% |
| intl402/RelativeTimeFormat | 80 | 0 | 0 | 100.00% |
| intl402/Segmenter | 79 | 0 | 0 | 100.00% |
| intl402/String | 19 | 0 | 0 | 100.00% |
| intl402/Temporal | 2029 | 0 | 0 | 100.00% |
| intl402/TypedArray | 1 | 0 | 0 | 100.00% |
| language/arguments-object | 263 | 0 | 0 | 100.00% |
| language/asi | 102 | 0 | 0 | 100.00% |
| language/block-scope | 145 | 0 | 0 | 100.00% |
| language/comments | 52 | 0 | 0 | 100.00% |
| language/computed-property-names | 48 | 0 | 0 | 100.00% |
| language/destructuring | 19 | 0 | 0 | 100.00% |
| language/directive-prologue | 62 | 0 | 0 | 100.00% |
| language/eval-code | 347 | 0 | 0 | 100.00% |
| language/export | 3 | 0 | 0 | 100.00% |
| language/expressions | 10704 | 398 | 0 | 100.00% |
| language/function-code | 217 | 0 | 0 | 100.00% |
| language/future-reserved-words | 55 | 0 | 0 | 100.00% |
| language/global-code | 42 | 0 | 0 | 100.00% |
| language/identifier-resolution | 14 | 0 | 0 | 100.00% |
| language/identifiers | 268 | 0 | 0 | 100.00% |
| language/import | 26 | 103 | 0 | 100.00% |
| language/keywords | 25 | 0 | 0 | 100.00% |
| language/line-terminators | 41 | 0 | 0 | 100.00% |
| language/literals | 536 | 0 | 0 | 100.00% |
| language/module-code | 595 | 4 | 0 | 100.00% |
| language/punctuators | 11 | 0 | 0 | 100.00% |
| language/reserved-words | 27 | 0 | 0 | 100.00% |
| language/rest-parameters | 11 | 0 | 0 | 100.00% |
| language/source-text | 1 | 0 | 0 | 100.00% |
| language/statementList | 80 | 0 | 0 | 100.00% |
| language/statements | 9333 | 14 | 0 | 100.00% |
| language/types | 113 | 0 | 0 | 100.00% |
| language/white-space | 67 | 0 | 0 | 100.00% |
| **Total** | 51490 | 527 | 0 | 100.00% |

## Ejecución

Generado por `cargo run --release -p bf-js --bin test262 -- tests/test262 --out docs/test262-status.md`.

- Commit de test262: `d53ebc8b1931d9f1541e43d59650776e4b766248`; fecha: 2026-10-20; 52017 archivos en 152 s.
- Cada test corre en un realm nuevo con `assert.js`, `sta.js` y sus `includes`; sin flags, dos veces (no estricto y con `"use strict";`). Pasa solo si pasan todas sus ejecuciones.
- `skipped`: módulos (paso 5) y las propuestas de `UNSUPPORTED_FEATURES`. Todo lo demás que falla cuenta como `failed`, incluido lo que todavía no está implementado; `timeout`: presupuesto de instrucciones o 10 s. Las propuestas omitidas:
  - `decorators`: stage 3: `@dec` en clases.
  - `source-phase-imports`: stage 3: `import source x from`.
  - `source-phase-imports-module-source`: stage 3: `import source`.
  - `import-defer`: stage 3: `import defer * as ns from`.
  - `export-defer`: stage 2.7: `export defer`.
  - `error-stack-accessor`: stage 3: `Error.prototype.stack` como accessor; el motor usa la propiedad propia no estándar `stack` de los errores.
  - `nonextensible-applies-to-private`: stage 3: PrivateFieldAdd lanza TypeError en objetos no extensibles; ECMA-262 todavía lo permite y sus tests contradicen el spec vigente.
- Detalle por test: `target/test262/results.tsv`.
- `language/` (referencia de regresión de `tests/test262_run.rs`): 77.90%

| Directorio | passed | failed | skipped | timeout | % |
|---|---:|---:|---:|---:|---:|
| annexB/built-ins | 238 | 3 | 0 | 0 | 98.8% |
| annexB/language | 844 | 1 | 0 | 0 | 99.9% |
| built-ins/AbstractModuleSource | 0 | 0 | 8 | 0 | 100.0% |
| built-ins/AggregateError | 25 | 0 | 0 | 0 | 100.0% |
| built-ins/Array | 2900 | 183 | 0 | 0 | 94.1% |
| built-ins/ArrayBuffer | 0 | 221 | 0 | 0 | 0.0% |
| built-ins/ArrayIteratorPrototype | 17 | 10 | 0 | 0 | 63.0% |
| built-ins/AsyncDisposableStack | 0 | 104 | 0 | 0 | 0.0% |
| built-ins/AsyncFromSyncIteratorPrototype | 0 | 38 | 0 | 0 | 0.0% |
| built-ins/AsyncFunction | 11 | 7 | 0 | 0 | 61.1% |
| built-ins/AsyncGeneratorFunction | 9 | 14 | 0 | 0 | 39.1% |
| built-ins/AsyncGeneratorPrototype | 0 | 48 | 0 | 0 | 0.0% |
| built-ins/AsyncIteratorPrototype | 0 | 13 | 0 | 0 | 0.0% |
| built-ins/Atomics | 0 | 389 | 0 | 0 | 0.0% |
| built-ins/BigInt | 77 | 0 | 0 | 0 | 100.0% |
| built-ins/Boolean | 51 | 0 | 0 | 0 | 100.0% |
| built-ins/DataView | 0 | 561 | 0 | 0 | 0.0% |
| built-ins/Date | 586 | 8 | 0 | 0 | 98.7% |
| built-ins/DisposableStack | 0 | 93 | 0 | 0 | 0.0% |
| built-ins/Error | 58 | 0 | 35 | 0 | 100.0% |
| built-ins/FinalizationRegistry | 0 | 47 | 0 | 0 | 0.0% |
| built-ins/Function | 505 | 4 | 0 | 0 | 99.2% |
| built-ins/GeneratorFunction | 23 | 0 | 0 | 0 | 100.0% |
| built-ins/GeneratorPrototype | 61 | 0 | 0 | 0 | 100.0% |
| built-ins/Infinity | 6 | 0 | 0 | 0 | 100.0% |
| built-ins/Iterator | 654 | 0 | 0 | 0 | 100.0% |
| built-ins/JSON | 166 | 0 | 0 | 0 | 100.0% |
| built-ins/Map | 0 | 204 | 0 | 0 | 0.0% |
| built-ins/MapIteratorPrototype | 0 | 11 | 0 | 0 | 0.0% |
| built-ins/Math | 327 | 0 | 0 | 0 | 100.0% |
| built-ins/NaN | 6 | 0 | 0 | 0 | 100.0% |
| built-ins/NativeErrors | 94 | 0 | 0 | 0 | 100.0% |
| built-ins/Number | 340 | 0 | 0 | 0 | 100.0% |
| built-ins/Object | 3378 | 33 | 0 | 0 | 99.0% |
| built-ins/Promise | 0 | 732 | 0 | 0 | 0.0% |
| built-ins/Proxy | 310 | 0 | 1 | 0 | 100.0% |
| built-ins/Reflect | 153 | 0 | 0 | 0 | 100.0% |
| built-ins/RegExp | 1877 | 2 | 0 | 0 | 99.9% |
| built-ins/RegExpStringIteratorPrototype | 17 | 0 | 0 | 0 | 100.0% |
| built-ins/Set | 0 | 383 | 0 | 0 | 0.0% |
| built-ins/SetIteratorPrototype | 0 | 11 | 0 | 0 | 0.0% |
| built-ins/ShadowRealm | 0 | 60 | 4 | 0 | 0.0% |
| built-ins/SharedArrayBuffer | 0 | 104 | 0 | 0 | 0.0% |
| built-ins/String | 1223 | 0 | 0 | 0 | 100.0% |
| built-ins/StringIteratorPrototype | 7 | 0 | 0 | 0 | 100.0% |
| built-ins/SuppressedError | 0 | 22 | 0 | 0 | 0.0% |
| built-ins/Symbol | 97 | 1 | 0 | 0 | 99.0% |
| built-ins/Temporal | 0 | 4605 | 0 | 0 | 0.0% |
| built-ins/ThrowTypeError | 14 | 0 | 0 | 0 | 100.0% |
| built-ins/TypedArray | 0 | 1453 | 0 | 0 | 0.0% |
| built-ins/TypedArrayConstructors | 0 | 738 | 0 | 0 | 0.0% |
| built-ins/Uint8Array | 0 | 70 | 0 | 0 | 0.0% |
| built-ins/WeakMap | 0 | 141 | 0 | 0 | 0.0% |
| built-ins/WeakRef | 0 | 29 | 0 | 0 | 0.0% |
| built-ins/WeakSet | 0 | 85 | 0 | 0 | 0.0% |
| built-ins/decodeURI | 54 | 0 | 0 | 1 | 98.2% |
| built-ins/decodeURIComponent | 55 | 0 | 0 | 1 | 98.2% |
| built-ins/encodeURI | 31 | 0 | 0 | 0 | 100.0% |
| built-ins/encodeURIComponent | 31 | 0 | 0 | 0 | 100.0% |
| built-ins/eval | 10 | 0 | 0 | 0 | 100.0% |
| built-ins/global | 29 | 0 | 0 | 0 | 100.0% |
| built-ins/isFinite | 15 | 0 | 0 | 0 | 100.0% |
| built-ins/isNaN | 15 | 0 | 0 | 0 | 100.0% |
| built-ins/parseFloat | 54 | 0 | 0 | 0 | 100.0% |
| built-ins/parseInt | 55 | 0 | 0 | 0 | 100.0% |
| built-ins/undefined | 8 | 0 | 0 | 0 | 100.0% |
| intl402 | 0 | 22 | 0 | 0 | 0.0% |
| intl402/Array | 1 | 1 | 0 | 0 | 50.0% |
| intl402/BigInt | 5 | 6 | 0 | 0 | 45.5% |
| intl402/Collator | 0 | 65 | 0 | 0 | 0.0% |
| intl402/Date | 8 | 4 | 0 | 0 | 66.7% |
| intl402/DateTimeFormat | 0 | 245 | 0 | 0 | 0.0% |
| intl402/DisplayNames | 0 | 57 | 0 | 0 | 0.0% |
| intl402/DurationFormat | 0 | 110 | 0 | 0 | 0.0% |
| intl402/FallbackSymbol | 0 | 2 | 0 | 0 | 0.0% |
| intl402/Intl | 0 | 66 | 0 | 0 | 0.0% |
| intl402/ListFormat | 0 | 81 | 0 | 0 | 0.0% |
| intl402/Locale | 0 | 190 | 0 | 0 | 0.0% |
| intl402/Number | 3 | 4 | 0 | 0 | 42.9% |
| intl402/NumberFormat | 0 | 251 | 0 | 0 | 0.0% |
| intl402/PluralRules | 0 | 53 | 0 | 0 | 0.0% |
| intl402/RelativeTimeFormat | 0 | 80 | 0 | 0 | 0.0% |
| intl402/Segmenter | 0 | 79 | 0 | 0 | 0.0% |
| intl402/String | 7 | 12 | 0 | 0 | 36.8% |
| intl402/Temporal | 0 | 2029 | 0 | 0 | 0.0% |
| intl402/TypedArray | 0 | 1 | 0 | 0 | 0.0% |
| language/arguments-object | 201 | 62 | 0 | 0 | 76.4% |
| language/asi | 102 | 0 | 0 | 0 | 100.0% |
| language/block-scope | 145 | 0 | 0 | 0 | 100.0% |
| language/comments | 51 | 0 | 1 | 0 | 100.0% |
| language/computed-property-names | 48 | 0 | 0 | 0 | 100.0% |
| language/destructuring | 17 | 2 | 0 | 0 | 89.5% |
| language/directive-prologue | 62 | 0 | 0 | 0 | 100.0% |
| language/eval-code | 295 | 48 | 4 | 0 | 86.0% |
| language/export | 0 | 0 | 3 | 0 | 100.0% |
| language/expressions | 8427 | 2217 | 458 | 0 | 79.2% |
| language/function-code | 217 | 0 | 0 | 0 | 100.0% |
| language/future-reserved-words | 55 | 0 | 0 | 0 | 100.0% |
| language/global-code | 42 | 0 | 0 | 0 | 100.0% |
| language/identifier-resolution | 14 | 0 | 0 | 0 | 100.0% |
| language/identifiers | 268 | 0 | 0 | 0 | 100.0% |
| language/import | 0 | 0 | 129 | 0 | 100.0% |
| language/keywords | 25 | 0 | 0 | 0 | 100.0% |
| language/line-terminators | 41 | 0 | 0 | 0 | 100.0% |
| language/literals | 532 | 4 | 0 | 0 | 99.3% |
| language/module-code | 1 | 1 | 597 | 0 | 50.0% |
| language/punctuators | 11 | 0 | 0 | 0 | 100.0% |
| language/reserved-words | 26 | 0 | 1 | 0 | 100.0% |
| language/rest-parameters | 11 | 0 | 0 | 0 | 100.0% |
| language/source-text | 1 | 0 | 0 | 0 | 100.0% |
| language/statementList | 80 | 0 | 0 | 0 | 100.0% |
| language/statements | 6674 | 2637 | 36 | 0 | 71.7% |
| language/types | 113 | 0 | 0 | 0 | 100.0% |
| language/white-space | 67 | 0 | 0 | 0 | 100.0% |
| **language/ (total)** | 17526 | 4971 | 1229 | 0 | 77.9% |
| **Total** | 31981 | 18757 | 1277 | 2 | 63.0% |

### Los 30 errores más frecuentes

| # | tests | error |
|---:|---:|---|
| 1 | 6135 | `ReferenceError: Temporal is not defined` |
| 2 | 4523 | `InternalError: no implementado: async/await (requiere Promise, paso N)` |
| 3 | 2083 | `harness testTypedArray.js: ReferenceError: FloatNArray is not defined` |
| 4 | 1183 | `ReferenceError: Intl is not defined` |
| 5 | 741 | `ReferenceError: Promise is not defined` |
| 6 | 580 | `ReferenceError: ArrayBuffer is not defined` |
| 7 | 327 | `ReferenceError: Set is not defined` |
| 8 | 289 | `TestNError: Expected a TypeError but got a ReferenceError` |
| 9 | 231 | `InternalError: no implementado: import() (módulos, paso N)` |
| 10 | 185 | `harness resizableArrayBufferUtils.js: ReferenceError: UintNArray is not defined` |
| 11 | 182 | `ReferenceError: Map is not defined` |
| 12 | 175 | `ReferenceError: DataView is not defined` |
| 13 | 127 | `ReferenceError: SharedArrayBuffer is not defined` |
| 14 | 121 | `TestNError: Calling as constructor Expected a TypeError but got a ReferenceError` |
| 15 | 112 | `harness atomicsHelper.js: TypeError: Cannot read properties of undefined (reading 'getReport')` |
| 16 | 101 | `ReferenceError: Atomics is not defined` |
| 17 | 95 | `ReferenceError: UintNArray is not defined` |
| 18 | 92 | `ReferenceError: WeakMap is not defined` |
| 19 | 89 | `ReferenceError: DisposableStack is not defined` |
| 20 | 85 | `ReferenceError: IntNArray is not defined` |
| 21 | 76 | `TestN:AsyncTestFailure:TestNError: The promise should be rejected, but threw an exception: Promise is not defined` |
| 22 | 73 | `TestNError: Expected a RangeError but got a ReferenceError` |
| 23 | 72 | `ReferenceError: AsyncDisposableStack is not defined` |
| 24 | 57 | `ReferenceError: ShadowRealm is not defined` |
| 25 | 54 | `TestNError: Expected a TestNError but got a ReferenceError` |
| 26 | 52 | `ReferenceError: WeakSet is not defined` |
| 27 | 48 | `SyntaxError: Identifier 'arguments' has already been declared` |
| 28 | 43 | `ReferenceError: FinalizationRegistry is not defined` |
| 29 | 40 | `InternalError: no implementado: using (explicit-resource-management)` |
| 30 | 34 | `RangeError: Maximum call stack size exceeded` |
