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

## Comparación A/B

En una máquina con carga, los tiempos sueltos varían mucho. `ab.sh` alterna el binario de la
línea base y el actual `ROUNDS` veces y compara el mejor tiempo de cada uno:

```
# la línea base, compilada aparte desde el commit de referencia
git worktree add ../bf-base c1e7872
cargo build --release -p bf-js --bin bf-bench --manifest-path ../bf-base/Cargo.toml --target-dir target/base-target
cargo build --release -p bf-js --bin bf-bench
ROUNDS=5 bash crates/bf-js/bench/ab.sh
```

## Resultados del bloque de rendimiento de la VM

Línea base: `c1e7872` (el intérprete antes del bloque). Mejor de 5 rondas alternadas.

| benchmark | base ms | nuevo ms | speedup |
|---|---:|---:|---:|
| alloc | 480.2 | 152.0 | 3.16× |
| arrays | 264.4 | 86.0 | 3.08× |
| closures | 475.2 | 140.2 | 3.39× |
| dict | 118.5 | 28.8 | 4.12× |
| fib | 225.2 | 61.2 | 3.68× |
| json | 131.8 | 38.2 | 3.45× |
| loops | 761.1 | 163.1 | 4.67× |
| methods | 431.7 | 138.2 | 3.12× |
| nbody | 1756.2 | 373.8 | 4.70× |
| poly | 713.0 | 180.2 | 3.96× |
| props | 356.2 | 83.4 | 4.27× |
| strings | 74.2 | 25.9 | 2.86× |

Media geométrica del speedup: **3.66×**.

Qué cambió: shapes compartidos con transiciones y modo diccionario; inline caches por sitio
(propias, del prototipo, ausentes, agregado, literales y globales; polimórficas de 4 vías);
una ruta rápida que ejecuta en ráfaga las instrucciones que solo tocan el frame y el heap;
llamadas y `new` a funciones ECMAScript en el mismo bucle (sin recursión de Rust) con frames
y argumentos reciclados; propiedades inline (3 por objeto); mirilla `Set*`+`Pop` → `Init*` y
`x++` descartado → `++x`; enteros y concatenación sin pasar por el agente; `JSON.parse`
directo y `JSON.stringify` en un solo buffer.
