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

- Commit de test262: `d53ebc8b1931d9f1541e43d59650776e4b766248`; fecha: 2026-11-12; 52017 archivos en 504 s.
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
| built-ins/ShadowRealm | 64 | 0 | 0 | 0 | 100.0% |
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
| intl402 | 22 | 0 | 0 | 0 | 100.0% |
| intl402/Array | 2 | 0 | 0 | 0 | 100.0% |
| intl402/BigInt | 11 | 0 | 0 | 0 | 100.0% |
| intl402/Collator | 62 | 3 | 0 | 0 | 95.4% |
| intl402/Date | 12 | 0 | 0 | 0 | 100.0% |
| intl402/DateTimeFormat | 212 | 33 | 0 | 0 | 86.5% |
| intl402/DisplayNames | 57 | 0 | 0 | 0 | 100.0% |
| intl402/DurationFormat | 109 | 1 | 0 | 0 | 99.1% |
| intl402/FallbackSymbol | 2 | 0 | 0 | 0 | 100.0% |
| intl402/Intl | 63 | 3 | 0 | 0 | 95.5% |
| intl402/ListFormat | 81 | 0 | 0 | 0 | 100.0% |
| intl402/Locale | 189 | 1 | 0 | 0 | 99.5% |
| intl402/Number | 7 | 0 | 0 | 0 | 100.0% |
| intl402/NumberFormat | 221 | 30 | 0 | 0 | 88.0% |
| intl402/PluralRules | 51 | 2 | 0 | 0 | 96.2% |
| intl402/RelativeTimeFormat | 80 | 0 | 0 | 0 | 100.0% |
| intl402/Segmenter | 79 | 0 | 0 | 0 | 100.0% |
| intl402/String | 15 | 4 | 0 | 0 | 78.9% |
| intl402/Temporal | 1497 | 532 | 0 | 0 | 73.8% |
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
| **Total** | 50594 | 625 | 797 | 1 | 98.8% |

### Los 30 errores más frecuentes

| # | tests | error |
|---:|---:|---|
| 1 | 163 | `RangeError: Unsupported calendar: chinese` |
| 2 | 120 | `RangeError: Unsupported calendar: dangi` |
| 3 | 96 | `RangeError: Unsupported calendar: islamic-umalqura` |
| 4 | 49 | `RangeError: Unknown time zone: America/Vancouver` |
| 5 | 19 | `TestNError: Expected SameValue(«…», «…») to be true` |
| 6 | 11 | `RangeError: Unknown time zone: America/Los_Angeles` |
| 7 | 10 | `RangeError: Unknown time zone: Africa/Monrovia` |
| 8 | 6 | `RangeError: Unknown time zone: Africa/Abidjan` |
| 9 | 6 | `RangeError: Unknown time zone: America/Toronto` |
| 10 | 6 | `TestNError: Code unit: dN Expected SameValue(«…», «…») to be true` |
| 11 | 5 | `RangeError: Unknown time zone: Asia/Calcutta` |
| 12 | 5 | `RangeError: the calendar islamic-tbla does not match the calendar of the format (gregory)` |
| 13 | 5 | `TestNError: Missing year throws TypeError before month/monthCode conflict throws RangeError Expected a TypeError but got a RangeError` |
| 14 | 4 | `RangeError: Maximum call stack size exceeded` |
| 15 | 4 | `RangeError: Unknown time zone: America/Bogota` |
| 16 | 4 | `TestNError: Actual [noon] and expected [] should have the same contents. unused dayPeriods: noon` |
| 17 | 4 | `TestNError: negative: parts[N].value Expected SameValue(«…», «…») to be true` |
| 18 | 3 | `RangeError: Unknown time zone: America/Sao_Paulo` |
| 19 | 3 | `RangeError: Unknown time zone: Etc/GMT+N` |
| 20 | 3 | `RangeError: Unknown time zone: Europe/Vienna` |
| 21 | 3 | `RangeError: Unknown time zone: Pacific/Apia` |
| 22 | 3 | `TestNError: Compact short: N: length Expected SameValue(«…», «…») to be true` |
| 23 | 3 | `TestNError: calendar mismatch Expected a RangeError to be thrown but no exception was thrown at all` |
| 24 | 2 | `RangeError: Unknown time zone: Africa/Asmera` |
| 25 | 2 | `RangeError: Unknown time zone: America/Kentucky/Louisville` |
| 26 | 2 | `RangeError: Unknown time zone: America/New_York` |
| 27 | 2 | `RangeError: Unknown time zone: America/St_Johns` |
| 28 | 2 | `RangeError: Unknown time zone: Asia/Kolkata` |
| 29 | 2 | `RangeError: Unknown time zone: Asia/Tokyo` |
| 30 | 2 | `RangeError: Unknown time zone: Etc/GMT-N` |

