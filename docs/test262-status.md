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

- Commit de test262: `d53ebc8b1931d9f1541e43d59650776e4b766248`; fecha: 2026-10-20; 52017 archivos en 133 s.
- Cada test corre en un realm nuevo con `assert.js`, `sta.js` y sus `includes`; sin flags, dos veces (no estricto y con `"use strict";`). Pasa solo si pasan todas sus ejecuciones.
- `skipped`: módulos (paso 5) y las propuestas de `UNSUPPORTED_FEATURES`. Todo lo demás que falla cuenta como `failed`, incluido lo que todavía no está implementado; `timeout`: presupuesto de instrucciones o 10 s. Las propuestas omitidas:
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
- Detalle por test: `target/test262/results.tsv`.
- `language/` (referencia de regresión de `tests/test262_run.rs`): 96.71%

| Directorio | passed | failed | skipped | timeout | % |
|---|---:|---:|---:|---:|---:|
| annexB/built-ins | 153 | 88 | 0 | 0 | 63.5% |
| annexB/language | 837 | 8 | 0 | 0 | 99.1% |
| built-ins/AbstractModuleSource | 0 | 0 | 8 | 0 | 100.0% |
| built-ins/AggregateError | 25 | 0 | 0 | 0 | 100.0% |
| built-ins/Array | 2872 | 211 | 0 | 0 | 93.2% |
| built-ins/ArrayBuffer | 192 | 0 | 29 | 0 | 100.0% |
| built-ins/ArrayIteratorPrototype | 27 | 0 | 0 | 0 | 100.0% |
| built-ins/AsyncDisposableStack | 0 | 104 | 0 | 0 | 0.0% |
| built-ins/AsyncFromSyncIteratorPrototype | 38 | 0 | 0 | 0 | 100.0% |
| built-ins/AsyncFunction | 18 | 0 | 0 | 0 | 100.0% |
| built-ins/AsyncGeneratorFunction | 23 | 0 | 0 | 0 | 100.0% |
| built-ins/AsyncGeneratorPrototype | 48 | 0 | 0 | 0 | 100.0% |
| built-ins/AsyncIteratorPrototype | 4 | 9 | 0 | 0 | 30.8% |
| built-ins/Atomics | 249 | 131 | 9 | 0 | 65.5% |
| built-ins/BigInt | 0 | 77 | 0 | 0 | 0.0% |
| built-ins/Boolean | 48 | 3 | 0 | 0 | 94.1% |
| built-ins/DataView | 543 | 7 | 11 | 0 | 98.7% |
| built-ins/Date | 0 | 594 | 0 | 0 | 0.0% |
| built-ins/DisposableStack | 0 | 93 | 0 | 0 | 0.0% |
| built-ins/Error | 55 | 3 | 35 | 0 | 94.8% |
| built-ins/FinalizationRegistry | 47 | 0 | 0 | 0 | 100.0% |
| built-ins/Function | 487 | 22 | 0 | 0 | 95.7% |
| built-ins/GeneratorFunction | 23 | 0 | 0 | 0 | 100.0% |
| built-ins/GeneratorPrototype | 61 | 0 | 0 | 0 | 100.0% |
| built-ins/Infinity | 6 | 0 | 0 | 0 | 100.0% |
| built-ins/Iterator | 650 | 4 | 0 | 0 | 99.4% |
| built-ins/JSON | 160 | 6 | 0 | 0 | 96.4% |
| built-ins/Map | 170 | 1 | 33 | 0 | 99.4% |
| built-ins/MapIteratorPrototype | 11 | 0 | 0 | 0 | 100.0% |
| built-ins/Math | 327 | 0 | 0 | 0 | 100.0% |
| built-ins/NaN | 6 | 0 | 0 | 0 | 100.0% |
| built-ins/NativeErrors | 94 | 0 | 0 | 0 | 100.0% |
| built-ins/Number | 337 | 3 | 0 | 0 | 99.1% |
| built-ins/Object | 3218 | 193 | 0 | 0 | 94.3% |
| built-ins/Promise | 643 | 0 | 89 | 0 | 100.0% |
| built-ins/Proxy | 303 | 7 | 1 | 0 | 97.7% |
| built-ins/Reflect | 151 | 2 | 0 | 0 | 98.7% |
| built-ins/RegExp | 192 | 1687 | 0 | 0 | 10.2% |
| built-ins/RegExpStringIteratorPrototype | 0 | 17 | 0 | 0 | 0.0% |
| built-ins/Set | 382 | 1 | 0 | 0 | 99.7% |
| built-ins/SetIteratorPrototype | 11 | 0 | 0 | 0 | 100.0% |
| built-ins/ShadowRealm | 0 | 60 | 4 | 0 | 0.0% |
| built-ins/SharedArrayBuffer | 104 | 0 | 0 | 0 | 100.0% |
| built-ins/String | 1041 | 182 | 0 | 0 | 85.1% |
| built-ins/StringIteratorPrototype | 7 | 0 | 0 | 0 | 100.0% |
| built-ins/SuppressedError | 0 | 22 | 0 | 0 | 0.0% |
| built-ins/Symbol | 95 | 3 | 0 | 0 | 96.9% |
| built-ins/Temporal | 0 | 4605 | 0 | 0 | 0.0% |
| built-ins/ThrowTypeError | 14 | 0 | 0 | 0 | 100.0% |
| built-ins/TypedArray | 1327 | 118 | 8 | 0 | 91.8% |
| built-ins/TypedArrayConstructors | 729 | 7 | 2 | 0 | 99.0% |
| built-ins/Uint8Array | 8 | 60 | 2 | 0 | 11.8% |
| built-ins/WeakMap | 102 | 0 | 39 | 0 | 100.0% |
| built-ins/WeakRef | 29 | 0 | 0 | 0 | 100.0% |
| built-ins/WeakSet | 85 | 0 | 0 | 0 | 100.0% |
| built-ins/decodeURI | 54 | 0 | 0 | 1 | 98.2% |
| built-ins/decodeURIComponent | 55 | 0 | 0 | 1 | 98.2% |
| built-ins/encodeURI | 31 | 0 | 0 | 0 | 100.0% |
| built-ins/encodeURIComponent | 31 | 0 | 0 | 0 | 100.0% |
| built-ins/eval | 10 | 0 | 0 | 0 | 100.0% |
| built-ins/global | 25 | 4 | 0 | 0 | 86.2% |
| built-ins/isFinite | 15 | 0 | 0 | 0 | 100.0% |
| built-ins/isNaN | 15 | 0 | 0 | 0 | 100.0% |
| built-ins/parseFloat | 54 | 0 | 0 | 0 | 100.0% |
| built-ins/parseInt | 55 | 0 | 0 | 0 | 100.0% |
| built-ins/undefined | 8 | 0 | 0 | 0 | 100.0% |
| intl402 | 0 | 22 | 0 | 0 | 0.0% |
| intl402/Array | 1 | 1 | 0 | 0 | 50.0% |
| intl402/BigInt | 0 | 11 | 0 | 0 | 0.0% |
| intl402/Collator | 0 | 65 | 0 | 0 | 0.0% |
| intl402/Date | 0 | 12 | 0 | 0 | 0.0% |
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
| intl402/TypedArray | 1 | 0 | 0 | 0 | 100.0% |
| language/arguments-object | 261 | 2 | 0 | 0 | 99.2% |
| language/asi | 102 | 0 | 0 | 0 | 100.0% |
| language/block-scope | 145 | 0 | 0 | 0 | 100.0% |
| language/comments | 51 | 0 | 1 | 0 | 100.0% |
| language/computed-property-names | 48 | 0 | 0 | 0 | 100.0% |
| language/destructuring | 17 | 2 | 0 | 0 | 89.5% |
| language/directive-prologue | 62 | 0 | 0 | 0 | 100.0% |
| language/eval-code | 343 | 0 | 4 | 0 | 100.0% |
| language/export | 0 | 0 | 3 | 0 | 100.0% |
| language/expressions | 10115 | 529 | 458 | 0 | 95.0% |
| language/function-code | 217 | 0 | 0 | 0 | 100.0% |
| language/future-reserved-words | 55 | 0 | 0 | 0 | 100.0% |
| language/global-code | 42 | 0 | 0 | 0 | 100.0% |
| language/identifier-resolution | 14 | 0 | 0 | 0 | 100.0% |
| language/identifiers | 268 | 0 | 0 | 0 | 100.0% |
| language/import | 0 | 0 | 129 | 0 | 100.0% |
| language/keywords | 25 | 0 | 0 | 0 | 100.0% |
| language/line-terminators | 41 | 0 | 0 | 0 | 100.0% |
| language/literals | 501 | 35 | 0 | 0 | 93.5% |
| language/module-code | 1 | 1 | 597 | 0 | 50.0% |
| language/punctuators | 11 | 0 | 0 | 0 | 100.0% |
| language/reserved-words | 26 | 0 | 1 | 0 | 100.0% |
| language/rest-parameters | 11 | 0 | 0 | 0 | 100.0% |
| language/source-text | 1 | 0 | 0 | 0 | 100.0% |
| language/statementList | 64 | 16 | 0 | 0 | 80.0% |
| language/statements | 9181 | 130 | 36 | 0 | 98.6% |
| language/types | 113 | 0 | 0 | 0 | 100.0% |
| language/white-space | 42 | 25 | 0 | 0 | 62.7% |
| **language/ (total)** | 21757 | 740 | 1229 | 0 | 96.7% |
| **Total** | 38074 | 12442 | 1499 | 2 | 75.4% |

