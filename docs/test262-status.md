# Estado de test262

Las dos secciones las generan herramientas: "Parsing", `test262_parse` solo con `BF_FULL=1`
(sin la variable, `cargo test` parsea 1 de cada 50 archivos y no escribe aquí); "Ejecución", el
runner `test262` en release.

## Parsing

Generado por `BF_FULL=1 cargo test -p bf-js --test test262_parse -- --nocapture` (sin `BF_FULL` el test parsea 1 de cada 50 archivos y no escribe aquí).

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

- Commit de test262: `d53ebc8b1931d9f1541e43d59650776e4b766248`; fecha: 2026-11-07; 52017 archivos en 2872 s.
- Cada test corre en un realm nuevo con `assert.js`, `sta.js` y sus `includes`; sin flags, dos veces (no estricto y con `"use strict";`). Pasa solo si pasan todas sus ejecuciones. Los `module` corren una vez como módulo, con sus `_FIXTURE` desde el disco; un error al cargar o enlazar el grafo es de fase `resolution`.
- `skipped`: las propuestas de `UNSUPPORTED_FEATURES`. Todo lo demás que falla cuenta como `failed`, incluido lo que todavía no está implementado; `timeout`: presupuesto de instrucciones o 10 s. Las propuestas omitidas:
  - `decorators`: stage 3: `@dec` en clases.
  - `source-phase-imports`: stage 3: `import source x from`.
  - `source-phase-imports-module-source`: stage 3: `import source`.
  - `import-defer`: stage 3: `import defer * as ns from`.
  - `export-defer`: stage 2.7: `export defer`.
  - `error-stack-accessor`: stage 3: `Error.prototype.stack` como accessor; el motor usa la propiedad propia no estándar `stack` de los errores.
  - `nonextensible-applies-to-private`: stage 3: PrivateFieldAdd lanza TypeError en objetos no extensibles; ECMA-262 todavía lo permite y sus tests contradicen el spec vigente.
  - `await-dictionary`: propuesta: `Promise.allKeyed` y `Promise.allSettledKeyed`.
  - `immutable-arraybuffer`: propuesta: `transferToImmutable`, `sliceToImmutable` y `ArrayBuffer.prototype.immutable`.
  - `upsert`: propuesta: `Map`/`WeakMap.prototype.getOrInsert` y `getOrInsertComputed`.
  - `import-text`: stage 3: módulos de texto (`with { type: "text" }`).
  - `import-bytes`: stage 2.7: módulos de bytes (`with { type: "bytes" }`, sobre immutable ArrayBuffer).
- Detalle por test: `target/test262/results.tsv`.
- `language/` (referencia de regresión de `tests/test262_run.rs`): 99.95%