## macOS frente a Windows

La sección "Ejecución" de arriba es la primera corrida completa en macOS (2026-11-12, Apple M4
de 10 núcleos, Rust 1.100.0, `master` `5b9a230`). Comando:
`cargo run --release -p bf-js --bin test262 -- tests/test262 --threads 4 --out docs/test262-status.md`.
El runner reescribe solo la sección "Ejecución"; esta se queda.

| corrida | archivos | passed | failed | skipped | timeout | % | tiempo |
|---|---:|---:|---:|---:|---:|---:|---:|
| Windows, 2026-11-07 (E8, `488bf24`) | 52 017 | 48 791 | 2 429 | 797 | 0 | 95.3 % | 2 872 s |
| macOS, 2026-11-12 (`5b9a230`) | 52 017 | **50 594** | 625 | 797 | 1 | **98.8 %** | 504 s |

- Mismo commit de test262 (`d53ebc8`). `language/` igual: 99.95 % (23 183 passed, 11 failed).
- **0 pánicos** (el runner los atrapa y los anota como `pánico: …`; ninguno en
  `target/test262/results.tsv`).
- **1 timeout**, por carga: `built-ins/decodeURIComponent/S15.1.3.2_A2.5_T1.js` (recorre
  rangos grandes de code points). Corrido solo pasa en 9 s de reloj (6.2 s de CPU) con el
  presupuesto de 10 s; durante la corrida completa la carga de la máquina estaba en 95–120
  (otros cinco ejecutores compilando). En Windows pasó.
- 504 s con `--threads 4` y la máquina cargada (791 s de CPU de usuario, 163 % de CPU),
  frente a 2 872 s en Windows.
- Directorios que cambiaron (todos por lo que E8 añadió después de la corrida de Windows: los
  calendarios de Temporal, Temporal en DateTimeFormat, ShadowRealm, Segmenter, DurationFormat,
  DisplayNames y datos CLDR de `de`/`pl`); ninguna regresión fuera del timeout de arriba:

| directorio | passed Windows | passed macOS | failed Windows | failed macOS |
|---|---:|---:|---:|---:|
| built-ins/ShadowRealm | 0 | 64 | 64 | 0 |
| intl402/BigInt | 10 | 11 | 1 | 0 |
| intl402/DateTimeFormat | 159 | 212 | 86 | 33 |
| intl402/DisplayNames | 4 | 57 | 53 | 0 |
| intl402/DurationFormat | 0 | 109 | 110 | 1 |
| intl402/Intl | 61 | 63 | 5 | 3 |
| intl402/ListFormat | 80 | 81 | 1 | 0 |
| intl402/NumberFormat | 209 | 221 | 42 | 30 |
| intl402/RelativeTimeFormat | 73 | 80 | 7 | 0 |
| intl402/Segmenter | 2 | 79 | 77 | 0 |
| intl402/Temporal | 72 | 1 497 | 1 957 | 532 |
| built-ins/decodeURIComponent | 56 | 55 | 0 | 0 (1 timeout) |

- Después de la corrida completa, los rangos de fecha y hora del mismo día (`formatRange` ya no
  repite la fecha) suben `intl402/DateTimeFormat` de 212 a 214 (los dos
  `temporal-objects-resolved-time-zone.js`), medido con `--filter intl402/` y comparado test
  por test, sin regresiones; la tabla de arriba es de antes de ese cambio.
- Lo que queda (625): sobre todo `intl402/Temporal` (532: calendarios `chinese`, `dangi` e
  `islamic-umalqura`, que necesitan tablas astronómicas, y zonas IANA, que no hay en el árbol),
  `intl402/DateTimeFormat` (33) e `intl402/NumberFormat` (30). Ver "Los 30 errores más
  frecuentes" arriba.
