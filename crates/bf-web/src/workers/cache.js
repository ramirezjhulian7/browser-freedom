// Cache API (Service Workers, https://w3c.github.io/ServiceWorker/): `Cache` (§5.4),
// `CacheStorage` (§5.5) y `caches` de WindowOrWorkerGlobalScope (§5.3.1). El almacén y
// "Query Cache" / "Batch Cache Operations" están en Rust (bf-web/src/workers/cache.rs,
// compartido entre agentes); aquí van los argumentos, los fetch de `addAll`, la lectura de
// los cuerpos y las promesas. (E3)
(function (global, native) {
  "use strict";
  const TOKEN = {};
  const P = Promise;
  const DOMExceptionCtor = global.DOMException;
  const err = (name, msg) => new DOMExceptionCtor(msg || name, name);

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

  // --- Los valores iniciales de Fetch (se toman al instalar; si faltan, al primer uso) ---
  const getter = (proto, name) => {
    const d = proto && Object.getOwnPropertyDescriptor(proto, name);
    return d && d.get;
  };
  let fx = null;
  function fetchIntrinsics() {
    if (fx && fx.Request) return fx;
    const RP = global.Response && global.Response.prototype;
    fx = {
      Request: global.Request,
      fetch: global.fetch,
      arrayBuffer: RP && RP.arrayBuffer,
      body: getter(RP, "body"),
      bodyUsed: getter(RP, "bodyUsed"),
      locked: getter(global.ReadableStream && global.ReadableStream.prototype, "locked"),
    };
    return fx;
  }
  fetchIntrinsics();

  // --- Utilidades ---

  // Un método que devuelve una promesa: las excepciones síncronas (conversión de argumentos,
  // `this` inválido) rechazan la promesa (WebIDL §3.6.8).
  function wrap(f) {
    try {
      return f();
    } catch (e) {
      return P.reject(e);
    }
  }

  // Los pasos "in parallel" + "queue a task to resolve/reject": `work` corre ahora (el
  // almacén es atómico) y la promesa se resuelve en una tarea.
  function inTask(work) {
    return new P((resolve, reject) => {
      let out;
      let failed = false;
      try {
        out = work();
      } catch (e) {
        failed = true;
        out = e;
      }
      native.queueTask(() => (failed ? reject(out) : resolve(out)));
    });
  }

  function need(n, args, iface, op) {
    if (args.length < n) {
      throw new TypeError(
        `Failed to execute '${op}' on '${iface}': ${n} argument${n > 1 ? "s" : ""} required, but only ${args.length} present.`
      );
    }
  }

  // `CacheQueryOptions` / `MultiCacheQueryOptions` (§5.4, §5.5): miembros en orden
  // lexicográfico.
  function queryOptions(o, multi) {
    const out = { cacheName: undefined, ignoreMethod: false, ignoreSearch: false, ignoreVary: false };
    if (o === undefined || o === null) return out;
    if (typeof o !== "object" && typeof o !== "function") {
      throw new TypeError(`The provided value is not of type '${multi ? "MultiCacheQueryOptions" : "CacheQueryOptions"}'.`);
    }
    if (multi) {
      const c = o.cacheName;
      if (c !== undefined) out.cacheName = String(c);
    }
    out.ignoreMethod = !!o.ignoreMethod;
    out.ignoreSearch = !!o.ignoreSearch;
    out.ignoreVary = !!o.ignoreVary;
    return out;
  }

  // `RequestInfo` → el Request que busca `match`/`matchAll`/`delete`/`keys` (paso 1–2 de
  // cada uno): `null` si es un Request con método distinto de GET sin `ignoreMethod` (el
  // resultado es vacío), o un `new Request(request)` para una cadena.
  const NO_MATCH = {};
  function queryRequest(request, opts) {
    if (native.isRequest(request)) {
      if (native.method(request) !== "GET" && !opts.ignoreMethod) return NO_MATCH;
      return request;
    }
    return new (fetchIntrinsics().Request)(String(request));
  }

  // Lanza el error que devolvió `native.batch` ("Nombre: mensaje").
  function batch(id, ops) {
    const r = native.batch(id, ops);
    if (typeof r === "number") return r;
    const i = r.indexOf(": ");
    const name = r.slice(0, i);
    const msg = r.slice(i + 2);
    if (name === "TypeError") throw new TypeError(msg);
    throw err(name, msg);
  }

  // Lee el cuerpo entero de `response` (lo deja usado): `null` si es nulo.
  function readBody(response) {
    const f = fetchIntrinsics();
    const body = f.body ? f.body.call(response) : response.body;
    if (body === null) return P.resolve(null);
    return f.arrayBuffer.call(response).then((buf) => new Uint8Array(buf));
  }

  // --- Cache (§5.4) ---
  const CACHE_ID = new WeakMap();
  function idOf(cache) {
    const id = CACHE_ID.get(cache);
    if (id === undefined) throw new TypeError("Illegal invocation");
    return id;
  }
  function newCache(id) {
    const c = new Cache(TOKEN);
    CACHE_ID.set(c, id);
    return c;
  }

  // `matchAll(request, options)` (§5.4.2).
  function matchAllSteps(id, request, options) {
    const opts = queryOptions(options, false);
    let r = null;
    if (request !== undefined) {
      r = queryRequest(request, opts);
      if (r === NO_MATCH) return P.resolve(Object.freeze([]));
    }
    return inTask(() => Object.freeze(native.matchAll(id, r, opts.ignoreSearch, opts.ignoreMethod, opts.ignoreVary)));
  }

  // `match(request, options)` (§5.4.1): el primero de `matchAll`, o `undefined`.
  function matchSteps(id, request, options) {
    return matchAllSteps(id, request, options).then((list) => (list.length ? list[0] : undefined));
  }

  // `addAll(requests)` (§5.4.4).
  function addAllSteps(id, requests) {
    const f = fetchIntrinsics();
    // 1–3: los Request con método distinto de GET rechazan antes de nada.
    for (const request of requests) {
      if (native.isRequest(request) && native.method(request) !== "GET") {
        throw new TypeError("Failed to execute 'addAll' on 'Cache': Request method must be 'GET'.");
      }
    }
    // 4–5: `new Request(request)` y el esquema; luego un fetch por petición.
    const requestList = [];
    for (const request of requests) {
      const r = new f.Request(native.isRequest(request) ? request : String(request));
      const problem = native.check(0, r, null);
      if (problem !== null) throw new TypeError(`Failed to execute 'addAll' on 'Cache': ${problem}.`);
      requestList.push(r);
    }
    const responsePromises = requestList.map((r) =>
      f.fetch.call(global, r).then((response) => {
        // processResponse: tipo error, estado no ok, 206 o `Vary: *` → TypeError.
        const problem = native.check(2, null, response);
        if (problem !== null) throw new TypeError(`Failed to execute 'addAll' on 'Cache': ${problem}.`);
        // processResponseEndOfBody: la respuesta se guarda con el cuerpo completo.
        return readBody(response).then((bytes) => [response, bytes]);
      })
    );
    // 6–7: cuando llegan todas, un único lote de "put" (todo o nada).
    return P.all(responsePromises).then((pairs) =>
      inTask(() => {
        batch(
          id,
          pairs.map(([response, bytes], i) => ["put", requestList[i], response, bytes])
        );
        return undefined;
      })
    );
  }

  // `put(request, response)` (§5.4.5).
  function putSteps(id, request, response) {
    // Conversión de los argumentos (WebIDL): RequestInfo y luego Response.
    const input = native.isRequest(request) ? request : String(request);
    if (!native.isResponse(response)) {
      throw new TypeError("Failed to execute 'put' on 'Cache': parameter 2 is not of type 'Response'.");
    }
    // 1–2.
    const r = typeof input === "string" ? new (fetchIntrinsics().Request)(input) : input;
    // 3: esquema y método.
    let problem = native.check(0, r, null);
    if (problem !== null) throw new TypeError(`Failed to execute 'put' on 'Cache': ${problem}.`);
    // 5–6: 206 y `Vary: *`.
    problem = native.check(1, null, response);
    if (problem !== null) throw new TypeError(`Failed to execute 'put' on 'Cache': ${problem}.`);
    // 7: cuerpo usado o bloqueado.
    const f = fetchIntrinsics();
    const body = f.body ? f.body.call(response) : response.body;
    const used = f.bodyUsed ? f.bodyUsed.call(response) : response.bodyUsed;
    const locked = body !== null && (f.locked ? f.locked.call(body) : body.locked);
    if (used || locked) {
      throw new TypeError("Failed to execute 'put' on 'Cache': Response body is already used");
    }
    // 8–10: se lee el cuerpo entero (la respuesta queda usada) y se guarda.
    return readBody(response).then((bytes) =>
      inTask(() => {
        batch(id, [["put", r, response, bytes]]);
        return undefined;
      })
    );
  }

  // `delete(request, options)` (§5.4.6).
  function deleteSteps(id, request, options) {
    const opts = queryOptions(options, false);
    const r = queryRequest(request, opts);
    if (r === NO_MATCH) return P.resolve(false);
    return inTask(() => batch(id, [["delete", r, null, null, opts.ignoreSearch, opts.ignoreMethod, opts.ignoreVary]]) > 0);
  }

  // `keys(request, options)` (§5.4.7).
  function keysSteps(id, request, options) {
    const opts = queryOptions(options, false);
    let r = null;
    if (request !== undefined) {
      r = queryRequest(request, opts);
      if (r === NO_MATCH) return P.resolve(Object.freeze([]));
    }
    return inTask(() => Object.freeze(native.keys(id, r, opts.ignoreSearch, opts.ignoreMethod, opts.ignoreVary)));
  }

  class Cache {
    constructor(token) {
      illegal(token);
    }
    match(request, options) {
      return wrap(() => {
        const id = idOf(this);
        need(1, arguments, "Cache", "match");
        return matchSteps(id, request, options);
      });
    }
    matchAll(request, options) {
      return wrap(() => matchAllSteps(idOf(this), request, options));
    }
    add(request) {
      return wrap(() => {
        const id = idOf(this);
        need(1, arguments, "Cache", "add");
        return addAllSteps(id, [native.isRequest(request) ? request : String(request)]).then(() => undefined);
      });
    }
    addAll(requests) {
      return wrap(() => {
        const id = idOf(this);
        need(1, arguments, "Cache", "addAll");
        // `sequence<RequestInfo>` (WebIDL §3.2.27).
        if (requests === null || (typeof requests !== "object" && typeof requests !== "function")) {
          throw new TypeError("Failed to execute 'addAll' on 'Cache': The provided value cannot be converted to a sequence.");
        }
        const it = requests[Symbol.iterator];
        if (typeof it !== "function") {
          throw new TypeError("Failed to execute 'addAll' on 'Cache': The object must have a callable @@iterator property.");
        }
        const list = [];
        for (const v of { [Symbol.iterator]: () => it.call(requests) }) {
          list.push(native.isRequest(v) ? v : String(v));
        }
        return addAllSteps(id, list);
      });
    }
    put(request, response) {
      return wrap(() => {
        const id = idOf(this);
        need(2, arguments, "Cache", "put");
        return putSteps(id, request, response);
      });
    }
    delete(request, options) {
      return wrap(() => {
        const id = idOf(this);
        need(1, arguments, "Cache", "delete");
        return deleteSteps(id, request, options);
      });
    }
    keys(request, options) {
      return wrap(() => keysSteps(idOf(this), request, options));
    }
  }
  enumerable(Cache.prototype);
  tag(Cache.prototype, "Cache");

  // --- CacheStorage (§5.5) ---
  const STORAGES = new WeakSet();
  function checkStorage(s) {
    if (!STORAGES.has(s)) throw new TypeError("Illegal invocation");
    // Un origen opaco no tiene almacén.
    if (native.opaque()) throw err("SecurityError", "Cache storage is disabled for opaque origins.");
  }

  class CacheStorage {
    constructor(token) {
      illegal(token);
    }
    // §5.5.1.
    match(request, options) {
      return wrap(() => {
        checkStorage(this);
        need(1, arguments, "CacheStorage", "match");
        const input = native.isRequest(request) ? request : String(request);
        const opts = queryOptions(options, true);
        if (opts.cacheName !== undefined) {
          return inTask(() => native.lookup(opts.cacheName)).then((id) =>
            id < 0 ? undefined : matchSteps(id, input, opts)
          );
        }
        // Una cadena de promesas sobre las cachés, en orden de creación.
        let promise = P.resolve(undefined);
        for (const [, id] of native.entries()) {
          promise = promise.then((response) => (response !== undefined ? response : matchSteps(id, input, opts)));
        }
        return promise;
      });
    }
    // §5.5.2.
    has(cacheName) {
      return wrap(() => {
        checkStorage(this);
        need(1, arguments, "CacheStorage", "has");
        const name = String(cacheName);
        return inTask(() => native.lookup(name) >= 0);
      });
    }
    // §5.5.3: un objeto `Cache` nuevo en cada llamada.
    open(cacheName) {
      return wrap(() => {
        checkStorage(this);
        need(1, arguments, "CacheStorage", "open");
        const name = String(cacheName);
        return inTask(() => newCache(native.open(name)));
      });
    }
    // §5.5.4.
    delete(cacheName) {
      return wrap(() => {
        checkStorage(this);
        need(1, arguments, "CacheStorage", "delete");
        const name = String(cacheName);
        return inTask(() => native.remove(name));
      });
    }
    // §5.5.5: los nombres en orden de creación.
    keys() {
      return wrap(() => {
        checkStorage(this);
        return inTask(() => native.names());
      });
    }
  }
  enumerable(CacheStorage.prototype);
  tag(CacheStorage.prototype, "CacheStorage");

  const storage = new CacheStorage(TOKEN);
  STORAGES.add(storage);
  define(global, "Cache", Cache);
  define(global, "CacheStorage", CacheStorage);
  // `[SecureContext, SameObject] readonly attribute CacheStorage caches` (§5.3.1).
  Object.defineProperty(global, "caches", {
    get() {
      return storage;
    },
    enumerable: true,
    configurable: true,
  });
});
