// Streams Standard §4: ReadableStream por defecto (sin byte streams), su reader y su
// controller. Los slots internos viven en WeakMaps de esta clausura; la función devuelve los
// helpers que usa el body mixin de Fetch (bf-web/src/net/streams.rs). (E2)
(function (global) {
  "use strict";
  const P = Promise;
  const then = Promise.prototype.then;
  const resolved = (v) => new P((r) => r(v));
  const rejected = (e) => new P((_, j) => j(e));
  const upon = (p, f, r) => {
    then.call(p, f, r);
  };
  const markHandled = (p) => {
    then.call(p, undefined, () => {});
  };
  const queueMicrotask = global.queueMicrotask;

  const S = new WeakMap(); // stream -> estado
  const R = new WeakMap(); // reader -> estado
  const C = new WeakMap(); // controller -> estado
  let creating = false;

  const define = (o, name, value) =>
    Object.defineProperty(o, name, { value, writable: true, configurable: true, enumerable: false });
  const tag = (o, t) =>
    Object.defineProperty(o, Symbol.toStringTag, { value: t, configurable: true });
  const enumerable = (proto, names) => {
    for (const n of names) {
      const d = Object.getOwnPropertyDescriptor(proto, n);
      d.enumerable = true;
      Object.defineProperty(proto, n, d);
    }
  };

  function stateOf(s) {
    const st = S.get(s);
    if (!st) throw new TypeError("Illegal invocation: not a ReadableStream");
    return st;
  }

  // --- Estrategias (§7) ---
  function extractHWM(strategy, def) {
    const hwm = strategy.highWaterMark;
    if (hwm === undefined) return def;
    const n = Number(hwm);
    if (Number.isNaN(n) || n < 0) throw new RangeError("Invalid highWaterMark");
    return n;
  }
  function extractSize(strategy) {
    const size = strategy.size;
    if (size === undefined) return () => 1;
    if (typeof size !== "function") throw new TypeError("size must be a function");
    return (chunk) => size(chunk);
  }

  // --- ReadableStream (§4.2) ---
  class ReadableStream {
    constructor(underlyingSource = undefined, strategy = {}) {
      if (creating) return;
      if (underlyingSource === null) throw new TypeError("underlyingSource must not be null");
      const src = underlyingSource === undefined ? {} : underlyingSource;
      if (typeof src !== "object" && typeof src !== "function")
        throw new TypeError("underlyingSource must be an object");
      if (strategy !== undefined && strategy !== null && typeof strategy !== "object")
        throw new TypeError("strategy must be an object");
      strategy = strategy == null ? {} : strategy;
      const cancel = src.cancel, pull = src.pull, start = src.start, type = src.type;
      if (cancel !== undefined && typeof cancel !== "function") throw new TypeError("cancel must be a function");
      if (pull !== undefined && typeof pull !== "function") throw new TypeError("pull must be a function");
      if (start !== undefined && typeof start !== "function") throw new TypeError("start must be a function");
      if (type !== undefined && String(type) !== "bytes") throw new TypeError("Invalid type");
      const size = extractSize(strategy);
      const hwm = extractHWM(strategy, 1);
      initStream(this);
      setUpController(
        this,
        start ? (c) => start.call(src, c) : () => undefined,
        pull ? (c) => { try { return resolved(pull.call(src, c)); } catch (e) { return rejected(e); } } : () => resolved(),
        cancel ? (r) => { try { return resolved(cancel.call(src, r)); } catch (e) { return rejected(e); } } : () => resolved(),
        hwm,
        size,
      );
    }
    get locked() {
      return stateOf(this).reader !== undefined;
    }
    cancel(reason = undefined) {
      let st;
      try { st = stateOf(this); } catch (e) { return rejected(e); }
      if (st.reader !== undefined) return rejected(new TypeError("Cannot cancel a locked stream"));
      return streamCancel(this, reason);
    }
    getReader(options = undefined) {
      stateOf(this);
      const mode = options == null ? undefined : options.mode;
      if (mode !== undefined) {
        if (String(mode) === "byob") throw new TypeError("byob readers are not supported");
        throw new TypeError("Invalid reader mode");
      }
      return new ReadableStreamDefaultReader(this);
    }
    tee() {
      stateOf(this);
      return streamTee(this);
    }
    values(options = undefined) {
      stateOf(this);
      const preventCancel = !!(options && options.preventCancel);
      return makeAsyncIterator(this, preventCancel);
    }
    pipeThrough(transform, options = {}) {
      throw new TypeError("pipeThrough is not supported yet");
    }
    pipeTo(dest, options = {}) {
      return rejected(new TypeError("pipeTo is not supported yet"));
    }
    static from(asyncIterable) {
      return streamFrom(asyncIterable);
    }
  }
  enumerable(ReadableStream.prototype, ["locked", "cancel", "getReader", "tee", "values", "pipeThrough", "pipeTo"]);
  define(ReadableStream.prototype, Symbol.asyncIterator, ReadableStream.prototype.values);
  tag(ReadableStream.prototype, "ReadableStream");

  function initStream(s) {
    S.set(s, {
      state: "readable",
      reader: undefined,
      storedError: undefined,
      disturbed: false,
      controller: undefined,
    });
  }

  function createStream(start, pull, cancel, hwm = 1, size = () => 1) {
    creating = true;
    let s;
    try { s = new ReadableStream(); } finally { creating = false; }
    initStream(s);
    setUpController(s, start, pull, cancel, hwm, size);
    return s;
  }

  function streamCancel(s, reason) {
    const st = S.get(s);
    st.disturbed = true;
    if (st.state === "closed") return resolved(undefined);
    if (st.state === "errored") return rejected(st.storedError);
    streamClose(s);
    const reader = st.reader;
    if (reader !== undefined) {
      // (byob: no hay read-into requests)
    }
    const cst = C.get(st.controller);
    cst.queue = [];
    cst.queueTotalSize = 0;
    const p = cst.cancelAlgorithm(reason);
    clearAlgorithms(cst);
    return new P((res, rej) => upon(p, () => res(undefined), rej));
  }

  function streamClose(s) {
    const st = S.get(s);
    st.state = "closed";
    const reader = st.reader;
    if (reader === undefined) return;
    const rst = R.get(reader);
    rst.resolveClosed(undefined);
    const reqs = rst.readRequests;
    rst.readRequests = [];
    for (const req of reqs) req.close();
  }

  function streamError(s, e) {
    const st = S.get(s);
    st.state = "errored";
    st.storedError = e;
    const reader = st.reader;
    if (reader === undefined) return;
    const rst = R.get(reader);
    rst.rejectClosed(e);
    markHandled(rst.closedPromise);
    const reqs = rst.readRequests;
    rst.readRequests = [];
    for (const req of reqs) req.error(e);
  }

  // --- ReadableStreamDefaultController (§4.6) ---
  class ReadableStreamDefaultController {
    constructor() {
      throw new TypeError("Illegal constructor");
    }
    get desiredSize() {
      return desiredSize(cstate(this));
    }
    close() {
      const cst = cstate(this);
      if (!canCloseOrEnqueue(cst)) throw new TypeError("The stream is not in a state that permits close");
      controllerClose(cst);
    }
    enqueue(chunk = undefined) {
      const cst = cstate(this);
      if (!canCloseOrEnqueue(cst)) throw new TypeError("The stream is not in a state that permits enqueue");
      controllerEnqueue(cst, chunk);
    }
    error(e = undefined) {
      controllerError(cstate(this), e);
    }
  }
  enumerable(ReadableStreamDefaultController.prototype, ["desiredSize", "close", "enqueue", "error"]);
  tag(ReadableStreamDefaultController.prototype, "ReadableStreamDefaultController");

  function cstate(c) {
    const st = C.get(c);
    if (!st) throw new TypeError("Illegal invocation: not a ReadableStreamDefaultController");
    return st;
  }

  function setUpController(stream, start, pull, cancel, hwm, size) {
    const controller = Object.create(ReadableStreamDefaultController.prototype);
    const cst = {
      stream,
      queue: [],
      queueTotalSize: 0,
      started: false,
      closeRequested: false,
      pullAgain: false,
      pulling: false,
      sizeAlgorithm: size,
      hwm,
      pullAlgorithm: pull,
      cancelAlgorithm: cancel,
    };
    C.set(controller, cst);
    S.get(stream).controller = controller;
    const startResult = start(controller);
    upon(
      resolved(startResult),
      () => {
        cst.started = true;
        callPullIfNeeded(cst);
      },
      (r) => controllerError(cst, r),
    );
  }

  function desiredSize(cst) {
    const state = S.get(cst.stream).state;
    if (state === "errored") return null;
    if (state === "closed") return 0;
    return cst.hwm - cst.queueTotalSize;
  }

  function canCloseOrEnqueue(cst) {
    return !cst.closeRequested && S.get(cst.stream).state === "readable";
  }

  function clearAlgorithms(cst) {
    cst.pullAlgorithm = undefined;
    cst.cancelAlgorithm = undefined;
    cst.sizeAlgorithm = undefined;
  }

  function controllerClose(cst) {
    if (!canCloseOrEnqueue(cst)) return;
    cst.closeRequested = true;
    if (cst.queue.length === 0) {
      clearAlgorithms(cst);
      streamClose(cst.stream);
    }
  }

  function controllerEnqueue(cst, chunk) {
    if (!canCloseOrEnqueue(cst)) return;
    const stream = cst.stream;
    const st = S.get(stream);
    const reader = st.reader;
    if (reader !== undefined && R.get(reader).readRequests.length > 0) {
      const req = R.get(reader).readRequests.shift();
      req.chunk(chunk);
    } else {
      let size;
      try {
        size = cst.sizeAlgorithm(chunk);
      } catch (e) {
        controllerError(cst, e);
        throw e;
      }
      size = Number(size);
      if (Number.isNaN(size) || size < 0 || size === Infinity) {
        const e = new RangeError("Invalid chunk size");
        controllerError(cst, e);
        throw e;
      }
      cst.queue.push({ value: chunk, size });
      cst.queueTotalSize += size;
    }
    callPullIfNeeded(cst);
  }

  function controllerError(cst, e) {
    const st = S.get(cst.stream);
    if (st.state !== "readable") return;
    cst.queue = [];
    cst.queueTotalSize = 0;
    clearAlgorithms(cst);
    streamError(cst.stream, e);
  }

  function shouldCallPull(cst) {
    if (!canCloseOrEnqueue(cst) || !cst.started) return false;
    const st = S.get(cst.stream);
    if (st.reader !== undefined && R.get(st.reader).readRequests.length > 0) return true;
    return desiredSize(cst) > 0;
  }

  function callPullIfNeeded(cst) {
    if (!shouldCallPull(cst)) return;
    if (cst.pulling) {
      cst.pullAgain = true;
      return;
    }
    cst.pulling = true;
    const controller = S.get(cst.stream).controller;
    upon(
      cst.pullAlgorithm(controller),
      () => {
        cst.pulling = false;
        if (cst.pullAgain) {
          cst.pullAgain = false;
          callPullIfNeeded(cst);
        }
      },
      (e) => controllerError(cst, e),
    );
  }

  // [[PullSteps]]
  function pullSteps(cst, req) {
    if (cst.queue.length > 0) {
      const { value, size } = cst.queue.shift();
      cst.queueTotalSize -= size;
      if (cst.queueTotalSize < 0) cst.queueTotalSize = 0;
      if (cst.closeRequested && cst.queue.length === 0) {
        clearAlgorithms(cst);
        streamClose(cst.stream);
      } else {
        callPullIfNeeded(cst);
      }
      req.chunk(value);
    } else {
      R.get(S.get(cst.stream).reader).readRequests.push(req);
      callPullIfNeeded(cst);
    }
  }

  // --- ReadableStreamDefaultReader (§4.4) ---
  class ReadableStreamDefaultReader {
    constructor(stream) {
      const st = S.get(stream);
      if (!st) throw new TypeError("ReadableStreamDefaultReader requires a ReadableStream");
      if (st.reader !== undefined) throw new TypeError("ReadableStream is locked");
      const rst = { stream, readRequests: [] };
      R.set(this, rst);
      genericInitialize(this, rst, stream);
    }
    get closed() {
      const rst = R.get(this);
      if (!rst) return rejected(new TypeError("Illegal invocation"));
      return rst.closedPromise;
    }
    cancel(reason = undefined) {
      const rst = R.get(this);
      if (!rst) return rejected(new TypeError("Illegal invocation"));
      if (rst.stream === undefined) return rejected(new TypeError("The reader has no stream"));
      return streamCancel(rst.stream, reason);
    }
    read() {
      const rst = R.get(this);
      if (!rst) return rejected(new TypeError("Illegal invocation"));
      if (rst.stream === undefined) return rejected(new TypeError("The reader has been released"));
      return new P((resolve, reject) => {
        readerRead(rst, {
          chunk: (v) => resolve({ value: v, done: false }),
          close: () => resolve({ value: undefined, done: true }),
          error: (e) => reject(e),
        });
      });
    }
    releaseLock() {
      const rst = R.get(this);
      if (!rst) throw new TypeError("Illegal invocation");
      if (rst.stream === undefined) return;
      genericRelease(rst);
      const e = new TypeError("Reader was released");
      const reqs = rst.readRequests;
      rst.readRequests = [];
      for (const r of reqs) r.error(e);
    }
  }
  enumerable(ReadableStreamDefaultReader.prototype, ["closed", "cancel", "read", "releaseLock"]);
  tag(ReadableStreamDefaultReader.prototype, "ReadableStreamDefaultReader");

  function genericInitialize(reader, rst, stream) {
    const st = S.get(stream);
    st.reader = reader;
    rst.closedPromise = new P((res, rej) => {
      rst.resolveClosed = res;
      rst.rejectClosed = rej;
    });
    if (st.state === "closed") rst.resolveClosed(undefined);
    else if (st.state === "errored") {
      rst.rejectClosed(st.storedError);
      markHandled(rst.closedPromise);
    }
  }

  function genericRelease(rst) {
    const st = S.get(rst.stream);
    const e = new TypeError("Reader was released");
    if (st.state === "readable") rst.rejectClosed(e);
    else {
      rst.closedPromise = rejected(e);
    }
    markHandled(rst.closedPromise);
    st.reader = undefined;
    rst.stream = undefined;
  }

  function readerRead(rst, req) {
    const st = S.get(rst.stream);
    st.disturbed = true;
    if (st.state === "closed") req.close();
    else if (st.state === "errored") req.error(st.storedError);
    else pullSteps(C.get(st.controller), req);
  }

  // --- tee (§4.9.1, ReadableStreamDefaultTee) ---
  function streamTee(stream) {
    const reader = new ReadableStreamDefaultReader(stream);
    const rst = R.get(reader);
    let reading = false, readAgain = false, canceled1 = false, canceled2 = false;
    let reason1, reason2, branch1, branch2, resolveCancel;
    const cancelPromise = new P((r) => (resolveCancel = r));
    const pull = () => {
      if (reading) {
        readAgain = true;
        return resolved();
      }
      reading = true;
      readerRead(rst, {
        chunk: (chunk) => {
          queueMicrotask(() => {
            readAgain = false;
            if (!canceled1) controllerEnqueue(C.get(S.get(branch1).controller), chunk);
            if (!canceled2) controllerEnqueue(C.get(S.get(branch2).controller), chunk);
            reading = false;
            if (readAgain) pull();
          });
        },
        close: () => {
          reading = false;
          if (!canceled1) controllerClose(C.get(S.get(branch1).controller));
          if (!canceled2) controllerClose(C.get(S.get(branch2).controller));
          if (!canceled1 || !canceled2) resolveCancel(undefined);
        },
        error: () => {
          reading = false;
        },
      });
      return resolved();
    };
    const cancel1 = (r) => {
      canceled1 = true;
      reason1 = r;
      if (canceled2) resolveCancel(streamCancel(stream, [reason1, reason2]));
      return cancelPromise;
    };
    const cancel2 = (r) => {
      canceled2 = true;
      reason2 = r;
      if (canceled1) resolveCancel(streamCancel(stream, [reason1, reason2]));
      return cancelPromise;
    };
    branch1 = createStream(() => undefined, pull, cancel1);
    branch2 = createStream(() => undefined, pull, cancel2);
    upon(rst.closedPromise, undefined, (r) => {
      controllerError(C.get(S.get(branch1).controller), r);
      controllerError(C.get(S.get(branch2).controller), r);
      if (!canceled1 || !canceled2) resolveCancel(undefined);
    });
    return [branch1, branch2];
  }

  // --- Iteración asíncrona (§4.2.5) ---
  const AsyncIteratorPrototype = Object.getPrototypeOf(Object.getPrototypeOf(async function* () {}).prototype);
  const ReadableStreamAsyncIteratorPrototype = Object.create(AsyncIteratorPrototype);
  const I = new WeakMap();
  function makeAsyncIterator(stream, preventCancel) {
    const reader = new ReadableStreamDefaultReader(stream);
    const it = Object.create(ReadableStreamAsyncIteratorPrototype);
    I.set(it, { reader, preventCancel, ongoing: undefined, finished: false });
    return it;
  }
  define(ReadableStreamAsyncIteratorPrototype, "next", function next() {
    const ist = I.get(this);
    if (!ist) return rejected(new TypeError("Illegal invocation"));
    const step = () => {
      if (ist.finished) return resolved({ value: undefined, done: true });
      const rst = R.get(ist.reader);
      if (rst.stream === undefined) return rejected(new TypeError("Iterator reader released"));
      return new P((resolve, reject) => {
        readerRead(rst, {
          chunk: (v) => resolve({ value: v, done: false }),
          close: () => {
            genericRelease(rst);
            ist.finished = true;
            resolve({ value: undefined, done: true });
          },
          error: (e) => {
            genericRelease(rst);
            ist.finished = true;
            reject(e);
          },
        });
      });
    };
    ist.ongoing = ist.ongoing ? then.call(ist.ongoing, step, step) : step();
    return ist.ongoing;
  });
  define(ReadableStreamAsyncIteratorPrototype, "return", function ret(value) {
    const ist = I.get(this);
    if (!ist) return rejected(new TypeError("Illegal invocation"));
    const step = () => {
      if (ist.finished) return resolved({ value, done: true });
      ist.finished = true;
      const rst = R.get(ist.reader);
      if (rst.stream === undefined) return resolved({ value, done: true });
      if (!ist.preventCancel) {
        const stream = rst.stream;
        const p = streamCancel(stream, value);
        genericRelease(rst);
        return then.call(p, () => ({ value, done: true }));
      }
      genericRelease(rst);
      return resolved({ value, done: true });
    };
    ist.ongoing = ist.ongoing ? then.call(ist.ongoing, step, step) : step();
    return ist.ongoing;
  });
  tag(ReadableStreamAsyncIteratorPrototype, "ReadableStream AsyncIterator");

  // ReadableStream.from (§4.2.6, ReadableStreamFromIterable)
  function streamFrom(iterable) {
    let method = iterable == null ? undefined : iterable[Symbol.asyncIterator];
    let iterator;
    if (method == null) {
      const sync = iterable == null ? undefined : iterable[Symbol.iterator];
      if (typeof sync !== "function") throw new TypeError("ReadableStream.from requires an iterable");
      const syncIt = sync.call(iterable);
      iterator = {
        next: () => {
          const r = syncIt.next();
          return then.call(resolved(r.value), (v) => ({ value: v, done: r.done }));
        },
        return: syncIt.return ? (v) => resolved(syncIt.return(v)) : undefined,
      };
    } else {
      if (typeof method !== "function") throw new TypeError("@@asyncIterator is not a function");
      iterator = method.call(iterable);
      if (iterator === null || typeof iterator !== "object") throw new TypeError("not an iterator");
    }
    let stream;
    const pull = () => {
      let r;
      try { r = iterator.next(); } catch (e) { return rejected(e); }
      return then.call(resolved(r), (res) => {
        if (res === null || typeof res !== "object") throw new TypeError("iterator result is not an object");
        if (res.done) controllerClose(C.get(S.get(stream).controller));
        else controllerEnqueue(C.get(S.get(stream).controller), res.value);
      });
    };
    const cancel = (reason) => {
      const ret = iterator.return;
      if (ret === undefined || ret === null) return resolved();
      let r;
      try { r = ret.call(iterator, reason); } catch (e) { return rejected(e); }
      return then.call(resolved(r), (res) => {
        if (res === null || typeof res !== "object") throw new TypeError("iterator result is not an object");
      });
    };
    stream = createStream(() => undefined, pull, cancel, 0);
    return stream;
  }

  for (const [name, cls] of [
    ["ReadableStream", ReadableStream],
    ["ReadableStreamDefaultReader", ReadableStreamDefaultReader],
    ["ReadableStreamDefaultController", ReadableStreamDefaultController],
  ]) {
    define(global, name, cls);
  }

  // --- Helpers del body mixin (Fetch §5.1) ---
  return {
    isStream: (v) => (typeof v === "object" || typeof v === "function") && v !== null && S.has(v),
    isLocked: (s) => S.get(s).reader !== undefined,
    isDisturbed: (s) => S.get(s).disturbed,
    isErrored: (s) => S.get(s).state === "errored",
    // Un stream con un solo chunk (Uint8Array), o vacío.
    fromBytes: (u8) =>
      createStream(
        (c) => {
          const cst = C.get(c);
          if (u8.byteLength > 0) controllerEnqueue(cst, u8);
          controllerClose(cst);
        },
        () => resolved(),
        () => resolved(),
        0,
      ),
    tee: (s) => streamTee(s),
    cancel: (s, r) => streamCancel(s, r),
    // Lee todo el stream: una promesa con un Uint8Array (TypeError si un chunk no lo es).
    readAll: (s) =>
      new P((resolve, reject) => {
        const reader = new ReadableStreamDefaultReader(s);
        const rst = R.get(reader);
        const parts = [];
        let total = 0;
        const step = () => {
          readerRead(rst, {
            chunk: (v) => {
              if (!(v instanceof Uint8Array)) {
                const e = new TypeError("The body stream yielded a chunk that is not a Uint8Array");
                streamCancel(s, e);
                reject(e);
                return;
              }
              parts.push(v);
              total += v.byteLength;
              queueMicrotask(step);
            },
            close: () => {
              const out = new Uint8Array(total);
              let off = 0;
              for (const p of parts) {
                out.set(p, off);
                off += p.byteLength;
              }
              resolve(out);
            },
            error: (e) => reject(e),
          });
        };
        step();
      }),
  };
})