| Directorio | passed | failed | skipped | timeout | % |
|---|---:|---:|---:|---:|---:|
| annexB/built-ins | 239 | 2 | 0 | 0 | 99.2% |
| annexB/language | 845 | 0 | 0 | 0 | 100.0% |
| built-ins/AbstractModuleSource | 0 | 0 | 8 | 0 | 100.0% |
| built-ins/AggregateError | 25 | 0 | 0 | 0 | 100.0% |
| built-ins/Array | 3083 | 0 | 0 | 0 | 100.0% |
| built-ins/ArrayBuffer | 192 | 0 | 29 | 0 | 100.0% |
| built-ins/ArrayIteratorPrototype | 27 | 0 | 0 | 0 | 100.0% |
| built-ins/AsyncDisposableStack | 104 | 0 | 0 | 0 | 100.0% |
| built-ins/AsyncFromSyncIteratorPrototype | 38 | 0 | 0 | 0 | 100.0% |
| built-ins/AsyncFunction | 18 | 0 | 0 | 0 | 100.0% |
| built-ins/AsyncGeneratorFunction | 23 | 0 | 0 | 0 | 100.0% |
| built-ins/AsyncGeneratorPrototype | 48 | 0 | 0 | 0 | 100.0% |
| built-ins/AsyncIteratorPrototype | 12 | 1 | 0 | 0 | 92.3% |
| built-ins/Atomics | 380 | 0 | 9 | 0 | 100.0% |
| built-ins/BigInt | 77 | 0 | 0 | 0 | 100.0% |
| built-ins/Boolean | 51 | 0 | 0 | 0 | 100.0% |
| built-ins/DataView | 550 | 0 | 11 | 0 | 100.0% |
| built-ins/Date | 594 | 0 | 0 | 0 | 100.0% |
| built-ins/DisposableStack | 93 | 0 | 0 | 0 | 100.0% |
| built-ins/Error | 58 | 0 | 35 | 0 | 100.0% |
| built-ins/FinalizationRegistry | 47 | 0 | 0 | 0 | 100.0% |
| built-ins/Function | 508 | 1 | 0 | 0 | 99.8% |
| built-ins/GeneratorFunction | 23 | 0 | 0 | 0 | 100.0% |
| built-ins/GeneratorPrototype | 61 | 0 | 0 | 0 | 100.0% |
| built-ins/Infinity | 6 | 0 | 0 | 0 | 100.0% |
| built-ins/Iterator | 654 | 0 | 0 | 0 | 100.0% |
| built-ins/JSON | 166 | 0 | 0 | 0 | 100.0% |
| built-ins/Map | 171 | 0 | 33 | 0 | 100.0% |
| built-ins/MapIteratorPrototype | 11 | 0 | 0 | 0 | 100.0% |
| built-ins/Math | 327 | 0 | 0 | 0 | 100.0% |
| built-ins/NaN | 6 | 0 | 0 | 0 | 100.0% |
| built-ins/NativeErrors | 94 | 0 | 0 | 0 | 100.0% |
| built-ins/Number | 340 | 0 | 0 | 0 | 100.0% |
| built-ins/Object | 3411 | 0 | 0 | 0 | 100.0% |
| built-ins/Promise | 643 | 0 | 89 | 0 | 100.0% |
| built-ins/Proxy | 311 | 0 | 0 | 0 | 100.0% |
| built-ins/Reflect | 153 | 0 | 0 | 0 | 100.0% |
| built-ins/RegExp | 1879 | 0 | 0 | 0 | 100.0% |
| built-ins/RegExpStringIteratorPrototype | 17 | 0 | 0 | 0 | 100.0% |
| built-ins/Set | 383 | 0 | 0 | 0 | 100.0% |
| built-ins/SetIteratorPrototype | 11 | 0 | 0 | 0 | 100.0% |
| built-ins/ShadowRealm | 0 | 64 | 0 | 0 | 0.0% |
| built-ins/SharedArrayBuffer | 104 | 0 | 0 | 0 | 100.0% |
| built-ins/String | 1223 | 0 | 0 | 0 | 100.0% |
| built-ins/StringIteratorPrototype | 7 | 0 | 0 | 0 | 100.0% |
| built-ins/SuppressedError | 22 | 0 | 0 | 0 | 100.0% |
| built-ins/Symbol | 98 | 0 | 0 | 0 | 100.0% |
| built-ins/Temporal | 4605 | 0 | 0 | 0 | 100.0% |
| built-ins/ThrowTypeError | 14 | 0 | 0 | 0 | 100.0% |
| built-ins/TypedArray | 1444 | 1 | 8 | 0 | 99.9% |
| built-ins/TypedArrayConstructors | 736 | 0 | 2 | 0 | 100.0% |
| built-ins/Uint8Array | 68 | 0 | 2 | 0 | 100.0% |
| built-ins/WeakMap | 102 | 0 | 39 | 0 | 100.0% |
| built-ins/WeakRef | 29 | 0 | 0 | 0 | 100.0% |
| built-ins/WeakSet | 85 | 0 | 0 | 0 | 100.0% |
| built-ins/decodeURI | 55 | 0 | 0 | 0 | 100.0% |
| built-ins/decodeURIComponent | 56 | 0 | 0 | 0 | 100.0% |
| built-ins/encodeURI | 31 | 0 | 0 | 0 | 100.0% |
| built-ins/encodeURIComponent | 31 | 0 | 0 | 0 | 100.0% |
| built-ins/eval | 10 | 0 | 0 | 0 | 100.0% |
| built-ins/global | 29 | 0 | 0 | 0 | 100.0% |
| built-ins/isFinite | 15 | 0 | 0 | 0 | 100.0% |
| built-ins/isNaN | 15 | 0 | 0 | 0 | 100.0% |
| built-ins/parseFloat | 54 | 0 | 0 | 0 | 100.0% |
| built-ins/parseInt | 55 | 0 | 0 | 0 | 100.0% |
| built-ins/undefined | 8 | 0 | 0 | 0 | 100.0% |
| intl402 | 22 | 0 | 0 | 0 | 100.0% |
| intl402/Array | 2 | 0 | 0 | 0 | 100.0% |
| intl402/BigInt | 10 | 1 | 0 | 0 | 90.9% |
| intl402/Collator | 62 | 3 | 0 | 0 | 95.4% |
| intl402/Date | 12 | 0 | 0 | 0 | 100.0% |
| intl402/DateTimeFormat | 159 | 86 | 0 | 0 | 64.9% |
| intl402/DisplayNames | 4 | 53 | 0 | 0 | 7.0% |
| intl402/DurationFormat | 0 | 110 | 0 | 0 | 0.0% |
| intl402/FallbackSymbol | 2 | 0 | 0 | 0 | 100.0% |
| intl402/Intl | 61 | 5 | 0 | 0 | 92.4% |
| intl402/ListFormat | 80 | 1 | 0 | 0 | 98.8% |
| intl402/Locale | 189 | 1 | 0 | 0 | 99.5% |
| intl402/Number | 7 | 0 | 0 | 0 | 100.0% |
| intl402/NumberFormat | 209 | 42 | 0 | 0 | 83.3% |
| intl402/PluralRules | 51 | 2 | 0 | 0 | 96.2% |
| intl402/RelativeTimeFormat | 73 | 7 | 0 | 0 | 91.2% |
| intl402/Segmenter | 2 | 77 | 0 | 0 | 2.5% |
| intl402/String | 15 | 4 | 0 | 0 | 78.9% |
| intl402/Temporal | 72 | 1957 | 0 | 0 | 3.5% |
| intl402/TypedArray | 1 | 0 | 0 | 0 | 100.0% |
| language/arguments-object | 261 | 2 | 0 | 0 | 99.2% |
| language/asi | 102 | 0 | 0 | 0 | 100.0% |
| language/block-scope | 145 | 0 | 0 | 0 | 100.0% |
| language/comments | 52 | 0 | 0 | 0 | 100.0% |
| language/computed-property-names | 48 | 0 | 0 | 0 | 100.0% |
| language/destructuring | 18 | 1 | 0 | 0 | 94.7% |
| language/directive-prologue | 62 | 0 | 0 | 0 | 100.0% |
| language/eval-code | 347 | 0 | 0 | 0 | 100.0% |
| language/export | 3 | 0 | 0 | 0 | 100.0% |
| language/expressions | 10699 | 4 | 399 | 0 | 100.0% |
| language/function-code | 217 | 0 | 0 | 0 | 100.0% |
| language/future-reserved-words | 55 | 0 | 0 | 0 | 100.0% |
| language/global-code | 42 | 0 | 0 | 0 | 100.0% |
| language/identifier-resolution | 14 | 0 | 0 | 0 | 100.0% |
| language/identifiers | 268 | 0 | 0 | 0 | 100.0% |
| language/import | 16 | 0 | 113 | 0 | 100.0% |
| language/keywords | 25 | 0 | 0 | 0 | 100.0% |
| language/line-terminators | 41 | 0 | 0 | 0 | 100.0% |
| language/literals | 532 | 4 | 0 | 0 | 99.3% |
| language/module-code | 595 | 0 | 4 | 0 | 100.0% |
| language/punctuators | 11 | 0 | 0 | 0 | 100.0% |
| language/reserved-words | 27 | 0 | 0 | 0 | 100.0% |
| language/rest-parameters | 11 | 0 | 0 | 0 | 100.0% |
| language/source-text | 1 | 0 | 0 | 0 | 100.0% |
| language/statementList | 80 | 0 | 0 | 0 | 100.0% |
| language/statements | 9331 | 0 | 16 | 0 | 100.0% |
| language/types | 113 | 0 | 0 | 0 | 100.0% |
| language/white-space | 67 | 0 | 0 | 0 | 100.0% |
| **language/ (total)** | 23183 | 11 | 532 | 0 | 100.0% |
| **Total** | 48791 | 2429 | 797 | 0 | 95.3% |

