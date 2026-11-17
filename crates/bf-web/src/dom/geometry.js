// E1: Geometry Interfaces Module Level 1 (W3C): DOMPointReadOnly, DOMPoint, DOMQuad,
// DOMMatrixReadOnly y DOMMatrix (§2, §4, §6). DOMRectReadOnly y DOMRect son clases del host
// (`cssom::view`). `[Exposed=(Window,Worker)]`; en una ventana, además el alias
// `WebKitCSSMatrix` (§6, LegacyWindowAlias) y el constructor con una cadena (§6.1: solo en
// Window, parsea una <transform-list> de CSS).
//
// La matriz se guarda en el orden de §6 (m11, m12, m13, m14, m21, …, m44): por columnas, con
// el elemento de la fila r y la columna c en el índice c * 4 + r.
(function (global, isWindow) {
  'use strict';
  const define = (name, value) =>
    Object.defineProperty(global, name, { value, writable: true, configurable: true, enumerable: false });
  const num = (v) => Number(v);
  const DOMException = global.DOMException;
  const syntaxError = (m) => new DOMException(m, 'SyntaxError');

  // --- DOMPoint (§2) ---

  class DOMPointReadOnly {
    #x; #y; #z; #w;
    constructor(x = 0, y = 0, z = 0, w = 1) {
      this.#x = num(x); this.#y = num(y); this.#z = num(z); this.#w = num(w);
    }
    static fromPoint(other) {
      const p = pointInit(other);
      return new this(p.x, p.y, p.z, p.w);
    }
    get x() { return this.#x; }
    get y() { return this.#y; }
    get z() { return this.#z; }
    get w() { return this.#w; }
    // Para DOMPoint (los setters no pueden ver el #privado de otra clase).
    static _set(p, k, v) {
      if (k === 'x') p.#x = num(v); else if (k === 'y') p.#y = num(v);
      else if (k === 'z') p.#z = num(v); else p.#w = num(v);
    }
    matrixTransform(matrix) {
      const m = DOMMatrixReadOnly.fromMatrix(matrix);
      return m.transformPoint(this);
    }
    toJSON() { return { x: this.x, y: this.y, z: this.z, w: this.w }; }
  }
  class DOMPoint extends DOMPointReadOnly {
    static fromPoint(other) {
      const p = pointInit(other);
      return new DOMPoint(p.x, p.y, p.z, p.w);
    }
    get x() { return super.x; } set x(v) { DOMPointReadOnly._set(this, 'x', v); }
    get y() { return super.y; } set y(v) { DOMPointReadOnly._set(this, 'y', v); }
    get z() { return super.z; } set z(v) { DOMPointReadOnly._set(this, 'z', v); }
    get w() { return super.w; } set w(v) { DOMPointReadOnly._set(this, 'w', v); }
  }
  const setPointHelper = DOMPointReadOnly._set;
  delete DOMPointReadOnly._set;
  // Los setters usan el ayudante guardado.
  for (const k of ['x', 'y', 'z', 'w']) {
    const get = Object.getOwnPropertyDescriptor(DOMPointReadOnly.prototype, k).get;
    Object.defineProperty(DOMPoint.prototype, k, {
      get() { return get.call(this); },
      set(v) { setPointHelper(this, k, v); },
      enumerable: true, configurable: true,
    });
  }

  function pointInit(d) {
    if (d === undefined || d === null) return { x: 0, y: 0, z: 0, w: 1 };
    if (typeof d !== 'object' && typeof d !== 'function') throw new TypeError('DOMPointInit must be an object');
    const g = (k, def) => (d[k] === undefined ? def : num(d[k]));
    return { x: g('x', 0), y: g('y', 0), z: g('z', 0), w: g('w', 1) };
  }

  // --- DOMQuad (§4) ---

  class DOMQuad {
    #p;
    constructor(p1 = undefined, p2 = undefined, p3 = undefined, p4 = undefined) {
      this.#p = [p1, p2, p3, p4].map((p) => DOMPoint.fromPoint(p));
    }
    static fromRect(other) {
      const r = rectInit(other);
      return new DOMQuad({ x: r.x, y: r.y }, { x: r.x + r.width, y: r.y },
        { x: r.x + r.width, y: r.y + r.height }, { x: r.x, y: r.y + r.height });
    }
    static fromQuad(other) {
      const q = other === undefined || other === null ? {} : other;
      return new DOMQuad(q.p1, q.p2, q.p3, q.p4);
    }
    get p1() { return this.#p[0]; }
    get p2() { return this.#p[1]; }
    get p3() { return this.#p[2]; }
    get p4() { return this.#p[3]; }
    getBounds() {
      const xs = this.#p.map((p) => p.x), ys = this.#p.map((p) => p.y);
      const left = nanSafeMin(xs), top = nanSafeMin(ys);
      return new global.DOMRect(left, top, nanSafeMax(xs) - left, nanSafeMax(ys) - top);
    }
    toJSON() { return { p1: this.p1, p2: this.p2, p3: this.p3, p4: this.p4 }; }
  }
  // §4: el mínimo y el máximo "of the values … ; if any is NaN, NaN".
  const nanSafeMin = (v) => (v.some(Number.isNaN) ? NaN : Math.min(...v));
  const nanSafeMax = (v) => (v.some(Number.isNaN) ? NaN : Math.max(...v));

  function rectInit(d) {
    if (d === undefined || d === null) return { x: 0, y: 0, width: 0, height: 0 };
    const g = (k) => (d[k] === undefined ? 0 : num(d[k]));
    return { x: g('x'), y: g('y'), width: g('width'), height: g('height') };
  }

  // --- DOMMatrix (§6) ---

  const IDENTITY = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
  const NAMES = ['m11', 'm12', 'm13', 'm14', 'm21', 'm22', 'm23', 'm24',
    'm31', 'm32', 'm33', 'm34', 'm41', 'm42', 'm43', 'm44'];
  const ALIASES = { a: 0, b: 1, c: 4, d: 5, e: 12, f: 13 };
  // Los elementos que tienen que valer 0 (o 1 en m33 y m44) en una matriz 2D.
  const THREE_D = [[2, 0], [3, 0], [6, 0], [7, 0], [8, 0], [9, 0], [10, 1], [11, 0], [14, 0], [15, 1]];

  function multiply(a, b) {
    const out = new Array(16);
    for (let c = 0; c < 4; c++) {
      for (let r = 0; r < 4; r++) {
        let s = 0;
        for (let k = 0; k < 4; k++) s += a[k * 4 + r] * b[c * 4 + k];
        out[c * 4 + r] = s;
      }
    }
    return out;
  }
  const translation = (x, y, z) => [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, x, y, z, 1];
  const scaling = (x, y, z) => [x, 0, 0, 0, 0, y, 0, 0, 0, 0, z, 0, 0, 0, 0, 1];
  const rad = (deg) => (deg * Math.PI) / 180;
  // CSS Transforms 2 §12 ("rotate3d"): rotación de `deg` grados alrededor de (x, y, z).
  function rotation(x, y, z, deg) {
    const len = Math.hypot(x, y, z);
    if (len === 0) return IDENTITY.slice();
    x /= len; y /= len; z /= len;
    const a = rad(deg) / 2, sc = Math.sin(a) * Math.cos(a), sq = Math.sin(a) ** 2;
    return [
      1 - 2 * (y * y + z * z) * sq, 2 * (x * y * sq + z * sc), 2 * (x * z * sq - y * sc), 0,
      2 * (x * y * sq - z * sc), 1 - 2 * (x * x + z * z) * sq, 2 * (y * z * sq + x * sc), 0,
      2 * (x * z * sq + y * sc), 2 * (y * z * sq - x * sc), 1 - 2 * (x * x + y * y) * sq, 0,
      0, 0, 0, 1,
    ];
  }
  // La rotación en Z exacta (sin el redondeo de la fórmula general) para 2D.
  function rotationZ(deg) {
    const t = rad(deg), c = Math.cos(t), s = Math.sin(t);
    return [c, s, 0, 0, -s, c, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
  }
  function invert(m) {
    const inv = new Array(16);
    inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15] + m[9] * m[7] * m[14] + m[13] * m[6] * m[11] - m[13] * m[7] * m[10];
    inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15] - m[8] * m[7] * m[14] - m[12] * m[6] * m[11] + m[12] * m[7] * m[10];
    inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15] + m[8] * m[7] * m[13] + m[12] * m[5] * m[11] - m[12] * m[7] * m[9];
    inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14] - m[8] * m[6] * m[13] - m[12] * m[5] * m[10] + m[12] * m[6] * m[9];
    inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15] - m[9] * m[3] * m[14] - m[13] * m[2] * m[11] + m[13] * m[3] * m[10];
    inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15] + m[8] * m[3] * m[14] + m[12] * m[2] * m[11] - m[12] * m[3] * m[10];
    inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15] - m[8] * m[3] * m[13] - m[12] * m[1] * m[11] + m[12] * m[3] * m[9];
    inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14] + m[8] * m[2] * m[13] + m[12] * m[1] * m[10] - m[12] * m[2] * m[9];
    inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15] + m[5] * m[3] * m[14] + m[13] * m[2] * m[7] - m[13] * m[3] * m[6];
    inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15] - m[4] * m[3] * m[14] - m[12] * m[2] * m[7] + m[12] * m[3] * m[6];
    inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15] + m[4] * m[3] * m[13] + m[12] * m[1] * m[7] - m[12] * m[3] * m[5];
    inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14] - m[4] * m[2] * m[13] - m[12] * m[1] * m[6] + m[12] * m[2] * m[5];
    inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11] - m[5] * m[3] * m[10] - m[9] * m[2] * m[7] + m[9] * m[3] * m[6];
    inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11] + m[4] * m[3] * m[10] + m[8] * m[2] * m[7] - m[8] * m[3] * m[6];
    inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11] - m[4] * m[3] * m[9] - m[8] * m[1] * m[7] + m[8] * m[3] * m[5];
    inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10] + m[4] * m[2] * m[9] + m[8] * m[1] * m[6] - m[8] * m[2] * m[5];
    const det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
    if (det === 0 || !Number.isFinite(det)) return null;
    return inv.map((v) => v / det);
  }

  // §6.2 "validate and fixup (2D)" y "validate and fixup".
  function fixup(init) {
    const d = init === undefined || init === null ? {} : init;
    if (typeof d !== 'object' && typeof d !== 'function') throw new TypeError('DOMMatrixInit must be an object');
    const read = (k) => (d[k] === undefined ? undefined : num(d[k]));
    const m = IDENTITY.slice();
    const given = NAMES.map((n) => read(n));
    for (const [alias, i] of Object.entries(ALIASES)) {
      const av = read(alias), mv = given[i];
      if (av !== undefined && mv !== undefined && !Object.is(av, mv) && !(av === 0 && mv === 0)) {
        throw new TypeError(`DOMMatrixInit: '${alias}' and '${NAMES[i]}' differ`);
      }
      if (mv === undefined && av !== undefined) given[i] = av;
    }
    given.forEach((v, i) => { if (v !== undefined) m[i] = v; });
    const flat = THREE_D.every(([i, v]) => m[i] === v);
    let is2D = d.is2D === undefined ? undefined : Boolean(d.is2D);
    if (is2D === true && !flat) throw new TypeError('DOMMatrixInit: is2D is true but the matrix is 3D');
    if (is2D === undefined) is2D = flat;
    return { m, is2D };
  }

  // §6.1: "parse a string into an abstract matrix" (una <transform-list> de CSS Transforms).
  const ANGLE = { deg: 1, rad: 180 / Math.PI, grad: 0.9, turn: 360 };
  function parseTransform(text) {
    let s = String(text).trim();
    if (s === '' || s === 'none') return { m: IDENTITY.slice(), is2D: true };
    let m = IDENTITY.slice(), is2D = true;
    const re = /^([a-zA-Z0-9]+)\(([^()]*)\)\s*/;
    while (s.length) {
      const f = re.exec(s);
      if (!f) throw syntaxError(`Failed to parse '${text}'.`);
      s = s.slice(f[0].length);
      const name = f[1].toLowerCase();
      const args = f[2].split(',').map((x) => x.trim());
      if (args.some((x) => x === '')) throw syntaxError(`Failed to parse '${text}'.`);
      const unit = (x, kind) => {
        const u = /^([+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?)([a-z%]*)$/i.exec(x);
        if (!u) throw syntaxError(`Failed to parse '${text}'.`);
        const v = Number(u[1]), un = u[2].toLowerCase();
        if (kind === 'number') { if (un) throw syntaxError(`Failed to parse '${text}'.`); return v; }
        if (un === '') { if (v === 0 || kind === 'number') return 0; throw syntaxError(`Failed to parse '${text}'.`); }
        if (kind === 'length' && un === 'px') return v;
        if (kind === 'angle' && un in ANGLE) return v * ANGLE[un];
        throw syntaxError(`Failed to parse '${text}'.`);
      };
      const n = (k) => args.map((x) => unit(x, k));
      const want = (lo, hi = lo) => { if (args.length < lo || args.length > hi) throw syntaxError(`Failed to parse '${text}'.`); };
      let t, three = false;
      switch (name) {
        case 'matrix': want(6); { const v = n('number'); t = [v[0], v[1], 0, 0, v[2], v[3], 0, 0, 0, 0, 1, 0, v[4], v[5], 0, 1]; } break;
        case 'matrix3d': want(16); t = n('number'); three = true; break;
        case 'translate': want(1, 2); { const v = n('length'); t = translation(v[0], v[1] || 0, 0); } break;
        case 'translatex': want(1); t = translation(n('length')[0], 0, 0); break;
        case 'translatey': want(1); t = translation(0, n('length')[0], 0); break;
        case 'translatez': want(1); t = translation(0, 0, n('length')[0]); three = true; break;
        case 'translate3d': want(3); { const v = n('length'); t = translation(v[0], v[1], v[2]); three = true; } break;
        case 'scale': want(1, 2); { const v = n('number'); t = scaling(v[0], v.length > 1 ? v[1] : v[0], 1); } break;
        case 'scalex': want(1); t = scaling(n('number')[0], 1, 1); break;
        case 'scaley': want(1); t = scaling(1, n('number')[0], 1); break;
        case 'scalez': want(1); t = scaling(1, 1, n('number')[0]); three = true; break;
        case 'scale3d': want(3); { const v = n('number'); t = scaling(v[0], v[1], v[2]); three = true; } break;
        case 'rotate': case 'rotatez': want(1); t = rotationZ(n('angle')[0]); three = name === 'rotatez'; break;
        case 'rotatex': want(1); t = rotation(1, 0, 0, n('angle')[0]); three = true; break;
        case 'rotatey': want(1); t = rotation(0, 1, 0, n('angle')[0]); three = true; break;
        case 'rotate3d': want(4); { const v = args.slice(0, 3).map((x) => unit(x, 'number')); t = rotation(v[0], v[1], v[2], unit(args[3], 'angle')); three = true; } break;
        case 'skew': want(1, 2); { const v = n('angle'); t = IDENTITY.slice(); t[4] = Math.tan(rad(v[0])); t[1] = Math.tan(rad(v[1] || 0)); } break;
        case 'skewx': want(1); t = IDENTITY.slice(); t[4] = Math.tan(rad(n('angle')[0])); break;
        case 'skewy': want(1); t = IDENTITY.slice(); t[1] = Math.tan(rad(n('angle')[0])); break;
        case 'perspective': want(1); { const d = n('length')[0]; t = IDENTITY.slice(); if (d !== 0) t[11] = -1 / d; three = true; } break;
        default: throw syntaxError(`Failed to parse '${text}'.`);
      }
      m = multiply(m, t);
      if (three) is2D = false;
    }
    return { m, is2D };
  }

  const fromInit = (init) => (typeof init === 'string' ? parseTransform(init) : null);

  class DOMMatrixReadOnly {
    #m; #is2D;
    constructor(init = undefined) {
      let r;
      if (init === undefined) {
        r = { m: IDENTITY.slice(), is2D: true };
      } else if (typeof init === 'string') {
        if (!isWindow) throw new TypeError('DOMMatrix: a string is only accepted in a Window');
        r = fromInit(init);
      } else {
        if (init === null || typeof init !== 'object') throw new TypeError('DOMMatrix: invalid init');
        const v = Array.from(init, num);
        if (v.length === 6) r = { m: [v[0], v[1], 0, 0, v[2], v[3], 0, 0, 0, 0, 1, 0, v[4], v[5], 0, 1], is2D: true };
        else if (v.length === 16) r = { m: v, is2D: false };
        else throw new TypeError(`DOMMatrix: the sequence must have 6 or 16 elements, not ${v.length}`);
      }
      this.#m = r.m; this.#is2D = r.is2D;
    }
    static fromMatrix(other) { return make(this, fixup(other)); }
    static fromFloat32Array(a) { return fromArray(this, a, Float32Array); }
    static fromFloat64Array(a) { return fromArray(this, a, Float64Array); }
    static _get(o) { return [o.#m, o.#is2D]; }
    static _set(o, m, is2D) { o.#m = m; o.#is2D = is2D; return o; }

    get is2D() { return this.#is2D; }
    get isIdentity() { return this.#m.every((v, i) => v === IDENTITY[i]); }

    translate(tx, ty, tz) { return copy(this).translateSelf(tx, ty, tz); }
    scale(sx, sy, sz, ox, oy, oz) { return copy(this).scaleSelf(sx, sy, sz, ox, oy, oz); }
    scaleNonUniform(sx = 1, sy = 1) { return copy(this).scaleSelf(sx, sy, 1, 0, 0, 0); }
    scale3d(s, ox, oy, oz) { return copy(this).scale3dSelf(s, ox, oy, oz); }
    rotate(rx, ry, rz) { return copy(this).rotateSelf(rx, ry, rz); }
    rotateFromVector(x, y) { return copy(this).rotateFromVectorSelf(x, y); }
    rotateAxisAngle(x, y, z, a) { return copy(this).rotateAxisAngleSelf(x, y, z, a); }
    skewX(s) { return copy(this).skewXSelf(s); }
    skewY(s) { return copy(this).skewYSelf(s); }
    multiply(other) { return copy(this).multiplySelf(other); }
    flipX() { return copy(this).multiplySelf({ a: -1 }); }
    flipY() { return copy(this).multiplySelf({ d: -1 }); }
    inverse() { return copy(this).invertSelf(); }
    transformPoint(point) {
      const p = pointInit(point), v = [p.x, p.y, p.z, p.w], m = this.#m, o = [0, 0, 0, 0];
      for (let r = 0; r < 4; r++) for (let c = 0; c < 4; c++) o[r] += m[c * 4 + r] * v[c];
      return new DOMPoint(o[0], o[1], o[2], o[3]);
    }
    toFloat32Array() { return new Float32Array(this.#m); }
    toFloat64Array() { return new Float64Array(this.#m); }
    toJSON() {
      const o = {};
      for (const k of ['a', 'b', 'c', 'd', 'e', 'f']) o[k] = this[k];
      NAMES.forEach((k, i) => { o[k] = this.#m[i]; });
      o.is2D = this.is2D; o.isIdentity = this.isIdentity;
      return o;
    }
    toString() {
      const m = this.#m;
      if (!m.every(Number.isFinite)) throw new DOMException('Cannot stringify a matrix with non-finite values.', 'InvalidStateError');
      const s = (v) => String(v);
      if (this.#is2D) return `matrix(${[m[0], m[1], m[4], m[5], m[12], m[13]].map(s).join(', ')})`;
      return `matrix3d(${m.map(s).join(', ')})`;
    }
  }
  const getState = DOMMatrixReadOnly._get, setState = DOMMatrixReadOnly._set;
  delete DOMMatrixReadOnly._get; delete DOMMatrixReadOnly._set;
  for (const [alias, i] of Object.entries(ALIASES)) {
    Object.defineProperty(DOMMatrixReadOnly.prototype, alias, { get() { return getState(this)[0][i]; }, enumerable: true, configurable: true });
  }
  NAMES.forEach((k, i) => {
    Object.defineProperty(DOMMatrixReadOnly.prototype, k, { get() { return getState(this)[0][i]; }, enumerable: true, configurable: true });
  });

  function make(Ctor, { m, is2D }) {
    const o = new Ctor();
    return setState(o, m.slice(), is2D);
  }
  function copy(matrix) {
    const [m, is2D] = getState(matrix);
    return make(DOMMatrix, { m, is2D });
  }
  function fromArray(Ctor, a, Kind) {
    if (!(a instanceof Kind)) throw new TypeError(`Argument must be a ${Kind.name}`);
    return new Ctor(Array.from(a));
  }
  // Post-multiplica `t` en la matriz de `o`.
  function post(o, t, keeps2D) {
    const [m, is2D] = getState(o);
    return setState(o, multiply(m, t), is2D && keeps2D);
  }

  class DOMMatrix extends DOMMatrixReadOnly {
    constructor(init = undefined) { super(init); }
    static fromMatrix(other) { return make(DOMMatrix, fixup(other)); }
    multiplySelf(other) {
      const r = fixup(other);
      return post(this, r.m, r.is2D);
    }
    preMultiplySelf(other) {
      const r = fixup(other), [m, is2D] = getState(this);
      return setState(this, multiply(r.m, m), is2D && r.is2D);
    }
    translateSelf(tx = 0, ty = 0, tz = 0) {
      tx = num(tx); ty = num(ty); tz = num(tz);
      return post(this, translation(tx, ty, tz), tz === 0);
    }
    scaleSelf(sx = 1, sy, sz = 1, ox = 0, oy = 0, oz = 0) {
      sx = num(sx); sy = sy === undefined ? sx : num(sy); sz = num(sz);
      ox = num(ox); oy = num(oy); oz = num(oz);
      post(this, translation(ox, oy, oz), true);
      post(this, scaling(sx, sy, sz), sz === 1 && oz === 0);
      return post(this, translation(-ox, -oy, -oz), true);
    }
    scale3dSelf(s = 1, ox = 0, oy = 0, oz = 0) {
      s = num(s); ox = num(ox); oy = num(oy); oz = num(oz);
      post(this, translation(ox, oy, oz), true);
      post(this, scaling(s, s, s), s === 1);
      return post(this, translation(-ox, -oy, -oz), true);
    }
    rotateSelf(rx = 0, ry, rz) {
      rx = num(rx);
      if (ry === undefined && rz === undefined) { rz = rx; rx = 0; ry = 0; }
      ry = ry === undefined ? 0 : num(ry); rz = rz === undefined ? 0 : num(rz);
      if (rz !== 0) post(this, rotationZ(rz), true);
      if (ry !== 0) post(this, rotation(0, 1, 0, ry), false);
      if (rx !== 0) post(this, rotation(1, 0, 0, rx), false);
      return this;
    }
    rotateFromVectorSelf(x = 0, y = 0) {
      x = num(x); y = num(y);
      const deg = x === 0 && y === 0 ? 0 : (Math.atan2(y, x) * 180) / Math.PI;
      return post(this, rotationZ(deg), true);
    }
    rotateAxisAngleSelf(x = 0, y = 0, z = 0, angle = 0) {
      x = num(x); y = num(y); z = num(z); angle = num(angle);
      return post(this, rotation(x, y, z, angle), x === 0 && y === 0);
    }
    skewXSelf(s = 0) {
      const t = IDENTITY.slice(); t[4] = Math.tan(rad(num(s)));
      return post(this, t, true);
    }
    skewYSelf(s = 0) {
      const t = IDENTITY.slice(); t[1] = Math.tan(rad(num(s)));
      return post(this, t, true);
    }
    invertSelf() {
      const [m, is2D] = getState(this);
      const inv = invert(m);
      if (!inv) return setState(this, new Array(16).fill(NaN), false);
      return setState(this, inv, is2D);
    }
    setMatrixValue(text) {
      if (!isWindow) throw new TypeError('setMatrixValue is only available in a Window');
      const r = parseTransform(text);
      return setState(this, r.m, r.is2D);
    }
  }
  for (const [alias, i] of Object.entries(ALIASES)) {
    Object.defineProperty(DOMMatrix.prototype, alias, {
      get() { return getState(this)[0][i]; },
      set(v) { getState(this)[0][i] = num(v); },
      enumerable: true, configurable: true,
    });
  }
  NAMES.forEach((k, i) => {
    Object.defineProperty(DOMMatrix.prototype, k, {
      get() { return getState(this)[0][i]; },
      set(v) {
        const [m, is2D] = getState(this);
        m[i] = num(v);
        // §6.5: escribir un elemento 3D con un valor que no es el de la identidad la hace 3D.
        const flatIndex = THREE_D.find(([j]) => j === i);
        if (is2D && flatIndex && m[i] !== flatIndex[1]) setState(this, m, false);
      },
      enumerable: true, configurable: true,
    });
  });

  define('DOMPointReadOnly', DOMPointReadOnly);
  define('DOMPoint', DOMPoint);
  define('DOMQuad', DOMQuad);
  define('DOMMatrixReadOnly', DOMMatrixReadOnly);
  define('DOMMatrix', DOMMatrix);
  if (isWindow) define('WebKitCSSMatrix', DOMMatrix);
})
