// Service Workers (https://w3c.github.io/ServiceWorker/), primera parte: ServiceWorker (§3.1),
// ServiceWorkerRegistration (§3.2), ServiceWorkerContainer (§3.4, navigator.serviceWorker),
// NavigationPreloadManager (§3.6) y, en el global del service worker, ExtendableEvent (§4.4),
// FetchEvent (§4.5), ExtendableMessageEvent (§4.6), Client/WindowClient (§4.2) y Clients
// (§4.3). El estado (registros, workers, clientes, trabajos) vive en Rust
// (bf-web/src/workers/service.rs): `native` lo consulta y los objetos de este realm se
// actualizan con las tareas que manda Rust a través de las funciones que devuelve este archivo.
// (E3)
(function (global, native, mode) {
  "use strict";
  const TOKEN = {};
  const P = Promise;
  const DOMExceptionCtor = global.DOMException;
  const EventCtor = global.Event;
  const MessageEventCtor = global.MessageEvent;
  const ET = global.EventTarget;
  const err = (name, msg) => new DOMExceptionCtor(msg || name, name);
  const parse = (s) => (s === null || s === undefined ? null : JSON.parse(s));

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
  function finish(cls, name) {
    enumerable(cls.prototype);
    tag(cls.prototype, name);
  }

  // --- Event handlers (HTML §8.1.8) ---
  function defineHandlers(proto, types, onSet) {
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
          if (onSet) onSet(this, type, value);
        },
      });
    }
  }

  // Los diccionarios de WebIDL: `undefined`/`null` es el vacío; otro primitivo, TypeError.
  function dict(v, what) {
    if (v === undefined || v === null) return {};
    if (typeof v !== "object" && typeof v !== "function") throw new TypeError(what + ": The provided value is not of type 'object'.");
    return v;
  }
  function enumValue(v, values, what) {
    const s = String(v);
    if (!values.includes(s)) throw new TypeError(what + ": The provided value '" + s + "' is not a valid enum value.");
    return s;
  }

  // --- ServiceWorker (§3.1) ---
  const W = new WeakMap();
  const workers = new Map();
  function sw(o) {
    const s = W.get(o);
    if (!s) throw new TypeError("Illegal invocation");
    return s;
  }
  class ServiceWorker extends ET {
    constructor(token, snap) {
      illegal(token);
      super();
      W.set(this, { id: snap.id, scriptURL: snap.scriptURL, state: snap.state });
    }
    get scriptURL() {
      return sw(this).scriptURL;
    }
    get state() {
      return sw(this).state;
    }
    postMessage(message, transfer) {
      const s = sw(this);
      if (arguments.length < 1) throw new TypeError("Failed to execute 'postMessage' on 'ServiceWorker': 1 argument required, but only 0 present.");
      native.postToWorker(s.id, message, transfer);
    }
  }
  finish(ServiceWorker, "ServiceWorker");
  defineHandlers(ServiceWorker.prototype, ["statechange", "error"]);
  // "Get the service worker object" (§3.1): uno por worker en este realm.
  function getWorker(snap) {
    if (!snap) return null;
    let w = workers.get(snap.id);
    if (!w) {
      w = new ServiceWorker(TOKEN, snap);
      workers.set(snap.id, w);
    }
    return w;
  }

  // --- NavigationPreloadManager (§3.6) ---
  const NP = new WeakMap();
  class NavigationPreloadManager {
    constructor(token, regId) {
      illegal(token);
      NP.set(this, { reg: regId, enabled: false, header: "true" });
    }
    enable() {
      const s = NP.get(this);
      if (!s) return P.reject(new TypeError("Illegal invocation"));
      if (!native.hasActive(s.reg)) return P.reject(err("InvalidStateError", "The registration does not have an active worker."));
      s.enabled = true;
      return P.resolve(undefined);
    }
    disable() {
      const s = NP.get(this);
      if (!s) return P.reject(new TypeError("Illegal invocation"));
      if (!native.hasActive(s.reg)) return P.reject(err("InvalidStateError", "The registration does not have an active worker."));
      s.enabled = false;
      return P.resolve(undefined);
    }
    setHeaderValue(value) {
      const s = NP.get(this);
      if (!s) return P.reject(new TypeError("Illegal invocation"));
      const v = String(value);
      if (/[\0\r\n]/.test(v) || /^[ \t]|[ \t]$/.test(v) || /[^\x00-\xff]/.test(v)) return P.reject(new TypeError("Failed to execute 'setHeaderValue': The value is not a valid header value."));
      if (!native.hasActive(s.reg)) return P.reject(err("InvalidStateError", "The registration does not have an active worker."));
      s.header = v;
      return P.resolve(undefined);
    }
    getState() {
      const s = NP.get(this);
      if (!s) return P.reject(new TypeError("Illegal invocation"));
      return P.resolve({ enabled: s.enabled, headerValue: s.header });
    }
  }
  finish(NavigationPreloadManager, "NavigationPreloadManager");

  // --- Trabajos: promesas que resuelve Rust ---
  const jobs = new Map();
  let nextJob = 1;
  function toError(name, msg) {
    return name === "TypeError" ? new TypeError(msg) : err(name, msg);
  }
  function job(start) {
    return new P((resolve, reject) => {
      const pid = nextJob++;
      jobs.set(pid, { resolve, reject });
      const e = start(pid);
      if (e) {
        jobs.delete(pid);
        reject(toError(e[0], e[1]));
      }
    });
  }

  // --- ServiceWorkerRegistration (§3.2) ---
  const R = new WeakMap();
  const regs = new Map();
  function reg(o) {
    const r = R.get(o);
    if (!r) throw new TypeError("Illegal invocation");
    return r;
  }
  class ServiceWorkerRegistration extends ET {
    constructor(token, snap) {
      illegal(token);
      super();
      R.set(this, {
        id: snap.id,
        scope: snap.scope,
        updateViaCache: snap.updateViaCache,
        installing: getWorker(snap.installing),
        waiting: getWorker(snap.waiting),
        active: getWorker(snap.active),
        navigationPreload: null,
      });
    }
    get installing() {
      return reg(this).installing;
    }
    get waiting() {
      return reg(this).waiting;
    }
    get active() {
      return reg(this).active;
    }
    get navigationPreload() {
      const r = reg(this);
      if (!r.navigationPreload) r.navigationPreload = new NavigationPreloadManager(TOKEN, r.id);
      return r.navigationPreload;
    }
    get scope() {
      return reg(this).scope;
    }
    get updateViaCache() {
      return reg(this).updateViaCache;
    }
    update() {
      let r;
      try {
        r = reg(this);
      } catch (e) {
        return P.reject(e);
      }
      return job((pid) => native.update(r.id, pid));
    }
    unregister() {
      let r;
      try {
        r = reg(this);
      } catch (e) {
        return P.reject(e);
      }
      return job((pid) => native.unregister(r.id, pid));
    }
  }
  finish(ServiceWorkerRegistration, "ServiceWorkerRegistration");
  defineHandlers(ServiceWorkerRegistration.prototype, ["updatefound"]);
  // "Get the service worker registration object" (§3.2): uno por registro en este realm.
  function getReg(snap) {
    if (!snap) return null;
    let r = regs.get(snap.id);
    if (!r) {
      r = new ServiceWorkerRegistration(TOKEN, snap);
      regs.set(snap.id, r);
    }
    return r;
  }

  // --- ServiceWorkerContainer (§3.4) ---
  const C = new WeakMap();
  let containerObj = null;
  let controllerSnap;
  let controllerKnown = false;
  function cont(o) {
    const c = C.get(o);
    if (!c) throw new TypeError("Illegal invocation");
    return c;
  }
  function currentController() {
    if (!controllerKnown) {
      controllerSnap = parse(native.controller());
      controllerKnown = true;
    }
    return controllerSnap;
  }
  // El *client message queue* (§3.4.1): se habilita con `startMessages()`, al poner
  // `onmessage` o cuando el documento terminó de parsearse.
  function enableMessages(c) {
    if (c.enabled) return;
    c.enabled = true;
    const q = c.queue;
    c.queue = [];
    for (const f of q) f();
  }
  class ServiceWorkerContainer extends ET {
    constructor(token) {
      illegal(token);
      super();
      C.set(this, { ready: null, readyResolve: null, enabled: false, queue: [] });
    }
    get controller() {
      cont(this);
      return getWorker(currentController());
    }
    get ready() {
      const c = cont(this);
      if (!c.ready) {
        c.ready = new P((r) => {
          c.readyResolve = r;
        });
        const s = native.ready();
        if (s !== null) resolveReady(c, parse(s));
      }
      return c.ready;
    }
    register(scriptURL, options) {
      let url, scope, type, uvc;
      try {
        cont(this);
        if (arguments.length < 1) throw new TypeError("Failed to execute 'register' on 'ServiceWorkerContainer': 1 argument required, but only 0 present.");
        url = String(scriptURL);
        const o = dict(options, "Failed to execute 'register' on 'ServiceWorkerContainer'");
        scope = o.scope === undefined ? null : String(o.scope);
        type = o.type === undefined ? "classic" : enumValue(o.type, ["classic", "module"], "Failed to execute 'register' on 'ServiceWorkerContainer'");
        uvc = o.updateViaCache === undefined ? "imports" : enumValue(o.updateViaCache, ["imports", "all", "none"], "Failed to execute 'register' on 'ServiceWorkerContainer'");
      } catch (e) {
        return P.reject(e);
      }
      return job((pid) => native.register(url, scope, type, uvc, pid));
    }
    getRegistration(clientURL) {
      try {
        cont(this);
        const u = clientURL === undefined ? "" : String(clientURL);
        const r = native.getRegistration(u);
        if (Array.isArray(r)) return P.reject(toError(r[0], r[1]));
        return P.resolve(r === null ? undefined : getReg(parse(r)));
      } catch (e) {
        return P.reject(e);
      }
    }
    getRegistrations() {
      try {
        cont(this);
        return P.resolve(parse(native.getRegistrations()).map(getReg));
      } catch (e) {
        return P.reject(e);
      }
    }
    startMessages() {
      enableMessages(cont(this));
    }
  }
  finish(ServiceWorkerContainer, "ServiceWorkerContainer");
  defineHandlers(ServiceWorkerContainer.prototype, ["controllerchange", "message", "messageerror"], (o, type) => {
    if (type === "message") enableMessages(cont(o));
  });
  function resolveReady(c, snap) {
    if (c.readyResolve) {
      const r = c.readyResolve;
      c.readyResolve = null;
      r(getReg(snap));
    }
  }
  function container() {
    if (!containerObj) {
      containerObj = new ServiceWorkerContainer(TOKEN);
      native.used();
    }
    return containerObj;
  }
  // Un mensaje para el contenedor: en cola hasta que se habilite.
  function toContainer(makeEvent) {
    const o = container();
    const c = C.get(o);
    const d = global.document;
    if (!c.enabled && d && d.readyState !== "loading") enableMessages(c);
    const run = () => native.fire(o, makeEvent());
    if (c.enabled) run();
    else c.queue.push(run);
  }

  const hooks = {
    job(pid, kind, payload) {
      const j = jobs.get(pid);
      if (!j) return;
      jobs.delete(pid);
      if (kind === "reg") j.resolve(getReg(parse(payload)));
      else if (kind === "bool") j.resolve(payload === "true");
      else {
        const e = parse(payload);
        j.reject(toError(e.name, e.message));
      }
    },
    regState(id, which, snap) {
      const r = regs.get(id);
      if (r) R.get(r)[which] = getWorker(parse(snap));
    },
    workerState(id, state) {
      const w = workers.get(id);
      if (!w) return;
      const s = W.get(w);
      if (s.state === state) return;
      s.state = state;
      native.fire(w, new EventCtor("statechange"));
    },
    updateFound(id) {
      const r = regs.get(id);
      if (r) native.fire(r, new EventCtor("updatefound"));
    },
    controller(snap) {
      controllerSnap = parse(snap);
      controllerKnown = true;
      if (containerObj) native.fire(containerObj, new EventCtor("controllerchange"));
    },
    ready(snap) {
      if (containerObj) resolveReady(C.get(containerObj), parse(snap));
    },
    message(source, origin, data, ports) {
      toContainer(() => new MessageEventCtor("message", { data, origin, source: getWorker(parse(source)), ports }));
    },
    messageError(source, origin) {
      toContainer(() => new MessageEventCtor("messageerror", { data: null, origin, source: getWorker(parse(source)) }));
    },
  };

  const exposed = [
    ["ServiceWorker", ServiceWorker],
    ["ServiceWorkerRegistration", ServiceWorkerRegistration],
    ["ServiceWorkerContainer", ServiceWorkerContainer],
    ["NavigationPreloadManager", NavigationPreloadManager],
  ];

  if (mode === "window" && typeof global.Navigator === "function") {
    const Nav = global.Navigator;
    Object.defineProperty(Nav.prototype, "serviceWorker", {
      configurable: true,
      enumerable: true,
      get() {
        if (!(this instanceof Nav)) throw new TypeError("Illegal invocation");
        return container();
      },
    });
  }

  if (mode === "service") {
    // --- ExtendableEvent (§4.4) ---
    const E = new WeakMap();
    function ext(o) {
      const s = E.get(o);
      if (!s) throw new TypeError("Illegal invocation");
      return s;
    }
    // "Add lifetime promise" (§4.4.1): el evento debe estar activo (despachándose o con
    // promesas pendientes).
    function addLifetime(ev, s, f) {
      if (s.pending === 0 && ev.eventPhase === 0) throw err("InvalidStateError", "The event handler is already finished and no extend lifetime promises are outstanding.");
      s.pending++;
      P.resolve(f).then(
        () => settled(s, false),
        () => settled(s, true),
      );
    }
    function settled(s, rejected) {
      queueMicrotask(() => {
        if (rejected) s.rejected = true;
        s.pending--;
        if (s.pending === 0 && s.done) {
          const d = s.done;
          s.done = null;
          d();
        }
      });
    }
    // Espera a que el evento deje de estar activo; `true` si ninguna promesa se rechazó.
    function whenSettled(s) {
      return new P((res) => {
        if (s.pending === 0) res(!s.rejected);
        else s.done = () => res(!s.rejected);
      });
    }
    class ExtendableEvent extends EventCtor {
      constructor(type, init) {
        if (arguments.length < 1) throw new TypeError("Failed to construct 'ExtendableEvent': 1 argument required, but only 0 present.");
        super(type, init);
        E.set(this, { pending: 0, rejected: false, done: null });
      }
      waitUntil(f) {
        const s = ext(this);
        if (arguments.length < 1) throw new TypeError("Failed to execute 'waitUntil' on 'ExtendableEvent': 1 argument required, but only 0 present.");
        if (!this.isTrusted) throw err("InvalidStateError", "The event is not trusted.");
        addLifetime(this, s, f);
      }
    }
    finish(ExtendableEvent, "ExtendableEvent");

    // --- FetchEvent (§4.5) ---
    const F = new WeakMap();
    function fe(o) {
      const s = F.get(o);
      if (!s) throw new TypeError("Illegal invocation");
      return s;
    }
    class FetchEvent extends ExtendableEvent {
      constructor(type, init) {
        if (arguments.length < 2) throw new TypeError("Failed to construct 'FetchEvent': 2 arguments required, but only " + arguments.length + " present.");
        const o = dict(init, "Failed to construct 'FetchEvent'");
        // Los miembros de FetchEventInit en orden lexicográfico (WebIDL §3.2.17).
        const clientId = o.clientId === undefined ? "" : String(o.clientId);
        const handled = o.handled === undefined ? null : P.resolve(o.handled);
        const preload = o.preloadResponse === undefined ? null : P.resolve(o.preloadResponse);
        const replacesClientId = o.replacesClientId === undefined ? "" : String(o.replacesClientId);
        const request = o.request;
        if (request === undefined) throw new TypeError("Failed to construct 'FetchEvent': required member request is undefined.");
        if (!native.isRequest(request)) throw new TypeError("Failed to construct 'FetchEvent': member request is not of type Request.");
        const resultingClientId = o.resultingClientId === undefined ? "" : String(o.resultingClientId);
        super(type, init);
        const s = { request, clientId, resultingClientId, replacesClientId, handled, preload, entered: false, response: null, handledResolve: null, handledReject: null };
        if (!s.handled) {
          s.handled = new P((a, b) => {
            s.handledResolve = a;
            s.handledReject = b;
          });
          s.handled.catch(() => {});
        }
        if (!s.preload) s.preload = P.resolve(undefined);
        F.set(this, s);
      }
      get request() {
        return fe(this).request;
      }
      get clientId() {
        return fe(this).clientId;
      }
      get resultingClientId() {
        return fe(this).resultingClientId;
      }
      get replacesClientId() {
        return fe(this).replacesClientId;
      }
      get handled() {
        return fe(this).handled;
      }
      get preloadResponse() {
        return fe(this).preload;
      }
      // §4.5.6
      respondWith(r) {
        const s = fe(this);
        if (arguments.length < 1) throw new TypeError("Failed to execute 'respondWith' on 'FetchEvent': 1 argument required, but only 0 present.");
        if (this.eventPhase === 0) throw err("InvalidStateError", "The event handler is already finished.");
        if (s.entered) throw err("InvalidStateError", "The event has already been responded to.");
        addLifetime(this, ext(this), r);
        this.stopImmediatePropagation();
        s.entered = true;
        s.response = P.resolve(r);
      }
    }
    finish(FetchEvent, "FetchEvent");

    // --- ExtendableMessageEvent (§4.6) ---
    const M = new WeakMap();
    function me(o) {
      const s = M.get(o);
      if (!s) throw new TypeError("Illegal invocation");
      return s;
    }
    class ExtendableMessageEvent extends ExtendableEvent {
      constructor(type, init) {
        if (arguments.length < 1) throw new TypeError("Failed to construct 'ExtendableMessageEvent': 1 argument required, but only 0 present.");
        const o = dict(init, "Failed to construct 'ExtendableMessageEvent'");
        // ExtendableMessageEventInit en orden lexicográfico: data, lastEventId, origin,
        // ports, source.
        const data = o.data === undefined ? null : o.data;
        const lastEventId = o.lastEventId === undefined ? "" : String(o.lastEventId);
        const origin = o.origin === undefined ? "" : String(o.origin);
        let ports = [];
        if (o.ports !== undefined) {
          if (o.ports === null || typeof o.ports !== "object" || typeof o.ports[Symbol.iterator] !== "function") throw new TypeError("Failed to construct 'ExtendableMessageEvent': The provided value cannot be converted to a sequence.");
          ports = Array.from(o.ports);
          for (const p of ports) if (!native.isMessagePort(p)) throw new TypeError("Failed to construct 'ExtendableMessageEvent': Failed to convert value to 'MessagePort'.");
        }
        let source = null;
        if (o.source !== undefined && o.source !== null) {
          source = o.source;
          if (!(source instanceof Client || source instanceof ServiceWorker || native.isMessagePort(source))) throw new TypeError("Failed to construct 'ExtendableMessageEvent': The provided value is not of type '(Client or ServiceWorker or MessagePort)'.");
        }
        super(type, init);
        M.set(this, { data, lastEventId, origin, ports: Object.freeze(ports), source });
      }
      get data() {
        return me(this).data;
      }
      get origin() {
        return me(this).origin;
      }
      get lastEventId() {
        return me(this).lastEventId;
      }
      get source() {
        return me(this).source;
      }
      get ports() {
        return me(this).ports;
      }
    }
    finish(ExtendableMessageEvent, "ExtendableMessageEvent");

    // --- Client / WindowClient (§4.2) ---
    const CL = new WeakMap();
    function cl(o) {
      const s = CL.get(o);
      if (!s) throw new TypeError("Illegal invocation");
      return s;
    }
    class Client {
      constructor(token, snap) {
        illegal(token);
        CL.set(this, snap);
      }
      get url() {
        return cl(this).url;
      }
      get frameType() {
        return cl(this).frameType;
      }
      get type() {
        return cl(this).type;
      }
      get id() {
        return cl(this).id;
      }
      postMessage(message, transfer) {
        const s = cl(this);
        if (arguments.length < 1) throw new TypeError("Failed to execute 'postMessage' on 'Client': 1 argument required, but only 0 present.");
        native.postToClient(s.id, message, transfer);
      }
    }
    finish(Client, "Client");
    const NO_ANCESTORS = Object.freeze([]);
    class WindowClient extends Client {
      constructor(token, snap) {
        illegal(token);
        super(token, snap);
      }
      get visibilityState() {
        cl(this);
        return "visible";
      }
      get focused() {
        cl(this);
        return false;
      }
      get ancestorOrigins() {
        cl(this);
        return NO_ANCESTORS;
      }
      focus() {
        try {
          cl(this);
        } catch (e) {
          return P.reject(e);
        }
        return P.reject(err("InvalidAccessError", "Not allowed to focus a window."));
      }
      navigate(url) {
        try {
          cl(this);
          String(url);
        } catch (e) {
          return P.reject(e);
        }
        return P.reject(new TypeError("Failed to execute 'navigate' on 'WindowClient': navigation is not supported yet."));
      }
    }
    finish(WindowClient, "WindowClient");
    function makeClient(snap) {
      return snap.type === "window" ? new WindowClient(TOKEN, snap) : new Client(TOKEN, snap);
    }

    // --- Clients (§4.3) ---
    const CS = new WeakSet();
    function cs(o) {
      if (!CS.has(o)) throw new TypeError("Illegal invocation");
    }
    class Clients {
      constructor(token) {
        illegal(token);
        CS.add(this);
      }
      get(id) {
        try {
          cs(this);
          if (arguments.length < 1) throw new TypeError("Failed to execute 'get' on 'Clients': 1 argument required, but only 0 present.");
          const s = parse(native.getClient(String(id)));
          return P.resolve(s ? makeClient(s) : undefined);
        } catch (e) {
          return P.reject(e);
        }
      }
      matchAll(options) {
        try {
          cs(this);
          const o = dict(options, "Failed to execute 'matchAll' on 'Clients'");
          const include = Boolean(o.includeUncontrolled);
          const type = o.type === undefined ? "window" : enumValue(o.type, ["window", "worker", "sharedworker", "all"], "Failed to execute 'matchAll' on 'Clients'");
          return P.resolve(parse(native.matchAll(include, type)).map(makeClient));
        } catch (e) {
          return P.reject(e);
        }
      }
      openWindow(url) {
        try {
          cs(this);
          String(url);
        } catch (e) {
          return P.reject(e);
        }
        return P.reject(err("InvalidAccessError", "Not allowed to open a window."));
      }
      claim() {
        try {
          cs(this);
        } catch (e) {
          return P.reject(e);
        }
        const e = native.claim();
        return e ? P.reject(err(e, "Only the active worker can claim clients.")) : P.resolve(undefined);
      }
    }
    finish(Clients, "Clients");

    // --- El global (§4.1) ---
    const SWGS = global.ServiceWorkerGlobalScope;
    const info = parse(native.selfInfo()) || { reg: null, worker: null };
    let clientsObj = null;
    const accessor = (name, get) => Object.defineProperty(SWGS.prototype, name, { configurable: true, enumerable: true, get });
    accessor("clients", function () {
      if (!clientsObj) clientsObj = new Clients(TOKEN);
      return clientsObj;
    });
    accessor("registration", function () {
      return getReg(info.reg);
    });
    accessor("serviceWorker", function () {
      return getWorker(info.worker);
    });
    Object.defineProperty(SWGS.prototype, "skipWaiting", {
      configurable: true,
      enumerable: true,
      writable: true,
      value: function skipWaiting() {
        return new P((resolve) => {
          native.skipWaiting();
          resolve(undefined);
        });
      },
    });

    hooks.lifecycle = function (token, kind) {
      const ev = new ExtendableEvent(kind);
      native.fire(global, ev);
      whenSettled(E.get(ev)).then((ok) => native.lifecycleDone(token, kind === "install" ? ok : true));
    };
    // "Handle Fetch" (§4.6), del lado del service worker.
    hooks.fetch = function (fid, request, clientId) {
      const ev = new FetchEvent("fetch", { request, clientId, cancelable: true });
      const s = F.get(ev);
      native.fire(global, ev);
      if (!s.entered) {
        native.fetchDone(fid, ev.defaultPrevented ? "error" : "fallback", null, null);
        if (s.handledReject) s.handledReject(err("NetworkError", "The event was not handled."));
        return;
      }
      s.response
        .then((r) => {
          if (!native.isResponse(r)) throw new TypeError("The value passed to respondWith() is not a Response.");
          if (r.bodyUsed || (r.body !== null && r.body.locked)) throw new TypeError("The Response body was already used.");
          if (r.body === null) return native.fetchDone(fid, "response", r, null);
          return r.arrayBuffer().then((b) => native.fetchDone(fid, "response", r, b));
        })
        .then(
          () => {
            if (s.handledResolve) s.handledResolve(undefined);
          },
          () => {
            native.fetchDone(fid, "error", null, null);
            if (s.handledReject) s.handledReject(err("NetworkError", "The response was not usable."));
          },
        );
    };
    hooks.swMessage = function (client, origin, data, ports) {
      const c = parse(client);
      const ev = new ExtendableMessageEvent("message", { data, origin, source: c ? makeClient(c) : null, ports });
      native.fire(global, ev);
    };
    hooks.swMessageError = function (client, origin) {
      const c = parse(client);
      const ev = new ExtendableMessageEvent("messageerror", { data: null, origin, source: c ? makeClient(c) : null });
      native.fire(global, ev);
    };

    exposed.push(
      ["ExtendableEvent", ExtendableEvent],
      ["FetchEvent", FetchEvent],
      ["ExtendableMessageEvent", ExtendableMessageEvent],
      ["Client", Client],
      ["WindowClient", WindowClient],
      ["Clients", Clients],
    );
  }

  for (const [name, cls] of exposed) define(global, name, cls);
  return hooks;
});
