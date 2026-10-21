# Rendimiento de la VM de bf-js

Bloque de rendimiento de E3. La comparación es contra `master` en `2eac3c1` (la VM de antes
del bloque; ese commit no cambia nada de `bf-js` respecto a la integración del core DOM
parte 2), compilado aparte con el mismo arnés de benchmark.

## Cómo medir

```
cargo run --release -p bf-js --bin bf-js -- bench                 # tabla de tiempos
cargo run --release -p bf-js --bin bf-js -- bench --gc            # y estadísticas del GC (stderr)
cargo run --release -p bf-js --bin bf-js -- bench --filter fib    # solo los que contienen "fib"
```

`bf-bench` es el mismo programa (`crates/bf-js/src/bin/bench/mod.rs`). Cada programa de
`crates/bf-js/bench/` termina con `check(valor, esperado)` (valores comprobados con node 22):
una optimización que cambie el resultado hace fallar el benchmark.

La máquina de desarrollo tiene carga variable (varios ejecutores compilando en paralelo), así
que los tiempos sueltos no sirven. `crates/bf-js/bench/ab.sh` alterna el binario de la línea
base y el nuevo `ROUNDS` veces y compara el mejor tiempo de cada uno:

```
git worktree add target/base-src 2eac3c1
mkdir -p target/base-src/crates/bf-js/src/bin/bench
cp crates/bf-js/src/bin/bench/mod.rs target/base-src/crates/bf-js/src/bin/bench/
cp crates/bf-js/src/bin/bf-bench.rs target/base-src/crates/bf-js/src/bin/
cargo build --release -p bf-js --bin bf-bench --manifest-path target/base-src/Cargo.toml --target-dir target/base-target
cargo build --release -p bf-js --bin bf-bench
ROUNDS=5 BASE=target/base-target/release/bf-bench NEW=target/release/bf-bench bash crates/bf-js/bench/ab.sh
```

## Resultados

TABLA

## Qué cambió

### Objetos

- **Shapes** (`object/property_map.rs`): las claves con nombre de un objeto ordinario están
  en un `Shape` compartido (árbol de transiciones por clave agregada, con la primera
  transición sin hash); los valores, en un vector por objeto. Redefinir atributos conserva
  el shape (los atributos van con el valor) y avanza una época global que invalida las
  caches de asignación. Borrar una clave o superar 64 pasa el objeto a **modo diccionario**
  (tabla propia con tombstones y compactación; su `id` cambia en cada cambio de claves).
- **Propiedades inline**: las tres primeras propiedades con nombre viven dentro del objeto,
  sin otra reserva de memoria (el asignador del sistema en Windows cuesta ~150 ns por
  malloc+free).
- **Claves internadas**: los nombres del código (átomos) y de las built-ins comparten el
  mismo `JsString`, y la igualdad compara primero el puntero. Las tablas de los shapes usan
  un hash multiplicativo propio en vez de SipHash.

### Inline caches (`vm/ic.rs`)

Por sitio de bytecode (constante de nombre), polimórficas de hasta 4 vías: propiedad propia,
del prototipo, ausente (y el prototipo sin ella), agregado (transición conocida, válida
mientras la época de setters no cambie), literal de objeto (`InitProp`) y variable global
(slot del objeto global y tamaño del environment declarativo). Se invalidan por shape, por
cambio de prototipo (`[[SetPrototypeOf]]` avanza la época) y por cambios de atributos. Un
primitivo string o number busca en `String.prototype`/`Number.prototype` con la cache. La
llamada a un método (`obj.m()`) es `GetPropKeep` con cache + `Call`, que corre la función
en el mismo bucle.

### Intérprete (`vm/interp.rs`)

- **Ráfagas**: `fast_op` ejecuta sin pasar por `step` las instrucciones que solo tocan el
  frame y el heap (locales, environments, pila, saltos, aritmética y comparaciones
  Number × Number, enteros a string, concatenación, literales, accesos con cache,
  `obj[i]` en Arrays densos, `obj[string]`, `length` de Arrays y strings, `for…of` sobre
  Arrays). Tiene que quedar en línea en el bucle: si LLVM no la inlinea, todo va ~50 % más
  lento (`#[inline(always)]`).
- **Llamadas**: `Call`/`New` a funciones ECMAScript simples empujan el frame y siguen en el
  mismo bucle (sin recursión de Rust); los vectores de pila, locales y argumentos se
  reciclan. El frame bajó de 256 a 232 bytes (lo de `with`/`eval`/código global va en una
  caja aparte); un test lo vigila.
- **Mirilla del compilador**: `Set*` + `Pop` → `Init*`, y `x++` en sentencia → `++x`.
- **`for…of`** sobre un Array cuyo registro de iterador tiene el `next` original de
  `%ArrayIteratorPrototype%`: los pasos de `next` sin crear el objeto resultado.

### Strings (`string.rs`)

Una concatenación de 256 code units o más es un **rope**: guarda las dos partes y se aplana
una sola vez, la primera vez que alguien lee sus code units (sin recursión; la liberación de
un rope profundo tampoco recursa). `s += x` en un bucle deja de copiar el string entero en
cada vuelta. Largo máximo: 2^30 (RangeError "Invalid string length").

### Built-ins

`JSON.parse` construye los valores directamente (sin árbol intermedio) con claves internadas;
`JSON.stringify` escribe en un solo buffer y lee las propiedades de datos sin `[[Get]]`
genérico. `map`/`filter` agregan al vector denso; `push` y los recorridos de Arrays densos no
pasan por `HasProperty`/`Get`.

### GC

Umbral adaptativo (la próxima recolección llega después de asignar tantos objetos como los
vivos, mínimo 100 000). Ahora se miden las pausas: `GcStats` tiene `pause_total` y
`pause_max` (`bf-js bench --gc`).

PAUSAS

## Profundidad

`thousand_levels_parse_and_run` (en `tests/depth.rs`, solo en release) parsea y corre 1000
niveles de paréntesis, arrays, bloques, objetos y funciones anidadas con la configuración del
navegador (`bf-shell`: 64 MB de pila de JS). El parser usa ~3 KB de pila por nivel de
paréntesis y ~10 KB por función anidada; con el límite por defecto de `parse_script`
(512 KB) llega a ~170 paréntesis, y con el del agente (4 MB) a ~1300.

## Pendiente

- Los valores de las propiedades ocupan 40 bytes cada uno (valor + atributos); mover los
  atributos al shape reduciría a la mitad la memoria de objetos grandes como los del
  benchmark `dom`.
- GC generacional.
- Frames del parser más chicos.
