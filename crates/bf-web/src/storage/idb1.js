// IndexedDB 3.0 (W3C): IDBFactory, IDBOpenDBRequest/IDBRequest, IDBDatabase, IDBTransaction,
// IDBObjectStore, IDBIndex, IDBKeyRange, IDBCursor/IDBCursorWithValue e
// IDBVersionChangeEvent. El estado de las bases de datos vive aquí (en memoria); `native`
// (bf-web/src/storage/idb.rs) dispara los eventos de confianza, enlaza el event path
// (request → transaction → database), encola tareas y guarda el log en disco. (E2)
(function (global, native) {
  "use strict";
  const TOKEN = {};
  const DOMExceptionCtor = global.DOMException;
  const err = (name, msg) => new DOMExceptionCtor(msg || name, name);
  const P = Promise;
  const hasOwn = (o, k) => Object.prototype.hasOwnProperty.call(o, k);

  const define = (o, name, value) =>
    Object.defineProperty(o, name, { value, writable: true, configurable: true, enumerable: false });
  const tag = (o, t) => Object.defineProperty(o, Symbol.toStringTag, { value: t, configurable: true });
  const enumerable = (proto) => {
    for (const n of Object.getOwnPropertyNames(proto)) {
      if (n === "constructor") continue;
      const d = Object.getOwnPropertyDescriptor(proto, n);
      d.enumerable = true;
      Object.defineProperty(proto, n, d);
    }
  };
  function illegal(token) {
    if (token !== TOKEN) throw new TypeError("Illegal constructor");
  }

  // --- Event handlers de las interfaces de este archivo (HTML §8.1.8) ---
  function defineHandlers(proto, types) {
    for (const type of types) {
      const H = new WeakMap();
      Object.defineProperty(proto, "on" + type, {
        configurable: true,
        enumerable: true,
        get() {
          const h = H.get(this);
          return h ? h.value : null;
        },
        set(v) {
          let h = H.get(this);
          const value = typeof v === "function" || (v !== null && typeof v === "object") ? v : null;
          if (!h) {
            h = { value };
            H.set(this, h);
            const self = this;
            EventTarget.prototype.addEventListener.call(this, type, function (e) {
              const f = h.value;
              if (typeof f !== "function") return;
              const r = f.call(self, e);
              if (r === false) e.preventDefault();
            });
          } else {
            h.value = value;
          }
        },
      });
    }
  }

  // --- Claves (§2.3, §7.1) ---
  // Una clave es {t, v}: t = 1 número, 2 fecha, 3 string, 4 binario, 5 array.
  const isDateObj = (v) => {
    try {
      Date.prototype.getTime.call(v);
      return true;
    } catch (e) {
      return false;
    }
  };
  const isBufferSource = (v) => v instanceof ArrayBuffer || ArrayBuffer.isView(v);
  function bytesOf(v) {
    if (v instanceof ArrayBuffer) return new Uint8Array(v.slice(0));
    return new Uint8Array(v.buffer.slice(v.byteOffset, v.byteOffset + v.byteLength));
  }

  // "Convert a value to a key": `null` si es inválido.
  function toKey(input, seen) {
    if (typeof input === "number") return Number.isNaN(input) ? null : { t: 1, v: input };
    if (typeof input === "string") return { t: 3, v: input };
    if (input === null || typeof input !== "object") return null;
    if (isDateObj(input)) {
      const ms = Date.prototype.getTime.call(input);
      return Number.isNaN(ms) ? null : { t: 2, v: ms };
    }
    if (isBufferSource(input)) {
      const buf = input instanceof ArrayBuffer ? input : input.buffer;
      if (buf.detached) return null;
      return { t: 4, v: bytesOf(input) };
    }
    if (Array.isArray(input)) {
      seen = seen || [];
      if (seen.includes(input)) return null;
      seen.push(input);
      const len = input.length;
      const keys = [];
      for (let i = 0; i < len; i++) {
        if (!hasOwn(input, i)) return null;
        const k = toKey(input[i], seen);
        if (k === null) return null;
        keys.push(k);
      }
      seen.pop();
      return { t: 5, v: keys };
    }
    return null;
  }

  function toKeyOrThrow(v) {
    const k = toKey(v);
    if (k === null) throw err("DataError", "The parameter is not a valid key.");
    return k;
  }

  // "Convert a key to a value".
  function keyValue(k) {
    switch (k.t) {
      case 1:
        return k.v;
      case 2:
        return new Date(k.v);
      case 3:
        return k.v;
      case 4:
        return k.v.slice().buffer;
      default:
        return k.v.map(keyValue);
    }
  }

  // "Compare two keys" (§2.3).
  function cmp(a, b) {
    if (a.t !== b.t) return a.t > b.t ? 1 : -1;
    switch (a.t) {
      case 1:
      case 2:
        return a.v < b.v ? -1 : a.v > b.v ? 1 : 0;
      case 3:
        return a.v < b.v ? -1 : a.v > b.v ? 1 : 0;
      case 4: {
        const n = Math.min(a.v.length, b.v.length);
        for (let i = 0; i < n; i++) {
          if (a.v[i] !== b.v[i]) return a.v[i] < b.v[i] ? -1 : 1;
        }
        return a.v.length < b.v.length ? -1 : a.v.length > b.v.length ? 1 : 0;
      }
      default: {
        const n = Math.min(a.v.length, b.v.length);
        for (let i = 0; i < n; i++) {
          const c = cmp(a.v[i], b.v[i]);
          if (c !== 0) return c;
        }
        return a.v.length < b.v.length ? -1 : a.v.length > b.v.length ? 1 : 0;
      }
    }
  }

  // Claves para el log (JSON).
  function b64(u8) {
    let s = "";
    for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode.apply(null, u8.subarray(i, i + 0x8000));
    return btoa(s);
  }
  function unb64(s) {
    const bin = atob(s);
    const u8 = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) u8[i] = bin.charCodeAt(i);
    return u8;
  }
  function keyToJson(k) {
    switch (k.t) {
      case 1:
        return ["n", Object.is(k.v, -0) ? 0 : k.v === Infinity ? "Inf" : k.v === -Infinity ? "-Inf" : k.v];
      case 2:
        return ["d", k.v];
      case 3:
        return ["s", k.v];
      case 4:
        return ["x", b64(k.v)];
      default:
        return ["a", k.v.map(keyToJson)];
    }
  }
  function keyFromJson(j) {
    switch (j[0]) {
      case "n":
        return { t: 1, v: j[1] === "Inf" ? Infinity : j[1] === "-Inf" ? -Infinity : j[1] };
      case "d":
        return { t: 2, v: j[1] };
      case "s":
        return { t: 3, v: j[1] };
      case "x":
        return { t: 4, v: unb64(j[1]) };
      default:
        return { t: 5, v: j[1].map(keyFromJson) };
    }
  }

  // --- IDBKeyRange (§4.7) ---
  const KR = new WeakMap();
  class IDBKeyRange {
    constructor(token) {
      illegal(token);
    }
    get lower() {
      const r = range(this);
      return r.lower === null ? undefined : keyValue(r.lower);
    }
    get upper() {
      const r = range(this);
      return r.upper === null ? undefined : keyValue(r.upper);
    }
    get lowerOpen() {
      return range(this).lowerOpen;
    }
    get upperOpen() {
      return range(this).upperOpen;
    }
    includes(key) {
      const r = range(this);
      if (arguments.length < 1) throw new TypeError("Failed to execute 'includes' on 'IDBKeyRange': 1 argument required.");
      return inRange(r, toKeyOrThrow(key));
    }
    static only(value) {
      if (arguments.length < 1) throw new TypeError("1 argument required");
      const k = toKeyOrThrow(value);
      return makeRange(k, k, false, false);
    }
    static lowerBound(lower, open = false) {
      if (arguments.length < 1) throw new TypeError("1 argument required");
      return makeRange(toKeyOrThrow(lower), null, !!open, true);
    }
    static upperBound(upper, open = false) {
      if (arguments.length < 1) throw new TypeError("1 argument required");
      return makeRange(null, toKeyOrThrow(upper), true, !!open);
    }
    static bound(lower, upper, lowerOpen = false, upperOpen = false) {
      if (arguments.length < 2) throw new TypeError("2 arguments required");
      const l = toKeyOrThrow(lower);
      const u = toKeyOrThrow(upper);
      const c = cmp(l, u);
      if (c > 0 || (c === 0 && (lowerOpen || upperOpen)))
        throw err("DataError", "The lower key is greater than the upper key.");
      return makeRange(l, u, !!lowerOpen, !!upperOpen);
    }
  }
  enumerable(IDBKeyRange.prototype);
  tag(IDBKeyRange.prototype, "IDBKeyRange");

  function range(r) {
    const x = KR.get(r);
    if (!x) throw new TypeError("Illegal invocation");
    return x;
  }
  function makeRange(lower, upper, lowerOpen, upperOpen) {
    const r = new IDBKeyRange(TOKEN);
    KR.set(r, { lower, upper, lowerOpen: lower === null ? true : lowerOpen, upperOpen: upper === null ? true : upperOpen });
    return r;
  }
  const UNBOUNDED = { lower: null, upper: null, lowerOpen: true, upperOpen: true };
  function inRange(r, k) {
    if (r.lower !== null) {
      const c = cmp(r.lower, k);
      if (c > 0 || (c === 0 && r.lowerOpen)) return false;
    }
    if (r.upper !== null) {
      const c = cmp(r.upper, k);
      if (c < 0 || (c === 0 && r.upperOpen)) return false;
    }
    return true;
  }
  // "Convert a value to a key range".
  function toRange(value, nullDisallowed) {
    if (KR.has(value)) return KR.get(value);
    if (value === undefined || value === null) {
      if (nullDisallowed) throw err("DataError", "No key or key range specified.");
      return UNBOUNDED;
    }
    const k = toKeyOrThrow(value);
    return { lower: k, upper: k, lowerOpen: false, upperOpen: false };
  }

  // --- Structured serialize for storage (HTML §2.7.3) ---
  const ERRORS = ["Error", "EvalError", "RangeError", "ReferenceError", "SyntaxError", "TypeError", "URIError"];
  const VIEWS = {
    Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array, Int32Array, Uint32Array,
    Float32Array, Float64Array, BigInt64Array, BigUint64Array, DataView,
  };
  function viewName(v) {
    for (const n in VIEWS) if (v instanceof VIEWS[n]) return n;
    return null;
  }
  const toStr = (v) => Object.prototype.toString.call(v);

  function serialize(value) {
    const memory = new Map();
    let next = 0;
    const ser = (v) => {
      switch (typeof v) {
        case "undefined":
          return ["u"];
        case "boolean":
          return ["b", v ? 1 : 0];
        case "string":
          return ["s", v];
        case "number":
          if (Number.isNaN(v)) return ["n", "NaN"];
          if (v === Infinity) return ["n", "Inf"];
          if (v === -Infinity) return ["n", "-Inf"];
          if (Object.is(v, -0)) return ["n", "-0"];
          return ["n", v];
        case "bigint":
          return ["i", v.toString()];
        case "symbol":
          throw err("DataCloneError", "A Symbol could not be cloned.");
        case "function":
          throw err("DataCloneError", "A function could not be cloned.");
      }
      if (v === null) return ["0"];
      if (memory.has(v)) return ["r", memory.get(v)];
      const id = next++;
      memory.set(v, id);
      if (native.isBlob(v)) {
        const p = native.blobParts(v);
        return ["bl", id, b64(p[0]), p[1], p[2], p[3]];
      }
      if (native.isHost(v)) throw err("DataCloneError", "The object could not be cloned.");
      if (isDateObj(v)) return ["d", id, Date.prototype.getTime.call(v)];
      if (toStr(v) === "[object RegExp]" && v instanceof RegExp) return ["re", id, v.source, v.flags];
      if (toStr(v) === "[object Boolean]") return ["B", id, Boolean.prototype.valueOf.call(v) ? 1 : 0];
      if (toStr(v) === "[object Number]") return ["N", id, ser(Number.prototype.valueOf.call(v))];
      if (toStr(v) === "[object String]") return ["S", id, String.prototype.valueOf.call(v)];
      if (typeof BigInt !== "undefined" && toStr(v) === "[object BigInt]") return ["I", id, BigInt.prototype.valueOf.call(v).toString()];
      if (v instanceof ArrayBuffer) {
        if (v.detached) throw err("DataCloneError", "An ArrayBuffer is detached and could not be cloned.");
        return ["ab", id, b64(new Uint8Array(v))];
      }
      if (ArrayBuffer.isView(v)) {
        const name = viewName(v);
        const len = name === "DataView" ? v.byteLength : v.length;
        return ["v", id, name, ser(v.buffer), v.byteOffset, len];
      }
      if (v instanceof Map) {
        const entries = [];
        for (const [k, x] of v) entries.push([ser(k), ser(x)]);
        return ["m", id, entries];
      }
      if (v instanceof Set) {
        const items = [];
        for (const x of v) items.push(ser(x));
        return ["st", id, items];
      }
      if (v instanceof Error) {
        let name = v.name;
        if (!ERRORS.includes(name)) name = "Error";
        const msg = hasOwn(v, "message") ? String(v.message) : undefined;
        return ["e", id, name, msg === undefined ? null : msg];
      }
      if (v instanceof Promise || v instanceof WeakMap || v instanceof WeakSet ||
        (typeof WeakRef !== "undefined" && v instanceof WeakRef))
        throw err("DataCloneError", "The object could not be cloned.");
      if (Array.isArray(v)) {
        const props = [];
        for (const k of Object.keys(v)) props.push([k, ser(v[k])]);
        return ["a", id, v.length, props];
      }
      const props = [];
      for (const k of Object.keys(v)) props.push([k, ser(v[k])]);
      return ["o", id, props];
    };
    return ser(value);
  }

  function deserialize(tree) {
    const memory = [];
    const de = (t) => {
      switch (t[0]) {
        case "u":
          return undefined;
        case "0":
          return null;
        case "b":
          return !!t[1];
        case "s":
          return t[1];
        case "n":
          return t[1] === "NaN" ? NaN : t[1] === "Inf" ? Infinity : t[1] === "-Inf" ? -Infinity : t[1] === "-0" ? -0 : t[1];
        case "i":
          return BigInt(t[1]);
        case "r":
          return memory[t[1]];
        case "d":
          return (memory[t[1]] = new Date(t[2]));
        case "re":
          return (memory[t[1]] = new RegExp(t[2], t[3]));
        case "B":
          return (memory[t[1]] = Object(!!t[2]));
        case "N":
          return (memory[t[1]] = Object(de(t[2])));
        case "S":
          return (memory[t[1]] = Object(t[2]));
        case "I":
          return (memory[t[1]] = Object(BigInt(t[2])));
        case "ab":
          return (memory[t[1]] = unb64(t[2]).buffer);
        case "v": {
          const buf = de(t[3]);
          const C = VIEWS[t[2]];
          return (memory[t[1]] = new C(buf, t[4], t[5]));
        }
        case "m": {
          const m = new Map();
          memory[t[1]] = m;
          for (const [k, x] of t[2]) m.set(de(k), de(x));
          return m;
        }
        case "st": {
          const s = new Set();
          memory[t[1]] = s;
          for (const x of t[2]) s.add(de(x));
          return s;
        }
        case "e": {
          const C = global[t[2]] || Error;
          const e = t[3] === null ? new C() : new C(t[3]);
          return (memory[t[1]] = e);
        }
        case "bl":
          return (memory[t[1]] = native.makeBlob(unb64(t[2]), t[3], t[4], t[5]));
        case "a": {
          const a = new Array(t[2]);
          memory[t[1]] = a;
          for (const [k, x] of t[3]) a[k] = de(x);
          return a;
        }
        default: {
          const o = {};
          memory[t[1]] = o;
          for (const [k, x] of t[2]) {
            Object.defineProperty(o, k, { value: de(x), writable: true, enumerable: true, configurable: true });
          }
          return o;
        }
      }
    };
    return de(tree);
  }

  // --- Key paths (§2.5, §7.5, §7.6) ---
  const IDENT = /^[$_\p{ID_Start}][$‌‍\p{ID_Continue}]*$/u;
  function validKeyPathString(s) {
    if (s === "") return true;
    return s.split(".").every((p) => IDENT.test(p));
  }
  function validKeyPath(kp) {
    if (Array.isArray(kp)) return kp.length > 0 && kp.every((x) => typeof x === "string" && validKeyPathString(x));
    return typeof kp === "string" && validKeyPathString(kp);
  }
  // `(DOMString or sequence<DOMString>)`.
  function convertKeyPath(kp) {
    if (kp !== null && typeof kp === "object" && typeof kp[Symbol.iterator] === "function" && !(kp instanceof String)) {
      return Array.from(kp, (x) => String(x));
    }
    return String(kp);
  }
  const FAIL = {};
  // "Evaluate a key path on a value".
  function evaluateKeyPath(value, kp) {
    if (Array.isArray(kp)) {
      const out = [];
      for (const p of kp) {
        const r = evaluateKeyPath(value, p);
        if (r === FAIL) return FAIL;
        out.push(r);
      }
      return out;
    }
    if (kp === "") return value;
    for (const id of kp.split(".")) {
      if (typeof value === "string" && id === "length") value = value.length;
      else if (Array.isArray(value) && id === "length") value = value.length;
      else if (native.isBlob(value) && id === "size") value = native.blobParts(value)[0].length;
      else if (native.isBlob(value) && id === "type") value = native.blobParts(value)[1];
      else if (native.isBlob(value) && id === "name" && native.blobParts(value)[2] !== null) value = native.blobParts(value)[2];
      else if (native.isBlob(value) && id === "lastModified" && native.blobParts(value)[2] !== null) value = native.blobParts(value)[3];
      else if (native.isBlob(value) && id === "lastModifiedDate" && native.blobParts(value)[2] !== null) value = new Date(native.blobParts(value)[3]);
      else {
        if (value === null || typeof value !== "object") return FAIL;
        if (!hasOwn(value, id)) return FAIL;
        value = value[id];
        if (value === undefined) return FAIL;
      }
    }
    return value;
  }
  // "Extract a key from a value using a key path": FAIL, `null` (inválida) o la clave.
  function extractKey(value, kp, multiEntry) {
    const r = evaluateKeyPath(value, kp);
    if (r === FAIL) return FAIL;
    if (!multiEntry || !Array.isArray(r)) return toKey(r);
    const keys = [];
    const seen = [r];
    for (let i = 0; i < r.length; i++) {
      if (!hasOwn(r, i)) continue;
      const k = toKey(r[i], seen);
      if (k !== null && !keys.some((x) => cmp(x, k) === 0)) keys.push(k);
    }
    return { t: 5, v: keys, multi: true };
  }
  // "Check that a key could be injected into a value".
  function canInject(value, kp) {
    const ids = kp.split(".");
    ids.pop();
    for (const id of ids) {
      if (value === null || typeof value !== "object") return false;
      if (!hasOwn(value, id)) return true;
      value = value[id];
    }
    return value !== null && typeof value === "object";
  }
  // "Inject a key into a value using a key path".
  function inject(value, key, kp) {
    const ids = kp.split(".");
    const last = ids.pop();
    for (const id of ids) {
      if (!hasOwn(value, id)) {
        Object.defineProperty(value, id, { value: {}, writable: true, enumerable: true, configurable: true });
      }
      value = value[id];
    }
    Object.defineProperty(value, last, { value: keyValue(key), writable: true, enumerable: true, configurable: true });
  }

  // --- DOMStringList (HTML §2.6.4) ---
  const DSL = new WeakMap();
  class DOMStringList {
    constructor(token) {
      illegal(token);
    }
    get length() {
      return dsl(this).length;
    }
    item(i) {
      const l = dsl(this);
      i = Number(i) >>> 0;
      return i < l.length ? l[i] : null;
    }
    contains(s) {
      return dsl(this).includes(String(s));
    }
  }
  enumerable(DOMStringList.prototype);
  tag(DOMStringList.prototype, "DOMStringList");
  function dsl(o) {
    const l = DSL.get(o);
    if (!l) throw new TypeError("Illegal invocation");
    return l;
  }
  function makeStringList(names) {
    const l = new DOMStringList(TOKEN);
    const sorted = names.slice().sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
    DSL.set(l, sorted);
    sorted.forEach((n, i) => Object.defineProperty(l, i, { value: n, enumerable: true, configurable: true }));
    return l;
  }