### Los 30 errores más frecuentes

| # | tests | error |
|---:|---:|---|
| 1 | 3378 | `ReferenceError: Temporal is not defined` |
| 2 | 2809 | `harness temporalHelpers.js: Error: no implementado: RegExp (built-in del paso N)` |
| 3 | 1485 | `InternalError: no implementado: RegExp (built-in del paso N)` |
| 4 | 981 | `ReferenceError: Intl is not defined` |
| 5 | 796 | `ReferenceError: Date is not defined` |
| 6 | 430 | `ReferenceError: RegExp is not defined` |
| 7 | 389 | `InternalError: no implementado: import() (módulos, paso N)` |
| 8 | 255 | `ReferenceError: BigInt is not defined` |
| 9 | 185 | `harness testIntl.js: Error: no implementado: RegExp (built-in del paso N)` |
| 10 | 133 | `InternalError: no implementado: aritmética de BigInt (paso N)` |
| 11 | 121 | `TestNError: Calling as constructor Expected a TypeError but got a ReferenceError` |
| 12 | 112 | `harness atomicsHelper.js: TypeError: Cannot read properties of undefined (reading 'bind')` |
| 13 | 99 | `InternalError: no implementado: using (explicit-resource-management)` |
| 14 | 89 | `ReferenceError: DisposableStack is not defined` |
| 15 | 72 | `ReferenceError: AsyncDisposableStack is not defined` |
| 16 | 70 | `TestNError: The result of evaluating (e instanceof SyntaxError) is expected to be true Expected SameValue(«…», «…») to be true` |
| 17 | 57 | `ReferenceError: ShadowRealm is not defined` |
| 18 | 56 | `TestN:AsyncTestFailure:TypeError: Array.fromAsync is not a function` |
| 19 | 49 | `TestNError: Expected a TestNError but got a ReferenceError` |
| 20 | 47 | `TestNError: Expected a TypeError but got a ReferenceError` |
| 21 | 44 | `InternalError: no implementado: RegExp (built-in de EN)` |
| 22 | 38 | `TestNError: Expected a RangeError but got a ReferenceError` |
| 23 | 34 | `RangeError: Maximum call stack size exceeded` |
| 24 | 28 | `TestN:AsyncTestFailure:ReferenceError: AsyncDisposableStack is not defined` |
| 25 | 19 | `ReferenceError: SuppressedError is not defined` |
| 26 | 18 | `TestNError: reject minus zero as extended year Expected a RangeError but got a ReferenceError` |
| 27 | 16 | `TypeError: Cannot convert undefined to object` |
| 28 | 15 | `TestNError: Expected SameValue(«…», «…») to be true` |
| 29 | 15 | `TestNError: empty string is not a valid calendar ID Expected a RangeError but got a ReferenceError` |
| 30 | 15 | `TypeError: Cannot mix BigInt and other types, use explicit conversions` |
