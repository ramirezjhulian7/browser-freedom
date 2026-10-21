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

Mejor de 5 rondas alternadas (base = `master` `2eac3c1`), en ms:

| benchmark | qué mide | master | E3 | speedup |
|---|---|---:|---:|---:|
| `fib` ★ | fib(30): llamadas recursivas, enteros | 2786.8 | 718.9 | 3.88× |
| `props` ★ | bucle de lecturas y escrituras de propiedades | 350.7 | 93.3 | 3.76× |
| `concat` ★ | `s += …`, templates, concatenación en bucle | 2531.7 | 62.9 | 40.25× |
| `arrays` ★ | `push`, suma por índice, `map`/`filter`/`reduce` | 265.3 | 91.7 | 2.89× |
| `closures` ★ | clausuras y variables capturadas | 476.1 | 153.5 | 3.10× |
| `json` ★ | `JSON.stringify` + `JSON.parse` | 142.5 | 41.9 | 3.40× |
| `regexp` ★ | `match`/`exec`/`replace`/`split`/`test` sobre 168 KB | 1422.8 | 506.7 | 2.81× (\*) |
| `dom` ★ | objetos tipo nodo del DOM con 18 propiedades: árbol, layout, recorridos | 3897.0 | 800.4 | 4.87× |
| `alloc` | literales de objetos y arrays, GC | 479.2 | 153.1 | 3.13× |
| `dict` | objetos como diccionario (5000 claves, `delete`) | 122.4 | 31.2 | 3.92× |
| `loops` | bucles con locales y aritmética | 791.6 | 175.9 | 4.50× |
| `methods` | clases, métodos del prototipo, `this` | 524.0 | 144.0 | 3.64× |
| `nbody` | coma flotante y propiedades | 1778.6 | 400.1 | 4.45× |
| `poly` | accesos polimórficos (4 formas) | 728.7 | 199.9 | 3.65× |
| `strings` | `join`, `indexOf`, `slice`, `split` | 68.4 | 20.9 | 3.28× |

★ = los 8 del pedido. **Media geométrica: 4.25× (los 15), 4.72× (los 8 pedidos); 3.47× en
los 8 sin `concat`** (los ropes lo vuelven lineal y dominaría la media).

(\*) `regexp` se midió aparte (3 rondas) después de la ruta rápida de `@@split`, que en ese
programa pasó de 190 ms a 8 ms; lo que queda es `exec` en bucle (~4 µs por match: el objeto
resultado y `lastIndex` van por las operaciones genéricas) y el propio matcher.

Verificación: test262 sin regresiones (52 018 tests, 84.2 %, dos `timeout` pasan a
`passed`); WPT testharness (`dom/events`, `dom/nodes`, `dom/ranges`, `dom/traversal`, `url`)
sin cambios de estado por subtest, con tres subtests nuevos que antes no terminaban
(`NodeList-static-length-getter-tampered-{1,2,3}`); la corrida pasó de 78 s a 60 s.

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

Pausas medidas con `bf-js bench --gc` (una corrida; solo los programas que recolectan):

| benchmark | recolecciones | liberados | pausa total | pausa máxima |
|---|---:|---:|---:|---:|
| `alloc` | 6 | 599 223 | 28.4 ms | 5.3 ms |
| `methods` | 1 | 99 208 | 3.1 ms | 3.1 ms |
| `regexp` | 1 | 99 211 | 32.5 ms | 32.5 ms |
| `dom` | 1 | 0 | 22.8 ms | 22.8 ms |

La pausa crece con lo vivo (marcado completo): `dom` tiene ~100 000 objetos vivos. Un GC
generacional (o incremental) es lo que la bajaría.

## Profundidad

`thousand_levels_parse_and_run` (en `tests/depth.rs`, solo en release) parsea y corre 1000
niveles de paréntesis, arrays, bloques, objetos y funciones anidadas con la configuración del
navegador (`bf-shell`: 64 MB de pila de JS). El parser usa ~3 KB de pila por nivel de
paréntesis y ~10 KB por función anidada; con el límite por defecto de `parse_script`
(512 KB) llega a ~170 paréntesis, y con el del agente (4 MB) a ~1300.

## Decisiones

- **Shapes en `object/property_map.rs`** (no en un `shape.rs` aparte): el shape y el mapa de
  propiedades comparten invariantes (posición del slot, modo diccionario) y quedan juntos.
- **Cambio de atributos sin transición**: los atributos viajan con cada valor, así que
  cambiarlos no cambia la disposición de las claves; las caches de asignación se invalidan
  con una época global (que también avanza con `[[SetPrototypeOf]]`).
- **Sin enteros `i32` etiquetados**: `Value::Number` sigue siendo `f64` (cambiarlo toca todo
  el motor y los host bindings). La aritmética y las comparaciones Number × Number van por la
  ruta rápida en una instrucción de máquina; los enteros tienen rutas propias donde importa
  (ToString, concatenación, `JSON.parse`, índices de Arrays).
- **Sin IC propia para `Call`**: la llamada a un método es `GetPropKeep` (con cache) + `Call`,
  y `Call` a una función ECMAScript ya no sale del bucle del intérprete.

## Pendiente

- Los valores de las propiedades ocupan 40 bytes cada uno (valor + atributos); mover los
  atributos al shape reduciría a la mitad la memoria de objetos grandes como los del
  benchmark `dom`.
- GC generacional.
- Frames del parser más chicos.
