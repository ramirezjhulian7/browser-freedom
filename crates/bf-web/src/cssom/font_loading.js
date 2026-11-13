// CSS Font Loading 3 (https://drafts.csswg.org/css-font-loading/): `FontFace` (§2),
// `FontFaceSetLoadEvent` (§3.1), `FontFaceSet` (§3) y `document.fonts` (§4). (E5)
//
// Las interfaces están aquí; las funciones nativas (font_loading.rs) validan los valores con
// los parsers de CSS del motor, descargan y comprueban las fuentes, y dejan en el documento las
// caras añadidas desde scripts como un bloque `@font-face` que la maqueta lee
// (`bf_style::loader::SCRIPT_FONT_FACES`).
(function (global, native) {
  "use strict";
  const TOKEN = {};
  const P = Promise;
  const DOMExceptionCtor = global.DOMException;
  const err = (name, msg) => new DOMExceptionCtor(msg || name, name);
  const ET = global.EventTarget;
  const Ev = global.Event;

  const tag = (o, t) => Object.defineProperty(o, Symbol.toStringTag, { value: t, configurable: true });
  const enumerable = (proto) => {
    for (const n of Object.getOwnPropertyNames(proto)) {
      if (n === "constructor") continue;
      const d = Object.getOwnPropertyDescriptor(proto, n);
      d.enumerable = true;
      Object.defineProperty(proto, n, d);
    }
  };
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
            ET.prototype.addEventListener.call(this, type, function (e) {
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
  const queueTask = (f) => native.queueTask(f);

  // --- Descriptores (§2.1) ---

  // Los nombres que no pueden ser un <family-name> sin comillas (genéricas y palabras clave
  // globales, CSS Fonts 4 §4.2).
  const RESERVED = new Set([
    "serif", "sans-serif", "monospace", "cursive", "fantasy", "system-ui", "ui-serif",
    "ui-sans-serif", "ui-monospace", "ui-rounded", "math", "emoji", "fangsong", "inherit",
    "initial", "unset", "default", "revert", "revert-layer",
  ]);
  const IDENT = /^-?(?:[A-Za-z_]|[^\x00-\x7f]|\\.)(?:[-A-Za-z0-9_]|[^\x00-\x7f]|\\.)*$/;
  // La serialización de la familia: identificadores separados por un espacio si se puede; si
  // no, una cadena (CSSOM §2.1 "serialize a string").
  function serializeFamily(name) {
    const parts = name.split(" ");
    const idents =
      name.length > 0 &&
      parts.every((p) => IDENT.test(p)) &&
      !(parts.length === 1 && RESERVED.has(name.toLowerCase()));
    return idents ? name : '"' + name.replace(/["\\]/g, (c) => "\\" + c) + '"';
  }
  // El nombre de familia que usa la maqueta (sin comillas).
  function familyName(serialized) {
    const s = serialized.trim();
    if (s.length >= 2 && (s[0] === '"' || s[0] === "'") && s[s.length - 1] === s[0]) {
      return s.slice(1, -1).replace(/\\(.)/g, "$1");
    }
    return s.replace(/\s+/g, " ");
  }

  const URANGE = /^\s*[Uu]\+[0-9A-Fa-f?]{1,6}(?:-[0-9A-Fa-f]{1,6})?\s*$/;
  const PERCENT = /^\s*(?:\d+(?:\.\d+)?|\.\d+)%\s*$/;
  // nombre del atributo → [descriptor CSS, valor inicial, validación]
  function range(prop) {
    return (v) => {
      const parts = v.trim().split(/\s+/);
      if (parts.length === 2 && !/^oblique$/i.test(parts[0])) {
        const a = native.parseLonghand(prop, parts[0]);
        const b = native.parseLonghand(prop, parts[1]);
        return a !== null && b !== null && a !== "normal" && b !== "normal" ? a + " " + b : null;
      }
      return native.parseLonghand(prop, v);
    };
  }
  // `font-stretch` (CSS Fonts 4 §2.3; el motor no tiene la propiedad): palabra clave o
  // porcentaje no negativo; en el descriptor, también un rango de dos.
  const STRETCH = new Set([
    "normal", "ultra-condensed", "extra-condensed", "condensed", "semi-condensed",
    "semi-expanded", "expanded", "extra-expanded", "ultra-expanded",
  ]);
  const stretchOne = (t) => (STRETCH.has(t.toLowerCase()) ? t.toLowerCase() : PERCENT.test(t) ? t : null);
  const stretchValue = (v) => {
    const parts = v.trim().split(/\s+/);
    if (parts.length === 1) return stretchOne(parts[0]);
    if (parts.length !== 2) return null;
    const a = stretchOne(parts[0]);
    const b = stretchOne(parts[1]);
    return a && b && a !== "normal" && b !== "normal" ? a + " " + b : null;
  };
  // `font-feature-settings` y `font-variation-settings` (CSS Fonts 4 §6.12, §7.2): `normal` o
  // una lista de etiquetas de cuatro caracteres con su valor.
  const TAG = /^\s*("[\x20-\x7e]{4}"|'[\x20-\x7e]{4}')/;
  function tagList(v, value, normalizeValue) {
    const t = v.trim();
    if (/^normal$/i.test(t)) return "normal";
    const items = t.split(",");
    const out = [];
    for (const item of items) {
      const m = TAG.exec(item);
      if (!m) return null;
      const rest = item.slice(m[0].length).trim();
      if (!value(rest)) return null;
      out.push('"' + m[1].slice(1, -1) + '"' + (rest ? " " + rest : ""));
      if (normalizeValue) out[out.length - 1] = normalizeValue(out[out.length - 1], rest);
    }
    return out.join(", ");
  }
  // El valor 1 (u `on`) se omite y `off` es 0 (CSS Fonts 4 §6.12, serialización).
  const featureSettings = (v) =>
    tagList(
      v,
      (r) => r === "" || /^(on|off)$/i.test(r) || /^\d+$/.test(r),
      (item, r) => {
        const tag = item.slice(0, 6);
        if (r === "" || /^on$/i.test(r) || r === "1") return tag;
        if (/^off$/i.test(r)) return tag + " 0";
        return tag + " " + String(Number(r));
      }
    );
  const variationSettings = (v) => tagList(v, (r) => /^[-+]?(\d+(\.\d+)?|\.\d+)(e[-+]?\d+)?$/i.test(r));
  const overrideValue = (v) => {
    const t = v.trim();
    if (/^normal$/i.test(t)) return "normal";
    return PERCENT.test(t) ? t : null;
  };
  const DESCRIPTORS = {
    style: ["font-style", "normal", (v) => native.parseLonghand("font-style", v)],
    weight: ["font-weight", "normal", range("font-weight")],
    stretch: ["font-stretch", "normal", stretchValue],
    unicodeRange: [
      "unicode-range",
      "U+0-10FFFF",
      (v) => {
        const items = v.split(",");
        if (!items.length || !items.every((i) => URANGE.test(i))) return null;
        // Serialización (CSSOM): hexadecimal en mayúsculas sin ceros a la izquierda.
        const hex = (h) => (/\?/.test(h) ? h.toUpperCase() : parseInt(h, 16).toString(16).toUpperCase());
        return items
          .map((i) => {
            const [a, b] = i.trim().slice(2).split("-");
            return "U+" + hex(a) + (b === undefined ? "" : "-" + hex(b));
          })
          .join(", ");
      },
    ],
    featureSettings: ["font-feature-settings", "normal", featureSettings],
    variationSettings: ["font-variation-settings", "normal", variationSettings],
    display: [
      "font-display",
      "auto",
      (v) => {
        const t = v.trim().toLowerCase();
        return ["auto", "block", "swap", "fallback", "optional"].includes(t) ? t : null;
      },
    ],
    ascentOverride: ["ascent-override", "normal", overrideValue],
    descentOverride: ["descent-override", "normal", overrideValue],
    lineGapOverride: ["line-gap-override", "normal", overrideValue],
    sizeAdjust: ["size-adjust", "100%", (v) => (PERCENT.test(v) ? v.trim() : null)],
  };

  // --- FontFace (§2) ---
  const F = new WeakMap();
  function face(o) {
    const f = F.get(o);
    if (!f) throw new TypeError("Illegal invocation");
    return f;
  }
  // Las cabeceras de los formatos de fuente (TrueType, OpenType, colección, WOFF, WOFF2).
  function isFontData(bytes) {
    if (bytes.length < 12) return false;
    const h = String.fromCharCode(bytes[0], bytes[1], bytes[2], bytes[3]);
    return (
      (bytes[0] === 0 && bytes[1] === 1 && bytes[2] === 0 && bytes[3] === 0) ||
      h === "OTTO" || h === "true" || h === "ttcf" || h === "wOFF" || h === "wOF2"
    );
  }
  function bytesOf(source) {
    if (source instanceof ArrayBuffer) return new Uint8Array(source.slice(0));
    if (ArrayBuffer.isView(source)) {
      return new Uint8Array(source.buffer.slice(source.byteOffset, source.byteOffset + source.byteLength));
    }
    return null;
  }
  function settle(o, f, ok, error) {
    if (f.status === "loaded" || f.status === "error") return;
    f.status = ok ? "loaded" : "error";
    if (ok) f.resolve(o);
    else f.reject(error);
    for (const set of f.sets) setFaceDone(set, o, ok);
    changed(o);
  }
  // "Load a font face" para una cara con URLs (§2.2): la descarga va en una tarea.
  function startLoad(o, f) {
    if (f.status !== "unloaded") return;
    f.status = "loading";
    for (const set of f.sets) setFaceLoading(set, o);
    queueTask(() => {
      const ok = f.sources !== null && native.fetchFont(f.sources);
      settle(o, f, ok, ok ? null : err("NetworkError", "The font could not be loaded."));
    });
  }
  class FontFace {
    constructor(family, source, descriptors = {}) {
      if (arguments.length < 2) throw new TypeError("Failed to construct 'FontFace': 2 arguments required.");
      family = String(family);
      const d = descriptors == null ? {} : Object(descriptors);
      const f = {
        family: serializeFamily(family),
        status: "unloaded",
        sources: null,
        data: null,
        css: null,
        sets: new Set(),
        descs: {},
      };
      f.loaded = new P((res, rej) => {
        f.resolve = res;
        f.reject = rej;
      });
      f.loaded.catch(() => {});
      F.set(this, f);
      let error = null;
      // `width` es alias de `stretch` y tiene prioridad (CSS Fonts 4 §4.4).
      const given = {};
      for (const k of Object.keys(DESCRIPTORS)) {
        const v = k === "stretch" && d.width !== undefined ? d.width : d[k];
        given[k] = v === undefined ? undefined : String(v);
      }
      for (const k of Object.keys(DESCRIPTORS)) {
        const [, initial, parse] = DESCRIPTORS[k];
        if (given[k] === undefined) {
          f.descs[k] = initial;
          continue;
        }
        const v = parse(given[k]);
        if (v === null) {
          error = error || err("SyntaxError", "Invalid value for descriptor '" + k + "'.");
          f.descs[k] = initial;
        } else {
          f.descs[k] = v;
        }
      }
      if (typeof source === "string" || !bytesOf(source)) {
        const text = String(source);
        const list = native.parseSrc(text, global.document ? global.document.baseURI : "");
        if (list === null) error = error || err("SyntaxError", "Invalid source.");
        else f.sources = list;
        if (error) {
          f.status = "error";
          f.reject(error);
        }
      } else {
        // Datos binarios (§2.1, paso 5): "loading" y, en una tarea, el resultado.
        const bytes = bytesOf(source);
        f.status = "loading";
        if (error) {
          f.status = "error";
          f.reject(error);
        } else {
          queueTask(() => {
            if (isFontData(bytes)) {
              f.data = bytes;
              settle(this, f, true);
            } else {
              settle(this, f, false, err("SyntaxError", "The font data is not a valid font."));
            }
          });
        }
      }
    }
    get family() {
      return face(this).family;
    }
    set family(v) {
      const f = face(this);
      f.family = serializeFamily(String(v));
      changed(this);
    }
    get status() {
      return face(this).status;
    }
    get loaded() {
      return face(this).loaded;
    }
    load() {
      let f;
      try {
        f = face(this);
      } catch (e) {
        return P.reject(e);
      }
      if (f.css && f.status === "unloaded") {
        f.sources = native.parseSrc(f.css.src, f.css.base);
      }
      startLoad(this, f);
      return f.loaded;
    }
  }
  for (const k of Object.keys(DESCRIPTORS)) {
    Object.defineProperty(FontFace.prototype, k, {
      configurable: true,
      enumerable: true,
      get() {
        return face(this).descs[k];
      },
      set(v) {
        const f = face(this);
        const parsed = DESCRIPTORS[k][2](String(v));
        if (parsed === null) throw err("SyntaxError", "Invalid value for descriptor '" + k + "'.");
        f.descs[k] = parsed;
        changed(this);
      },
    });
  }
  // `width` (CSS Fonts 4): alias de `stretch`.
  Object.defineProperty(FontFace.prototype, "width", Object.getOwnPropertyDescriptor(FontFace.prototype, "stretch"));
  enumerable(FontFace.prototype);
  tag(FontFace.prototype, "FontFace");

  // --- FontFaceSetLoadEvent (§3.1) ---
  const LE = new WeakMap();
  class FontFaceSetLoadEvent extends Ev {
    constructor(type, init = {}) {
      if (arguments.length < 1) throw new TypeError("Failed to construct 'FontFaceSetLoadEvent': 1 argument required.");
      super(type, init);
      const o = init == null ? {} : init;
      const list = o.fontfaces === undefined ? [] : Array.from(o.fontfaces);
      for (const x of list) face(x);
      LE.set(this, Object.freeze(list));
    }
    get fontfaces() {
      const v = LE.get(this);
      if (!v) throw new TypeError("Illegal invocation");
      return v;
    }
  }
  enumerable(FontFaceSetLoadEvent.prototype);
  tag(FontFaceSetLoadEvent.prototype, "FontFaceSetLoadEvent");

  // --- FontFaceSet (§3) ---
  const S = new WeakMap();
  function setOf(o) {
    const s = S.get(o);
    if (!s) throw new TypeError("Illegal invocation");
    return s;
  }
  function newReady(s, o) {
    s.readyResolved = false;
    s.ready = new P((res) => {
      s.readyResolve = () => {
        s.readyResolved = true;
        res(o);
      };
    });
  }
  // Las caras del conjunto: primero las de las reglas `@font-face` del documento (CSS-connected,
  // §4.1), en orden, y después las añadidas.
  function members(s) {
    return s.doc ? cssFaces(s).concat(s.faces) : s.faces.slice();
  }
  function setFaceLoading(o, faceObj) {
    const s = S.get(o);
    if (!s) return;
    if (s.loading.size === 0) {
      s.status = "loading";
      if (s.readyResolved) newReady(s, o);
      queueTask(() => o.dispatchEvent(new FontFaceSetLoadEvent("loading")));
    }
    s.loading.add(faceObj);
  }
  function setFaceDone(o, faceObj, ok) {
    const s = S.get(o);
    if (!s || !s.loading.has(faceObj)) return;
    s.loading.delete(faceObj);
    (ok ? s.loaded : s.failed).push(faceObj);
    if (s.loading.size === 0) switchToLoaded(o, s);
  }
  // "Switch the FontFaceSet to loaded" (§3.3), cuando además el entorno no tiene pendientes
  // (§4.2: el documento terminó de cargar y la maqueta usa ya las caras).
  function switchToLoaded(o, s) {
    if (s.loading.size !== 0) return;
    if (s.doc && s.doc.readyState !== "complete") return;
    queueTask(() => {
      if (s.loading.size !== 0) return;
      s.status = "loaded";
      const loaded = s.loaded;
      const failed = s.failed;
      s.loaded = [];
      s.failed = [];
      if (loaded.length || failed.length) {
        o.dispatchEvent(new FontFaceSetLoadEvent("loadingdone", { fontfaces: loaded }));
        if (failed.length) o.dispatchEvent(new FontFaceSetLoadEvent("loadingerror", { fontfaces: failed }));
      }
      if (!s.readyResolved) s.readyResolve();
    });
  }
  // Un `FontFaceSet` del documento: un `EventTarget` del motor con el prototipo de la clase.
  function makeSet(doc, initial) {
    const o = Reflect.construct(ET, [], FontFaceSet);
    return initSet(o, doc, initial);
  }
  function initSet(o, doc, initial) {
    const s = { faces: [], doc, status: "loaded", loading: new Set(), loaded: [], failed: [], css: new Map() };
    S.set(o, s);
    newReady(s, o);
    for (const x of initial) add(o, s, x);
    if (doc) {
      if (doc.readyState === "complete") switchToLoaded(o, s);
      else global.addEventListener("load", () => switchToLoaded(o, s));
    } else {
      switchToLoaded(o, s);
    }
    return o;
  }
  function add(o, s, faceObj) {
    const f = face(faceObj);
    if (f.css) throw err("InvalidModificationError", "CSS-connected FontFaces cannot be added.");
    if (s.faces.includes(faceObj)) return;
    s.faces.push(faceObj);
    f.sets.add(o);
    if (f.status === "loading") setFaceLoading(o, faceObj);
    publish(s);
  }
  // Sin constructor público (CSS Font Loading 3 lo retiró; WPT `historical.html`).
  class FontFaceSet extends ET {
    constructor(token) {
      if (token !== TOKEN) throw new TypeError("Illegal constructor");
      super();
    }
    get size() {
      return members(setOf(this)).length;
    }
    get ready() {
      return setOf(this).ready;
    }
    get status() {
      return setOf(this).status;
    }
    add(font) {
      add(this, setOf(this), font);
      return this;
    }
    has(font) {
      return members(setOf(this)).includes(font);
    }
    delete(font) {
      const s = setOf(this);
      const f = face(font);
      if (f.css) return false;
      const i = s.faces.indexOf(font);
      if (i < 0) return false;
      s.faces.splice(i, 1);
      f.sets.delete(this);
      if (s.loading.delete(font) && s.loading.size === 0) switchToLoaded(this, s);
      publish(s);
      return true;
    }
    clear() {
      const s = setOf(this);
      for (const x of s.faces) {
        face(x).sets.delete(this);
        s.loading.delete(x);
      }
      s.faces = [];
      publish(s);
    }
    forEach(callback, thisArg) {
      if (typeof callback !== "function") throw new TypeError("callback is not a function");
      for (const x of members(setOf(this))) callback.call(thisArg, x, x, this);
    }
    entries() {
      return members(setOf(this)).map((x) => [x, x])[Symbol.iterator]();
    }
    values() {
      return members(setOf(this))[Symbol.iterator]();
    }
    // "Find the matching font faces" (§3.5.1).
    load(font, text = " ") {
      let s;
      try {
        s = setOf(this);
      } catch (e) {
        return P.reject(e);
      }
      const matched = matching(s, String(font));
      if (matched === null) return P.reject(err("SyntaxError", "Invalid font."));
      return P.all(matched.map((x) => x.load())).then(() => matched);
    }
    check(font, text = " ") {
      const s = setOf(this);
      const matched = matching(s, String(font));
      if (matched === null) throw err("SyntaxError", "Invalid font.");
      return matched.every((x) => face(x).status === "loaded");
    }
  }
  FontFaceSet.prototype.keys = FontFaceSet.prototype.values;
  FontFaceSet.prototype[Symbol.iterator] = FontFaceSet.prototype.values;
  defineHandlers(FontFaceSet.prototype, ["loading", "loadingdone", "loadingerror"]);
  enumerable(FontFaceSet.prototype);
  Object.defineProperty(FontFaceSet.prototype, Symbol.iterator, { enumerable: false });
  tag(FontFaceSet.prototype, "FontFaceSet");

  // Las caras que coinciden con `font` (el shorthand `font`; `null` si no es válido o usa una
  // palabra clave global o `var()`): las del conjunto de alguna de sus familias.
  function matching(s, font) {
    const parsed = native.parseFont(font);
    if (parsed === null) return null;
    const names = parsed.map((n) => n.toLowerCase());
    return members(s).filter((x) => names.includes(familyName(face(x).family).toLowerCase()));
  }

  // --- Caras CSS-connected (§4.1) ---
  function rulesOf(list, out, base) {
    for (const r of Array.from(list || [])) {
      if (r.type === 5) out.push([r, base]);
      else if (r.cssRules) rulesOf(r.cssRules, out, base);
    }
  }
  function cssFaces(s) {
    const out = [];
    const doc = s.doc;
    let sheets = [];
    try {
      sheets = Array.from(doc.styleSheets || []);
    } catch (e) {
      sheets = [];
    }
    const rules = [];
    for (const sheet of sheets) {
      let list = null;
      try {
        list = sheet.cssRules;
      } catch (e) {
        list = null;
      }
      rulesOf(list, rules, sheet.href || doc.baseURI);
    }
    const seen = new Map();
    for (const [rule, base] of rules) {
      let x = s.css.get(rule);
      if (!x) {
        const st = rule.style;
        const get = (n) => st.getPropertyValue(n);
        const descs = {};
        for (const k of Object.keys(DESCRIPTORS)) {
          const [prop, initial, parse] = DESCRIPTORS[k];
          const v = get(prop);
          descs[k] = v ? parse(v) || initial : initial;
        }
        x = new FontFace(familyName(get("font-family")), "local(x)", {});
        const f = face(x);
        f.family = serializeFamily(familyName(get("font-family")));
        f.descs = descs;
        f.css = { src: get("src"), base };
        f.sources = null;
        f.status = "unloaded";
        f.sets.add(s.self);
        s.css.set(rule, x);
      }
      seen.set(rule, x);
      out.push(x);
    }
    for (const k of Array.from(s.css.keys())) if (!seen.has(k)) s.css.delete(k);
    return out;
  }

  // --- Lo que ve la maqueta ---
  function changed(faceObj) {
    const f = F.get(faceObj);
    if (!f) return;
    for (const set of f.sets) {
      const s = S.get(set);
      if (s) publish(s);
    }
  }
  function cssText(x) {
    const f = face(x);
    if (f.status === "error") return "";
    let src;
    if (f.data) {
      let bin = "";
      for (let i = 0; i < f.data.length; i += 0x8000) {
        bin += String.fromCharCode.apply(null, f.data.subarray(i, i + 0x8000));
      }
      src = 'url("data:font/ttf;base64,' + global.btoa(bin) + '")';
    } else if (f.sources && f.sources.length) {
      src = f.sources.map((u) => (u.startsWith("local:") ? "local(" + JSON.stringify(u.slice(6)) + ")" : "url(" + JSON.stringify(u.slice(4)) + ")")).join(", ");
    } else {
      return "";
    }
    const d = f.descs;
    return (
      "@font-face{font-family:" + JSON.stringify(familyName(f.family)) + ";src:" + src +
      ";font-style:" + d.style + ";font-weight:" + d.weight + ";font-stretch:" + d.stretch +
      ";unicode-range:" + d.unicodeRange + "}"
    );
  }
  // Solo el conjunto del documento de la ventana llega a su maqueta.
  function publish(s) {
    if (!s.doc || s.doc !== global.document) return;
    native.setFaces(s.faces.map(cssText).filter((t) => t).join("\n"));
  }

  // --- document.fonts (§4) ---
  const sets = new WeakMap();
  function fontsOf(doc) {
    let o = sets.get(doc);
    if (!o) {
      o = makeSet(doc, []);
      S.get(o).self = o;
      sets.set(doc, o);
    }
    return o;
  }
  const D = global.Document;
  if (D) {
    Object.defineProperty(D.prototype, "fonts", {
      configurable: true,
      enumerable: true,
      get() {
        if (!(this instanceof D)) throw new TypeError("Illegal invocation");
        return fontsOf(this);
      },
    });
  }
  for (const [name, value] of [
    ["FontFace", FontFace],
    ["FontFaceSet", FontFaceSet],
    ["FontFaceSetLoadEvent", FontFaceSetLoadEvent],
  ]) {
    Object.defineProperty(global, name, { value, writable: true, configurable: true, enumerable: false });
  }
});
