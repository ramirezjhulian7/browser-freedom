# Benchmark de bf-js

Programas que ejercitan las rutas calientes de la VM. Cada uno termina con
`check(valor, esperado)`: si el resultado no es el esperado, el benchmark falla (una
optimización incorrecta no pasa desapercibida).

```
cargo run --release -p bf-js --bin bf-bench                       # mide y muestra la tabla
cargo run --release -p bf-js --bin bf-bench -- --save base.tsv    # guarda la línea base
cargo run --release -p bf-js --bin bf-bench -- --compare base.tsv # speedup y media geométrica
cargo run --release -p bf-js --bin bf-bench -- --filter fib       # solo los que contienen "fib"
```

| Archivo | Qué mide |
|---|---|
| `fib.js` | llamadas recursivas, aritmética de enteros |
| `loops.js` | bucles con locales, comparaciones y sumas |
| `props.js` | lectura y escritura de propiedades con la misma forma |
| `methods.js` | clases, métodos del prototipo, `this` |
| `closures.js` | clausuras y variables capturadas |
| `alloc.js` | creación de objetos literales y GC |
| `arrays.js` | `push`, índices, `map`/`filter`/`reduce` |
| `strings.js` | concatenación, `indexOf`, `slice`, templates, `join` |
| `nbody.js` | simulación con coma flotante y propiedades |
| `json.js` | `JSON.stringify` y `JSON.parse` |
| `poly.js` | accesos polimórficos (objetos de formas distintas) |
| `dict.js` | objetos usados como diccionario (muchas claves) |