### Los 30 errores más frecuentes

| # | tests | error |
|---:|---:|---|
| 1 | 263 | `RangeError: Unsupported calendar: gregory` |
| 2 | 139 | `RangeError: Unsupported calendar: buddhist` |
| 3 | 138 | `RangeError: Unsupported calendar: chinese` |
| 4 | 135 | `RangeError: Unsupported calendar: japanese` |
| 5 | 131 | `RangeError: Unsupported calendar: hebrew` |
| 6 | 120 | `RangeError: Unsupported calendar: dangi` |
| 7 | 114 | `RangeError: Unsupported calendar: roc` |
| 8 | 106 | `RangeError: Unsupported calendar: islamic-civil` |
| 9 | 104 | `RangeError: Unsupported calendar: ethiopic` |
| 10 | 93 | `RangeError: Unsupported calendar: islamic-tbla` |
| 11 | 84 | `RangeError: Unsupported calendar: coptic` |
| 12 | 84 | `RangeError: Unsupported calendar: ethioaa` |
| 13 | 83 | `RangeError: Unsupported calendar: persian` |
| 14 | 82 | `RangeError: Unsupported calendar: islamic-umalqura` |
| 15 | 73 | `RangeError: Unsupported calendar: indian` |
| 16 | 72 | `TypeError: Intl.DurationFormat is not a constructor` |
| 17 | 61 | `ReferenceError: ShadowRealm is not defined` |
| 18 | 59 | `TypeError: Temporal objects cannot be converted to primitives; use compare() or equals()` |
| 19 | 49 | `RangeError: Unknown time zone: America/Vancouver` |
| 20 | 45 | `TestNError: Expected SameValue(«…», «…») to be true` |
| 21 | 36 | `TypeError: Cannot read properties of undefined (reading 'prototype')` |
| 22 | 32 | `TypeError: Intl.Segmenter is not a constructor` |
| 23 | 24 | `TypeError: Intl.DisplayNames is not a constructor` |
| 24 | 21 | `TypeError: Cannot read properties of undefined (reading 'supportedLocalesOf')` |
| 25 | 13 | `RangeError: Unsupported calendar: islamicc` |
| 26 | 11 | `RangeError: Unknown time zone: America/Los_Angeles` |
| 27 | 10 | `RangeError: Unknown time zone: Africa/Monrovia` |
| 28 | 10 | `TestNError: Expected a TestNError but got a TypeError` |
| 29 | 10 | `TypeError: Cannot convert undefined to object` |
| 30 | 8 | `TestNError: Missing year throws TypeError before month/monthCode conflict throws RangeError Expected a TypeError but got a RangeError` |
