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
