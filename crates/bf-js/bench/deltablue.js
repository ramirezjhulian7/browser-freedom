// DeltaBlue: resolvedor incremental de jerarquías de restricciones.
//
// Implementación propia de bf-js (misma licencia que el repositorio), escrita a partir del
// algoritmo publicado: B. Freeman-Benson, J. Maloney y A. Borning, "An Incremental Constraint
// Solver", Communications of the ACM 33(1), 1990. No copia el texto de `deltablue.js` de
// Octane (GPL v2), pero reproduce su carga de trabajo para que los tiempos sean comparables:
// - cada vuelta es `chainTest(100)` + `projectionTest(100)`, con las mismas comprobaciones;
// - las listas de trabajo se vacían desde el final (el `removeFirst` de Octane es un `pop`);
// - la dirección de una restricción binaria la decide solo la comparación de `walkStrength`
//   de sus dos variables (en Octane los casos por marca quedan sobrescritos).

// Fuerzas: un número menor es más fuerte.
const REQUIRED = 0;
const STRONG_PREFERRED = 1;
const PREFERRED = 2;
const STRONG_DEFAULT = 3;
const NORMAL = 4;
const WEAK_DEFAULT = 5;
const WEAKEST = 6;

const stronger = (a, b) => a < b;
const weaker = (a, b) => a > b;
const weakestOf = (a, b) => (a > b ? a : b);

const NONE = 0;
const FORWARD = 1;
const BACKWARD = 2;

let planner = null;

class Var {
  constructor(name, value) {
    this.name = name;
    this.value = value === undefined ? 0 : value;
    this.constraints = [];
    this.determinedBy = null;
    this.mark = 0;
    this.walkStrength = WEAKEST;
    this.stay = true;
  }
  attach(c) {
    this.constraints.push(c);
  }
  detach(c) {
    this.constraints = this.constraints.filter((x) => x !== c);
    if (this.determinedBy === c) this.determinedBy = null;
  }
}

class Constraint {
  constructor(strength) {
    this.strength = strength;
  }
  // Lo añade al grafo y lo satisface si puede (incrementalAdd).
  install() {
    this.addToGraph();
    planner.add(this);
  }
  // Elige cómo satisfacerse con la marca dada; devuelve la restricción que desplazó.
  satisfy(mark) {
    this.chooseMethod(mark);
    if (!this.isSatisfied()) {
      if (this.strength === REQUIRED) throw new Error("required constraint not satisfied");
      return null;
    }
    this.markInputs(mark);
    const out = this.output();
    const displaced = out.determinedBy;
    if (displaced !== null) displaced.markUnsatisfied();
    out.determinedBy = this;
    if (!planner.propagateFrom(this, mark)) throw new Error("cycle");
    out.mark = mark;
    return displaced;
  }
  destroy() {
    if (this.isSatisfied()) planner.remove(this);
    else this.removeFromGraph();
  }
  isInput() {
    return false;
  }
}

class Unary extends Constraint {
  constructor(v, strength) {
    super(strength);
    this.v = v;
    this.satisfied = false;
  }
  addToGraph() {
    this.v.attach(this);
    this.satisfied = false;
  }
  removeFromGraph() {
    this.v.detach(this);
    this.satisfied = false;
  }
  chooseMethod(mark) {
    this.satisfied = this.v.mark !== mark && stronger(this.strength, this.v.walkStrength);
  }
  isSatisfied() {
    return this.satisfied;
  }
  markUnsatisfied() {
    this.satisfied = false;
  }
  markInputs(mark) {}
  inputsKnown(mark) {
    return true;
  }
  output() {
    return this.v;
  }
  recalculate() {
    this.v.walkStrength = this.strength;
    this.v.stay = !this.isInput();
    if (this.v.stay) this.execute();
  }
  execute() {}
}

// Mantiene el valor de su variable.
class Stay extends Unary {
  constructor(v, strength) {
    super(v, strength);
    this.install();
  }
}

// Marca una variable como entrada que cambia el usuario.
class Edit extends Unary {
  constructor(v, strength) {
    super(v, strength);
    this.install();
  }
  isInput() {
    return true;
  }
}

class Binary extends Constraint {
  constructor(a, b, strength) {
    super(strength);
    this.a = a;
    this.b = b;
    this.dir = NONE;
  }
  addToGraph() {
    this.a.attach(this);
    this.b.attach(this);
    this.dir = NONE;
  }
  removeFromGraph() {
    this.a.detach(this);
    this.b.detach(this);
    this.dir = NONE;
  }
  chooseMethod(mark) {
    const sa = this.a.walkStrength;
    const sb = this.b.walkStrength;
    if (weaker(sa, sb)) this.dir = stronger(this.strength, sa) ? BACKWARD : NONE;
    else this.dir = stronger(this.strength, sb) ? FORWARD : BACKWARD;
  }
  isSatisfied() {
    return this.dir !== NONE;
  }
  markUnsatisfied() {
    this.dir = NONE;
  }
  input() {
    return this.dir === FORWARD ? this.a : this.b;
  }
  output() {
    return this.dir === FORWARD ? this.b : this.a;
  }
  markInputs(mark) {
    this.input().mark = mark;
  }
  inputsKnown(mark) {
    const i = this.input();
    return i.mark === mark || i.stay || i.determinedBy === null;
  }
  recalculate() {
    const i = this.input();
    const o = this.output();
    o.walkStrength = weakestOf(this.strength, i.walkStrength);
    o.stay = i.stay;
    if (o.stay) this.execute();
  }
}

// a = b.
class Equal extends Binary {
  constructor(a, b, strength) {
    super(a, b, strength);
    this.install();
  }
  execute() {
    this.output().value = this.input().value;
  }
}

