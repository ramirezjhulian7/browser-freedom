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

  // --- Utilidades de promesas con estado (para `ready`/`closed` de los writers) ---
  function deferred() {
    const d = { settled: false };
    d.promise = new P((res, rej) => {
      d.resolve = (v) => {
        if (!d.settled) {
          d.settled = true;
          res(v);
        }
      };
      d.reject = (e) => {
        if (!d.settled) {
          d.settled = true;
          rej(e);
        }
      };
    });
    return d;
  }
  function settledRejected(e) {
    const d = deferred();
    d.reject(e);
    markHandled(d.promise);
    return d;
  }
  function settledResolved(v) {
    const d = deferred();
    d.resolve(v);
    return d;
  }
  function promiseCall(f, thisArg, ...args) {
    try {
      return resolved(f.apply(thisArg, args));
    } catch (e) {
      return rejected(e);
    }
  }

  // --- WritableStream (§5) ---
  const W = new WeakMap();
  const WW = new WeakMap();
  const WC = new WeakMap();
  const closeSentinel = {};
  let creatingW = false;

  function wstate(s) {
    const st = W.get(s);
    if (!st) throw new TypeError("Illegal invocation: not a WritableStream");
    return st;
  }

  class WritableStream {
    constructor(underlyingSink = undefined, strategy = {}) {
      if (creatingW) return;
      if (underlyingSink === null) throw new TypeError("underlyingSink must not be null");
      const sink = underlyingSink === undefined ? {} : underlyingSink;
      if (typeof sink !== "object" && typeof sink !== "function")
        throw new TypeError("underlyingSink must be an object");
      if (strategy !== undefined && strategy !== null && typeof strategy !== "object")
        throw new TypeError("strategy must be an object");
      strategy = strategy == null ? {} : strategy;
      const abort = sink.abort, close = sink.close, start = sink.start, type = sink.type, write = sink.write;
      if (abort !== undefined && typeof abort !== "function") throw new TypeError("abort must be a function");
      if (close !== undefined && typeof close !== "function") throw new TypeError("close must be a function");
      if (start !== undefined && typeof start !== "function") throw new TypeError("start must be a function");
      if (write !== undefined && typeof write !== "function") throw new TypeError("write must be a function");
      if (type !== undefined) throw new RangeError("Invalid type");
      const size = extractSize(strategy);
      const hwm = extractHWM(strategy, 1);
      initWritable(this);
      setUpWritableController(
        this,
        start ? (c) => start.call(sink, c) : () => undefined,
        write ? (chunk, c) => promiseCall(write, sink, chunk, c) : () => resolved(),
        close ? () => promiseCall(close, sink) : () => resolved(),
        abort ? (r) => promiseCall(abort, sink, r) : () => resolved(),
        hwm,
        size,
      );
    }
    get locked() {
      return wstate(this).writer !== undefined;
    }
    abort(reason = undefined) {
      let st;
      try { st = wstate(this); } catch (e) { return rejected(e); }
      if (st.writer !== undefined) return rejected(new TypeError("Cannot abort a locked stream"));
      return writableAbort(this, reason);
    }
    close() {
      let st;
      try { st = wstate(this); } catch (e) { return rejected(e); }
      if (st.writer !== undefined) return rejected(new TypeError("Cannot close a locked stream"));
      if (closeQueuedOrInFlight(st)) return rejected(new TypeError("The stream is already closing"));
      return writableClose(this);
    }
    getWriter() {
      wstate(this);
      return new WritableStreamDefaultWriter(this);
    }
  }
  enumerable(WritableStream.prototype, ["locked", "abort", "close", "getWriter"]);
  tag(WritableStream.prototype, "WritableStream");

  function initWritable(s) {
    W.set(s, {
      state: "writable",
      storedError: undefined,
      writer: undefined,
      controller: undefined,
      inFlightWriteRequest: undefined,
      closeRequest: undefined,
      inFlightCloseRequest: undefined,
      pendingAbortRequest: undefined,
      writeRequests: [],
      backpressure: false,
    });
  }

  function createWritable(start, write, close, abort, hwm, size) {
    creatingW = true;
    let s;
    try { s = new WritableStream(); } finally { creatingW = false; }
    initWritable(s);
    setUpWritableController(s, start, write, close, abort, hwm, size);
    return s;
  }

  const closeQueuedOrInFlight = (st) => st.closeRequest !== undefined || st.inFlightCloseRequest !== undefined;
  const hasOperationInFlight = (st) => st.inFlightWriteRequest !== undefined || st.inFlightCloseRequest !== undefined;

  function writableAbort(stream, reason) {
    const st = W.get(stream);
    if (st.state === "closed" || st.state === "errored") return resolved(undefined);
    const cst = WC.get(st.controller);
    if (cst.abortController) cst.abortController.abort(reason);
    if (st.state === "closed" || st.state === "errored") return resolved(undefined);
    if (st.pendingAbortRequest !== undefined) return st.pendingAbortRequest.d.promise;
    let wasAlreadyErroring = false;
    if (st.state === "erroring") {
      wasAlreadyErroring = true;
      reason = undefined;
    }
    const d = deferred();
    st.pendingAbortRequest = { d, reason, wasAlreadyErroring };
    if (!wasAlreadyErroring) writableStartErroring(stream, reason);
    return d.promise;
  }

  function writableClose(stream) {
    const st = W.get(stream);
    if (st.state === "closed" || st.state === "errored")
      return rejected(new TypeError("The stream is closed or errored"));
    const d = deferred();
    st.closeRequest = d;
    const writer = st.writer;
    if (writer !== undefined && st.backpressure && st.state === "writable") WW.get(writer).ready.resolve(undefined);
    const cst = WC.get(st.controller);
    enqueueValueWithSize(cst, closeSentinel, 0);
    writableAdvanceQueueIfNeeded(cst);
    return d.promise;
  }

  function writableDealWithRejection(stream, error) {
    const st = W.get(stream);
    if (st.state === "writable") {
      writableStartErroring(stream, error);
      return;
    }
    writableFinishErroring(stream);
  }

  function writableStartErroring(stream, reason) {
    const st = W.get(stream);
    const cst = WC.get(st.controller);
    st.state = "erroring";
    st.storedError = reason;
    if (st.writer !== undefined) writerEnsureReadyRejected(WW.get(st.writer), reason);
    if (!hasOperationInFlight(st) && cst.started) writableFinishErroring(stream);
  }

  function writableFinishErroring(stream) {
    const st = W.get(stream);
    st.state = "errored";
    const cst = WC.get(st.controller);
    cst.queue = [];
    cst.queueTotalSize = 0;
    const storedError = st.storedError;
    for (const req of st.writeRequests) req.reject(storedError);
    st.writeRequests = [];
    if (st.pendingAbortRequest === undefined) {
      rejectCloseAndClosedIfNeeded(stream);
      return;
    }
    const abortRequest = st.pendingAbortRequest;
    st.pendingAbortRequest = undefined;
    if (abortRequest.wasAlreadyErroring) {
      abortRequest.d.reject(storedError);
      rejectCloseAndClosedIfNeeded(stream);
      return;
    }
    const p = cst.abortAlgorithm(abortRequest.reason);
    clearWritableAlgorithms(cst);
    upon(
      p,
      () => {
        abortRequest.d.resolve(undefined);
        rejectCloseAndClosedIfNeeded(stream);
      },
      (r) => {
        abortRequest.d.reject(r);
        rejectCloseAndClosedIfNeeded(stream);
      },
    );
  }

  function finishInFlightWrite(stream) {
    const st = W.get(stream);
    st.inFlightWriteRequest.resolve(undefined);
    st.inFlightWriteRequest = undefined;
  }

  function finishInFlightWriteWithError(stream, error) {
    const st = W.get(stream);
    st.inFlightWriteRequest.reject(error);
    st.inFlightWriteRequest = undefined;
    writableDealWithRejection(stream, error);
  }

  function finishInFlightClose(stream) {
    const st = W.get(stream);
    st.inFlightCloseRequest.resolve(undefined);
    st.inFlightCloseRequest = undefined;
    if (st.state === "erroring") {
      st.storedError = undefined;
      if (st.pendingAbortRequest !== undefined) {
        st.pendingAbortRequest.d.resolve(undefined);
        st.pendingAbortRequest = undefined;
      }
    }
    st.state = "closed";
    if (st.writer !== undefined) WW.get(st.writer).closed.resolve(undefined);
  }

  function finishInFlightCloseWithError(stream, error) {
    const st = W.get(stream);
    st.inFlightCloseRequest.reject(error);
    st.inFlightCloseRequest = undefined;
    if (st.pendingAbortRequest !== undefined) {
      st.pendingAbortRequest.d.reject(error);
      st.pendingAbortRequest = undefined;
    }
    writableDealWithRejection(stream, error);
  }

  function rejectCloseAndClosedIfNeeded(stream) {
    const st = W.get(stream);
    if (st.closeRequest !== undefined) {
      st.closeRequest.reject(st.storedError);
      st.closeRequest = undefined;
    }
    if (st.writer !== undefined) {
      const wst = WW.get(st.writer);
      wst.closed.reject(st.storedError);
      markHandled(wst.closed.promise);
    }
  }

  function writableUpdateBackpressure(stream, backpressure) {
    const st = W.get(stream);
    if (st.writer !== undefined && backpressure !== st.backpressure) {
      const wst = WW.get(st.writer);
      if (backpressure) wst.ready = deferred();
      else wst.ready.resolve(undefined);
    }
    st.backpressure = backpressure;
  }

  // --- WritableStreamDefaultWriter (§5.3) ---
  class WritableStreamDefaultWriter {
    constructor(stream) {
      const st = W.get(stream);
      if (!st) throw new TypeError("WritableStreamDefaultWriter requires a WritableStream");
      if (st.writer !== undefined) throw new TypeError("WritableStream is locked");
      const wst = { stream };
      WW.set(this, wst);
      st.writer = this;
      if (st.state === "writable") {
        wst.ready = !closeQueuedOrInFlight(st) && st.backpressure ? deferred() : settledResolved(undefined);
        wst.closed = deferred();
      } else if (st.state === "erroring") {
        wst.ready = settledRejected(st.storedError);
        wst.closed = deferred();
      } else if (st.state === "closed") {
        wst.ready = settledResolved(undefined);
        wst.closed = settledResolved(undefined);
      } else {
        wst.ready = settledRejected(st.storedError);
        wst.closed = settledRejected(st.storedError);
      }
    }
    get closed() {
      const wst = WW.get(this);
      if (!wst) return rejected(new TypeError("Illegal invocation"));
      return wst.closed.promise;
    }
    get desiredSize() {
      const wst = WW.get(this);
      if (!wst) throw new TypeError("Illegal invocation");
      if (wst.stream === undefined) throw new TypeError("The writer has been released");
      return writerDesiredSize(wst);
    }
    get ready() {
      const wst = WW.get(this);
      if (!wst) return rejected(new TypeError("Illegal invocation"));
      return wst.ready.promise;
    }
    abort(reason = undefined) {
      const wst = WW.get(this);
      if (!wst) return rejected(new TypeError("Illegal invocation"));
      if (wst.stream === undefined) return rejected(new TypeError("The writer has been released"));
      return writableAbort(wst.stream, reason);
    }
    close() {
      const wst = WW.get(this);
      if (!wst) return rejected(new TypeError("Illegal invocation"));
      if (wst.stream === undefined) return rejected(new TypeError("The writer has been released"));
      if (closeQueuedOrInFlight(W.get(wst.stream))) return rejected(new TypeError("The stream is already closing"));
      return writableClose(wst.stream);
    }
    releaseLock() {
      const wst = WW.get(this);
      if (!wst) throw new TypeError("Illegal invocation");
      if (wst.stream === undefined) return;
      writerRelease(this);
    }
    write(chunk = undefined) {
      const wst = WW.get(this);
      if (!wst) return rejected(new TypeError("Illegal invocation"));
      if (wst.stream === undefined) return rejected(new TypeError("The writer has been released"));
      return writerWrite(this, chunk);
    }
  }
  enumerable(WritableStreamDefaultWriter.prototype, ["closed", "desiredSize", "ready", "abort", "close", "releaseLock", "write"]);
  tag(WritableStreamDefaultWriter.prototype, "WritableStreamDefaultWriter");

  function writerDesiredSize(wst) {
    const st = W.get(wst.stream);
    if (st.state === "errored" || st.state === "erroring") return null;
    if (st.state === "closed") return 0;
    const cst = WC.get(st.controller);
    return cst.hwm - cst.queueTotalSize;
  }

  function writerEnsureReadyRejected(wst, error) {
    if (!wst.ready.settled) wst.ready.reject(error);
    else wst.ready = settledRejected(error);
    markHandled(wst.ready.promise);
  }

  function writerEnsureClosedRejected(wst, error) {
    if (!wst.closed.settled) wst.closed.reject(error);
    else wst.closed = settledRejected(error);
    markHandled(wst.closed.promise);
  }

  function writerRelease(writer) {
    const wst = WW.get(writer);
    const st = W.get(wst.stream);
    const e = new TypeError("Writer was released");
    writerEnsureReadyRejected(wst, e);
    writerEnsureClosedRejected(wst, e);
    st.writer = undefined;
    wst.stream = undefined;
  }

  function writerWrite(writer, chunk) {
    const wst = WW.get(writer);
    const stream = wst.stream;
    const st = W.get(stream);
    const cst = WC.get(st.controller);
    let chunkSize = 1;
    if (cst.sizeAlgorithm !== undefined) {
      try {
        chunkSize = cst.sizeAlgorithm(chunk);
      } catch (e) {
        writableControllerErrorIfNeeded(cst, e);
        chunkSize = 1;
      }
    }
    if (stream !== wst.stream) return rejected(new TypeError("The writer has been released"));
    if (st.state === "errored") return rejected(st.storedError);
    if (closeQueuedOrInFlight(st) || st.state === "closed")
      return rejected(new TypeError("The stream is closing or closed"));
    if (st.state === "erroring") return rejected(st.storedError);
    const d = deferred();
    st.writeRequests.push(d);
    writableControllerWrite(cst, chunk, chunkSize);
    return d.promise;
  }

  function writerCloseWithErrorPropagation(writer) {
    const wst = WW.get(writer);
    const st = W.get(wst.stream);
    if (closeQueuedOrInFlight(st) || st.state === "closed") return resolved(undefined);
    if (st.state === "errored") return rejected(st.storedError);
    return writableClose(wst.stream);
  }

  // --- WritableStreamDefaultController (§5.4) ---
  class WritableStreamDefaultController {
    constructor() {
      throw new TypeError("Illegal constructor");
    }
    get signal() {
      const cst = WC.get(this);
      if (!cst) throw new TypeError("Illegal invocation");
      return cst.abortController ? cst.abortController.signal : undefined;
    }
    error(e = undefined) {
      const cst = WC.get(this);
      if (!cst) throw new TypeError("Illegal invocation");
      if (W.get(cst.stream).state !== "writable") return;
      writableControllerError(cst, e);
    }
  }
  enumerable(WritableStreamDefaultController.prototype, ["signal", "error"]);
  tag(WritableStreamDefaultController.prototype, "WritableStreamDefaultController");

  function setUpWritableController(stream, start, write, close, abort, hwm, size) {
    const controller = Object.create(WritableStreamDefaultController.prototype);
    const AC = global.AbortController;
    const cst = {
      stream,
      queue: [],
      queueTotalSize: 0,
      abortController: typeof AC === "function" ? new AC() : undefined,
      started: false,
      sizeAlgorithm: size,
      hwm,
      writeAlgorithm: write,
      closeAlgorithm: close,
      abortAlgorithm: abort,
    };
    WC.set(controller, cst);
    const st = W.get(stream);
    st.controller = controller;
    writableUpdateBackpressure(stream, cst.hwm - cst.queueTotalSize <= 0);
    const startResult = start(controller);
    upon(
      resolved(startResult),
      () => {
        cst.started = true;
        writableAdvanceQueueIfNeeded(cst);
      },
      (r) => {
        cst.started = true;
        writableDealWithRejection(stream, r);
      },
    );
  }

  function clearWritableAlgorithms(cst) {
    cst.writeAlgorithm = undefined;
    cst.closeAlgorithm = undefined;
    cst.abortAlgorithm = undefined;
    cst.sizeAlgorithm = undefined;
  }

  function enqueueValueWithSize(cst, value, size) {
    size = Number(size);
    if (Number.isNaN(size) || size < 0 || size === Infinity) throw new RangeError("Invalid chunk size");
    cst.queue.push({ value, size });
    cst.queueTotalSize += size;
  }

  function dequeueValue(cst) {
    const { value, size } = cst.queue.shift();
    cst.queueTotalSize -= size;
    if (cst.queueTotalSize < 0) cst.queueTotalSize = 0;
    return value;
  }

  function writableControllerWrite(cst, chunk, chunkSize) {
    try {
      enqueueValueWithSize(cst, chunk, chunkSize);
    } catch (e) {
      writableControllerErrorIfNeeded(cst, e);
      return;
    }
    const st = W.get(cst.stream);
    if (!closeQueuedOrInFlight(st) && st.state === "writable")
      writableUpdateBackpressure(cst.stream, cst.hwm - cst.queueTotalSize <= 0);
    writableAdvanceQueueIfNeeded(cst);
  }

  function writableAdvanceQueueIfNeeded(cst) {
    const stream = cst.stream;
    const st = W.get(stream);
    if (!cst.started) return;
    if (st.inFlightWriteRequest !== undefined) return;
    if (st.state === "erroring") {
      writableFinishErroring(stream);
      return;
    }
    if (st.state !== "writable") return;
    if (cst.queue.length === 0) return;
    const value = cst.queue[0].value;
    if (value === closeSentinel) {
      // ProcessClose
      st.inFlightCloseRequest = st.closeRequest;
      st.closeRequest = undefined;
      dequeueValue(cst);
      const p = cst.closeAlgorithm();
      clearWritableAlgorithms(cst);
      upon(p, () => finishInFlightClose(stream), (r) => finishInFlightCloseWithError(stream, r));
    } else {
      // ProcessWrite
      st.inFlightWriteRequest = st.writeRequests.shift();
      upon(
        cst.writeAlgorithm(value, st.controller),
        () => {
          finishInFlightWrite(stream);
          dequeueValue(cst);
          if (!closeQueuedOrInFlight(st) && st.state === "writable")
            writableUpdateBackpressure(stream, cst.hwm - cst.queueTotalSize <= 0);
          writableAdvanceQueueIfNeeded(cst);
        },
        (r) => {
          if (st.state === "writable") clearWritableAlgorithms(cst);
          finishInFlightWriteWithError(stream, r);
        },
      );
    }
  }

  function writableControllerErrorIfNeeded(cst, e) {
    if (W.get(cst.stream).state === "writable") writableControllerError(cst, e);
  }

  function writableControllerError(cst, e) {
    clearWritableAlgorithms(cst);
    writableStartErroring(cst.stream, e);
  }

  // --- pipeTo / pipeThrough (§4.9.13, ReadableStreamPipeTo) ---
  function pipeOptions(options) {
    if (options !== undefined && options !== null && typeof options !== "object")
      throw new TypeError("options must be an object");
    const o = options == null ? {} : options;
    const preventAbort = !!o.preventAbort;
    const preventCancel = !!o.preventCancel;
    const preventClose = !!o.preventClose;
    const signal = o.signal;
    if (signal !== undefined) {
      const AS = global.AbortSignal;
      if (typeof AS !== "function" || !(signal instanceof AS)) throw new TypeError("signal must be an AbortSignal");
    }
    return { preventAbort, preventCancel, preventClose, signal };
  }

  function pipeTo(source, dest, preventClose, preventAbort, preventCancel, signal) {
    const reader = new ReadableStreamDefaultReader(source);
    const rst = R.get(reader);
    const writer = new WritableStreamDefaultWriter(dest);
    const wst = WW.get(writer);
    S.get(source).disturbed = true;
    let shuttingDown = false;
    let currentWrite = resolved(undefined);
    return new P((resolve, reject) => {
      let abortAlgorithm;
      const finalize = (isError, error) => {
        writerRelease(writer);
        genericRelease(rst);
        if (signal !== undefined) signal.removeEventListener("abort", abortAlgorithm);
        if (isError) reject(error);
        else resolve(undefined);
      };
      const waitForWrites = () => {
        const old = currentWrite;
        return then.call(old, () => (old !== currentWrite ? waitForWrites() : undefined));
      };
      const destOpen = () => W.get(dest).state === "writable" && !closeQueuedOrInFlight(W.get(dest));
      const shutdownWithAction = (action, originalIsError, originalError) => {
        if (shuttingDown) return;
        shuttingDown = true;
        const doIt = () =>
          upon(
            action(),
            () => finalize(originalIsError, originalError),
            (e) => finalize(true, e),
          );
        if (destOpen()) upon(waitForWrites(), doIt, doIt);
        else doIt();
      };
      const shutdown = (isError, error) => {
        if (shuttingDown) return;
        shuttingDown = true;
        const fin = () => finalize(isError, error);
        if (destOpen()) upon(waitForWrites(), fin, fin);
        else fin();
      };
      if (signal !== undefined) {
        abortAlgorithm = () => {
          const error = signal.reason;
          const actions = [];
          if (!preventAbort)
            actions.push(() => (W.get(dest).state === "writable" ? writableAbort(dest, error) : resolved(undefined)));
          if (!preventCancel)
            actions.push(() => (S.get(source).state === "readable" ? streamCancel(source, error) : resolved(undefined)));
          shutdownWithAction(() => P.all(actions.map((a) => a())), true, error);
        };
        if (signal.aborted) {
          abortAlgorithm();
          return;
        }
        signal.addEventListener("abort", abortAlgorithm);
      }
      // Errores y cierres en ambos sentidos.
      const sourceErrored = (e) => {
        if (!preventAbort) shutdownWithAction(() => writableAbort(dest, e), true, e);
        else shutdown(true, e);
      };
      const destErrored = (e) => {
        if (!preventCancel) shutdownWithAction(() => streamCancel(source, e), true, e);
        else shutdown(true, e);
      };
      const sourceClosed = () => {
        if (!preventClose) shutdownWithAction(() => writerCloseWithErrorPropagation(writer));
        else shutdown();
      };
      if (S.get(source).state === "errored") sourceErrored(S.get(source).storedError);
      else upon(rst.closedPromise, undefined, sourceErrored);
      if (W.get(dest).state === "errored") destErrored(W.get(dest).storedError);
      else upon(wst.closed.promise, undefined, destErrored);
      if (S.get(source).state === "closed") sourceClosed();
      else upon(rst.closedPromise, sourceClosed, () => {});
      if (closeQueuedOrInFlight(W.get(dest)) || W.get(dest).state === "closed") {
        const destClosed = new TypeError("the destination writable stream closed before all data could be piped to it");
        if (!preventCancel) shutdownWithAction(() => streamCancel(source, destClosed), true, destClosed);
        else shutdown(true, destClosed);
      }
      const step = () => {
        if (shuttingDown) return;
        upon(
          wst.ready.promise,
          () => {
            if (shuttingDown || rst.stream === undefined) return;
            readerRead(rst, {
              chunk: (chunk) => {
                if (wst.stream === undefined) return;
                currentWrite = writerWrite(writer, chunk);
                markHandled(currentWrite);
                queueMicrotask(step);
              },
              close: () => {},
              error: () => {},
            });
          },
          () => {},
        );
      };
      step();
    });
  }

  define(ReadableStream.prototype, "pipeTo", { pipeTo(destination, options = {}) {
    if (!S.has(this)) return rejected(new TypeError("Illegal invocation"));
    if (!W.has(destination)) return rejected(new TypeError("destination must be a WritableStream"));
    let o;
    try { o = pipeOptions(options); } catch (e) { return rejected(e); }
    if (S.get(this).reader !== undefined) return rejected(new TypeError("The source stream is locked"));
    if (W.get(destination).writer !== undefined) return rejected(new TypeError("The destination stream is locked"));
    return pipeTo(this, destination, o.preventClose, o.preventAbort, o.preventCancel, o.signal);
  } }.pipeTo);
  define(ReadableStream.prototype, "pipeThrough", { pipeThrough(transform, options = {}) {
    if (!S.has(this)) throw new TypeError("Illegal invocation");
    const readable = transform == null ? undefined : transform.readable;
    if (!S.has(readable)) throw new TypeError("transform.readable must be a ReadableStream");
    const writable = transform.writable;
    if (!W.has(writable)) throw new TypeError("transform.writable must be a WritableStream");
    const o = pipeOptions(options);
    if (S.get(this).reader !== undefined) throw new TypeError("The source stream is locked");
    if (W.get(writable).writer !== undefined) throw new TypeError("The writable stream is locked");
    const p = pipeTo(this, writable, o.preventClose, o.preventAbort, o.preventCancel, o.signal);
    markHandled(p);
    return readable;
  } }.pipeThrough);
  enumerable(ReadableStream.prototype, ["pipeTo", "pipeThrough"]);

  // --- TransformStream (§6) ---
  const T = new WeakMap();
  const TC = new WeakMap();

  class TransformStream {
    constructor(transformer = undefined, writableStrategy = {}, readableStrategy = {}) {
      if (transformer === null) throw new TypeError("transformer must not be null");
      const tr = transformer === undefined ? {} : transformer;
      if (typeof tr !== "object" && typeof tr !== "function") throw new TypeError("transformer must be an object");
      const cancel = tr.cancel, flush = tr.flush, readableType = tr.readableType, start = tr.start,
        transform = tr.transform, writableType = tr.writableType;
      if (cancel !== undefined && typeof cancel !== "function") throw new TypeError("cancel must be a function");
      if (flush !== undefined && typeof flush !== "function") throw new TypeError("flush must be a function");
      if (start !== undefined && typeof start !== "function") throw new TypeError("start must be a function");
      if (transform !== undefined && typeof transform !== "function") throw new TypeError("transform must be a function");
      if (readableType !== undefined) throw new RangeError("Invalid readableType");
      if (writableType !== undefined) throw new RangeError("Invalid writableType");
      writableStrategy = writableStrategy == null ? {} : writableStrategy;
      readableStrategy = readableStrategy == null ? {} : readableStrategy;
      const readableHWM = extractHWM(readableStrategy, 0);
      const readableSize = extractSize(readableStrategy);
      const writableHWM = extractHWM(writableStrategy, 1);
      const writableSize = extractSize(writableStrategy);
      const startD = deferred();
      initTransform(this, startD.promise, writableHWM, writableSize, readableHWM, readableSize);
      const controller = Object.create(TransformStreamDefaultController.prototype);
      setUpTransformController(
        this,
        controller,
        transform
          ? (chunk) => promiseCall(transform, tr, chunk, controller)
          : (chunk) => {
              try {
                transformEnqueue(TC.get(controller), chunk);
                return resolved(undefined);
              } catch (e) {
                return rejected(e);
              }
            },
        flush ? () => promiseCall(flush, tr, controller) : () => resolved(undefined),
        cancel ? (r) => promiseCall(cancel, tr, r) : () => resolved(undefined),
      );
      if (start) {
        try {
          startD.resolve(start.call(tr, controller));
        } catch (e) {
          startD.reject(e);
          throw e;
        }
      } else startD.resolve(undefined);
    }
    get readable() {
      const t = T.get(this);
      if (!t) throw new TypeError("Illegal invocation");
      return t.readable;
    }
    get writable() {
      const t = T.get(this);
      if (!t) throw new TypeError("Illegal invocation");
      return t.writable;
    }
  }
  enumerable(TransformStream.prototype, ["readable", "writable"]);
  tag(TransformStream.prototype, "TransformStream");

  function initTransform(stream, startPromise, writableHWM, writableSize, readableHWM, readableSize) {
    const t = { backpressure: undefined, backpressureChange: undefined, controller: undefined };
    T.set(stream, t);
    const start = () => startPromise;
    t.writable = createWritable(
      start,
      (chunk) => transformSinkWrite(stream, chunk),
      () => transformSinkClose(stream),
      (reason) => transformSinkAbort(stream, reason),
      writableHWM,
      writableSize,
    );
    t.readable = createStream(
      start,
      () => {
        transformSetBackpressure(t, false);
        return t.backpressureChange.promise;
      },
      (reason) => transformSourceCancel(stream, reason),
      readableHWM,
      readableSize,
    );
    transformSetBackpressure(t, true);
  }

  function transformSetBackpressure(t, bp) {
    if (t.backpressureChange !== undefined) t.backpressureChange.resolve(undefined);
    t.backpressureChange = deferred();
    t.backpressure = bp;
  }

  const readableCst = (t) => C.get(S.get(t.readable).controller);
  const writableCst = (t) => WC.get(W.get(t.writable).controller);

  function transformError(stream, e) {
    const t = T.get(stream);
    controllerError(readableCst(t), e);
    transformErrorWritableAndUnblock(stream, e);
  }

  function transformErrorWritableAndUnblock(stream, e) {
    const t = T.get(stream);
    clearTransformAlgorithms(TC.get(t.controller));
    writableControllerErrorIfNeeded(writableCst(t), e);
    if (t.backpressure) transformSetBackpressure(t, false);
  }

  class TransformStreamDefaultController {
    constructor() {
      throw new TypeError("Illegal constructor");
    }
    get desiredSize() {
      const tc = TC.get(this);
      if (!tc) throw new TypeError("Illegal invocation");
      return desiredSize(readableCst(T.get(tc.stream)));
    }
    enqueue(chunk = undefined) {
      const tc = TC.get(this);
      if (!tc) throw new TypeError("Illegal invocation");
      transformEnqueue(tc, chunk);
    }
    error(reason = undefined) {
      const tc = TC.get(this);
      if (!tc) throw new TypeError("Illegal invocation");
      transformError(tc.stream, reason);
    }
    terminate() {
      const tc = TC.get(this);
      if (!tc) throw new TypeError("Illegal invocation");
      const t = T.get(tc.stream);
      controllerClose(readableCst(t));
      transformErrorWritableAndUnblock(tc.stream, new TypeError("TransformStream terminated"));
    }
  }
  enumerable(TransformStreamDefaultController.prototype, ["desiredSize", "enqueue", "error", "terminate"]);
  tag(TransformStreamDefaultController.prototype, "TransformStreamDefaultController");

  function setUpTransformController(stream, controller, transformAlgorithm, flushAlgorithm, cancelAlgorithm) {
    const tc = { stream, transformAlgorithm, flushAlgorithm, cancelAlgorithm, finish: undefined };
    TC.set(controller, tc);
    T.get(stream).controller = controller;
  }

  function clearTransformAlgorithms(tc) {
    tc.transformAlgorithm = undefined;
    tc.flushAlgorithm = undefined;
    tc.cancelAlgorithm = undefined;
  }

  function transformEnqueue(tc, chunk) {
    const t = T.get(tc.stream);
    const rc = readableCst(t);
    if (!canCloseOrEnqueue(rc)) throw new TypeError("Readable side is not in a state that permits enqueue");
    try {
      controllerEnqueue(rc, chunk);
    } catch (e) {
      transformErrorWritableAndUnblock(tc.stream, e);
      throw S.get(t.readable).storedError;
    }
    const backpressure = !shouldCallPull(rc);
    if (backpressure !== t.backpressure) transformSetBackpressure(t, true);
  }

  function transformPerform(tc, chunk) {
    const p = tc.transformAlgorithm(chunk);
    return then.call(p, undefined, (r) => {
      transformError(tc.stream, r);
      throw r;
    });
  }

  function transformSinkWrite(stream, chunk) {
    const t = T.get(stream);
    const tc = TC.get(t.controller);
    if (t.backpressure) {
      return then.call(t.backpressureChange.promise, () => {
        const wst = W.get(t.writable);
        if (wst.state === "erroring") throw wst.storedError;
        return transformPerform(tc, chunk);
      });
    }
    return transformPerform(tc, chunk);
  }

  function transformSinkAbort(stream, reason) {
    const t = T.get(stream);
    const tc = TC.get(t.controller);
    if (tc.finish !== undefined) return tc.finish.promise;
    tc.finish = deferred();
    const p = tc.cancelAlgorithm(reason);
    clearTransformAlgorithms(tc);
    upon(
      p,
      () => {
        const rs = S.get(t.readable);
        if (rs.state === "errored") tc.finish.reject(rs.storedError);
        else {
          controllerError(readableCst(t), reason);
          tc.finish.resolve(undefined);
        }
      },
      (r) => {
        controllerError(readableCst(t), r);
        tc.finish.reject(r);
      },
    );
    return tc.finish.promise;
  }

  function transformSinkClose(stream) {
    const t = T.get(stream);
    const tc = TC.get(t.controller);
    if (tc.finish !== undefined) return tc.finish.promise;
    tc.finish = deferred();
    const p = tc.flushAlgorithm();
    clearTransformAlgorithms(tc);
    upon(
      p,
      () => {
        const rs = S.get(t.readable);
        if (rs.state === "errored") tc.finish.reject(rs.storedError);
        else {
          controllerClose(readableCst(t));
          tc.finish.resolve(undefined);
        }
      },
      (r) => {
        controllerError(readableCst(t), r);
        tc.finish.reject(r);
      },
    );
    return tc.finish.promise;
  }

  function transformSourceCancel(stream, reason) {
    const t = T.get(stream);
    const tc = TC.get(t.controller);
    if (tc.finish !== undefined) return tc.finish.promise;
    tc.finish = deferred();
    const p = tc.cancelAlgorithm(reason);
    clearTransformAlgorithms(tc);
    upon(
      p,
      () => {
        const ws = W.get(t.writable);
        if (ws.state === "errored") tc.finish.reject(ws.storedError);
        else {
          writableControllerErrorIfNeeded(writableCst(t), reason);
          if (t.backpressure) transformSetBackpressure(t, false);
          tc.finish.resolve(undefined);
        }
      },
      (r) => {
        writableControllerErrorIfNeeded(writableCst(t), r);
        if (t.backpressure) transformSetBackpressure(t, false);
        tc.finish.reject(r);
      },
    );
    return tc.finish.promise;
  }

  // --- Estrategias de cola (§7) ---
  const countSize = function size() {
    return 1;
  };
  const byteSize = function size(chunk) {
    return chunk.byteLength;
  };
  const QS = new WeakMap();
  function strategyInit(o, init, name) {
    if (init === null || typeof init !== "object")
      throw new TypeError(`Failed to construct '${name}': 1 argument required`);
    if (!("highWaterMark" in init) && init.highWaterMark === undefined)
      throw new TypeError(`Failed to construct '${name}': required member highWaterMark is undefined.`);
    QS.set(o, Number(init.highWaterMark));
  }
  class CountQueuingStrategy {
    constructor(init) {
      strategyInit(this, init, "CountQueuingStrategy");
    }
    get highWaterMark() {
      if (!QS.has(this)) throw new TypeError("Illegal invocation");
      return QS.get(this);
    }
    get size() {
      if (!QS.has(this)) throw new TypeError("Illegal invocation");
      return countSize;
    }
  }
  enumerable(CountQueuingStrategy.prototype, ["highWaterMark", "size"]);
  tag(CountQueuingStrategy.prototype, "CountQueuingStrategy");
  class ByteLengthQueuingStrategy {
    constructor(init) {
      strategyInit(this, init, "ByteLengthQueuingStrategy");
    }
    get highWaterMark() {
      if (!QS.has(this)) throw new TypeError("Illegal invocation");
      return QS.get(this);
    }
    get size() {
      if (!QS.has(this)) throw new TypeError("Illegal invocation");
      return byteSize;
    }
  }
  enumerable(ByteLengthQueuingStrategy.prototype, ["highWaterMark", "size"]);
  tag(ByteLengthQueuingStrategy.prototype, "ByteLengthQueuingStrategy");

  // --- TextEncoderStream / TextDecoderStream (Encoding §5) ---
  const TS = new WeakMap();
  class TextEncoderStream {
    constructor() {
      const enc = new global.TextEncoder();
      let pending = null;
      const transform = new TransformStream({
        transform(chunk, controller) {
          let s = String(chunk);
          if (pending !== null) {
            s = pending + s;
            pending = null;
          }
          const last = s.length ? s.charCodeAt(s.length - 1) : 0;
          if (last >= 0xd800 && last <= 0xdbff) {
            pending = s[s.length - 1];
            s = s.slice(0, -1);
          }
          if (s.length) controller.enqueue(enc.encode(s));
        },
        flush(controller) {
          if (pending !== null) controller.enqueue(new Uint8Array([0xef, 0xbf, 0xbd]));
        },
      });
      TS.set(this, { transform });
    }
    get encoding() {
      if (!TS.has(this)) throw new TypeError("Illegal invocation");
      return "utf-8";
    }
    get readable() {
      if (!TS.has(this)) throw new TypeError("Illegal invocation");
      return TS.get(this).transform.readable;
    }
    get writable() {
      if (!TS.has(this)) throw new TypeError("Illegal invocation");
      return TS.get(this).transform.writable;
    }
  }
  enumerable(TextEncoderStream.prototype, ["encoding", "readable", "writable"]);
  tag(TextEncoderStream.prototype, "TextEncoderStream");

  class TextDecoderStream {
    constructor(label = "utf-8", options = {}) {
      const dec = new global.TextDecoder(label, options);
      const isBufferSource = (c) => c instanceof ArrayBuffer || ArrayBuffer.isView(c) ||
        (typeof SharedArrayBuffer === "function" && c instanceof SharedArrayBuffer);
      const transform = new TransformStream({
        transform(chunk, controller) {
          if (!isBufferSource(chunk)) throw new TypeError("TextDecoderStream chunks must be BufferSource");
          const s = dec.decode(chunk, { stream: true });
          if (s.length) controller.enqueue(s);
        },
        flush(controller) {
          const s = dec.decode();
          if (s.length) controller.enqueue(s);
        },
      });
      TS.set(this, { transform, dec });
    }
    get encoding() {
      if (!TS.has(this)) throw new TypeError("Illegal invocation");
      return TS.get(this).dec.encoding;
    }
    get fatal() {
      if (!TS.has(this)) throw new TypeError("Illegal invocation");
      return TS.get(this).dec.fatal;
    }
    get ignoreBOM() {
      if (!TS.has(this)) throw new TypeError("Illegal invocation");
      return TS.get(this).dec.ignoreBOM;
    }
    get readable() {
      if (!TS.has(this)) throw new TypeError("Illegal invocation");
      return TS.get(this).transform.readable;
    }
    get writable() {
      if (!TS.has(this)) throw new TypeError("Illegal invocation");
      return TS.get(this).transform.writable;
    }
  }
  enumerable(TextDecoderStream.prototype, ["encoding", "fatal", "ignoreBOM", "readable", "writable"]);
  tag(TextDecoderStream.prototype, "TextDecoderStream");

  for (const [name, cls] of [
    ["WritableStream", WritableStream],
    ["WritableStreamDefaultWriter", WritableStreamDefaultWriter],
    ["WritableStreamDefaultController", WritableStreamDefaultController],
    ["TransformStream", TransformStream],
    ["TransformStreamDefaultController", TransformStreamDefaultController],
    ["CountQueuingStrategy", CountQueuingStrategy],
    ["ByteLengthQueuingStrategy", ByteLengthQueuingStrategy],
    ["TextEncoderStream", TextEncoderStream],
    ["TextDecoderStream", TextDecoderStream],
  ]) {
    define(global, name, cls);
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