// dst = src * scale + offset.
class Scale extends Binary {
  constructor(src, scale, offset, dst, strength) {
    super(src, dst, strength);
    this.scale = scale;
    this.offset = offset;
    this.install();
  }
  addToGraph() {
    super.addToGraph();
    this.scale.attach(this);
    this.offset.attach(this);
  }
  removeFromGraph() {
    super.removeFromGraph();
    this.scale.detach(this);
    this.offset.detach(this);
  }
  markInputs(mark) {
    super.markInputs(mark);
    this.scale.mark = mark;
    this.offset.mark = mark;
  }
  execute() {
    if (this.dir === FORWARD) {
      this.b.value = this.a.value * this.scale.value + this.offset.value;
    } else {
      this.a.value = (this.b.value - this.offset.value) / this.scale.value;
    }
  }
  recalculate() {
    const i = this.input();
    const o = this.output();
    o.walkStrength = weakestOf(this.strength, i.walkStrength);
    o.stay = i.stay && this.scale.stay && this.offset.stay;
    if (o.stay) this.execute();
  }
}

class Planner {
  constructor() {
    this.mark = 0;
  }
  nextMark() {
    return ++this.mark;
  }
  add(c) {
    const mark = this.nextMark();
    let displaced = c.satisfy(mark);
    while (displaced !== null) displaced = displaced.satisfy(mark);
  }
  // Quita una restricción satisfecha y vuelve a intentar las que quedaron sin satisfacer
  // aguas abajo, de la más fuerte a la más débil (las WEAKEST no se reintentan).
  remove(c) {
    const out = c.output();
    c.markUnsatisfied();
    c.removeFromGraph();
    const pending = this.unpropagate(out);
    for (let s = REQUIRED; s !== WEAKEST; s++) {
      for (const u of pending) if (u.strength === s) this.add(u);
    }
  }
  // Recalcula aguas abajo de `c`; false si vuelve a la variable marcada (un ciclo).
  propagateFrom(c, mark) {
    const todo = [c];
    while (todo.length > 0) {
      const d = todo.pop();
      if (d.output().mark === mark) {
        this.remove(c);
        return false;
      }
      d.recalculate();
      this.consumersOf(d.output(), todo);
    }
    return true;
  }
  unpropagate(out) {
    out.determinedBy = null;
    out.walkStrength = WEAKEST;
    out.stay = true;
    const pending = [];
    const todo = [out];
    while (todo.length > 0) {
      const v = todo.pop();
      for (const c of v.constraints) if (!c.isSatisfied()) pending.push(c);
      const by = v.determinedBy;
      for (const c of v.constraints) {
        if (c !== by && c.isSatisfied()) {
          c.recalculate();
          todo.push(c.output());
        }
      }
    }
    return pending;
  }
  consumersOf(v, into) {
    const by = v.determinedBy;
    for (const c of v.constraints) if (c !== by && c.isSatisfied()) into.push(c);
  }
  // El plan: las restricciones en orden topológico desde las entradas.
  planFor(constraints) {
    const sources = constraints.filter((c) => c.isInput() && c.isSatisfied());
    const mark = this.nextMark();
    const plan = [];
    const todo = sources.slice();
    while (todo.length > 0) {
      const c = todo.pop();
      const o = c.output();
      if (o.mark !== mark && c.inputsKnown(mark)) {
        plan.push(c);
        o.mark = mark;
        this.consumersOf(o, todo);
      }
    }
    return plan;
  }
}

function run(plan) {
  for (const c of plan) c.execute();
}

let failures = 0;

// Cadena de n igualdades con un Stay en un extremo y un Edit en el otro.
function chainTest(n) {
  planner = new Planner();
  let prev = null;
  let first = null;
  let last = null;
  for (let i = 0; i <= n; i++) {
    const v = new Var("v" + i);
    if (prev !== null) new Equal(prev, v, REQUIRED);
    if (i === 0) first = v;
    if (i === n) last = v;
    prev = v;
  }
  new Stay(last, STRONG_DEFAULT);
  const plan = planner.planFor([new Edit(first, PREFERRED)]);
  for (let i = 0; i < 100; i++) {
    first.value = i;
    run(plan);
    if (last.value !== i) failures++;
  }
}

// n pares dst = src * scale + offset; cambia cada lado y los dos factores.
function projectionTest(n) {
  planner = new Planner();
  const scale = new Var("scale", 10);
  const offset = new Var("offset", 1000);
  let src = null;
  let dst = null;
  const dests = [];
  for (let i = 0; i < n; i++) {
    src = new Var("src" + i, i);
    dst = new Var("dst" + i, i);
    dests.push(dst);
    new Stay(src, NORMAL);
    new Scale(src, scale, offset, dst, REQUIRED);
  }
  change(src, 17);
  if (dst.value !== 1170) failures++;
  change(dst, 1050);
  if (src.value !== 5) failures++;
  change(scale, 5);
  for (let i = 0; i < n - 1; i++) if (dests[i].value !== i * 5 + 1000) failures++;
  change(offset, 2000);
  for (let i = 0; i < n - 1; i++) if (dests[i].value !== i * 5 + 2000) failures++;
}

function change(v, value) {
  const edit = new Edit(v, PREFERRED);
  const plan = planner.planFor([edit]);
  for (let i = 0; i < 10; i++) {
    v.value = value;
    run(plan);
  }
  edit.destroy();
}

// bf-bench: RUNS vueltas de la carga de Octane (que hace 4400 en modo determinista).
const RUNS = 10;
for (let r = 0; r < RUNS; r++) {
  chainTest(100);
  projectionTest(100);
}
check(failures, 0);
