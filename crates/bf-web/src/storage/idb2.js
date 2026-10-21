  // --- El estado de las bases de datos (§2.1–§2.7) ---
  const DBS = new Map(); // nombre → DbState (cargadas)
  let nextStoreId = 1;

  function sortedInsert(records, rec, compare) {
    let lo = 0, hi = records.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (compare(records[mid], rec) < 0) lo = mid + 1;
      else hi = mid;
    }
    records.splice(lo, 0, rec);
  }
  // El primer índice con `records[i].key >= key` (o `> key` con `after`).
  function lowerIndex(records, key, after) {
    let lo = 0, hi = records.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      const c = cmp(records[mid].key, key);
      if (c < 0 || (after && c === 0)) lo = mid + 1;
      else hi = mid;
    }
    return lo;
  }
  const storeOrder = (a, b) => cmp(a.key, b.key);
  const indexOrder = (a, b) => cmp(a.key, b.key) || cmp(a.pk, b.pk);

  function newStore(name, keyPath, autoIncrement) {
    return { id: nextStoreId++, name, keyPath, autoIncrement, current: 1, records: [], indexes: new Map(), deleted: false };
  }
  function newIndex(store, name, keyPath, unique, multiEntry) {
    return { store, name, keyPath, unique, multiEntry, records: [], deleted: false };
  }
  function findRecord(store, key) {
    const i = lowerIndex(store.records, key, false);
    const r = store.records[i];
    return r && cmp(r.key, key) === 0 ? { i, r } : null;
  }
  // Las claves de índice de un valor (deserializado).
  function indexKeys(index, value) {
    const k = extractKey(value, index.keyPath, index.multiEntry);
    if (k === FAIL || k === null) return [];
    if (k.multi) return k.v;
    return [k];
  }
  function indexAdd(index, keys, pk) {
    for (const k of keys) sortedInsert(index.records, { key: k, pk }, indexOrder);
  }
  function indexRemove(index, pk) {
    index.records = index.records.filter((r) => cmp(r.pk, pk) !== 0);
  }
  // "Store a record into an object store" sin el generador (§5.4): `null` o un error.
  function storeRecord(store, key, tree, noOverwrite) {
    const existing = findRecord(store, key);
    if (existing && noOverwrite) return err("ConstraintError", "Key already exists in the object store.");
    const value = store.indexes.size ? deserialize(tree) : undefined;
    const perIndex = [];
    for (const index of store.indexes.values()) {
      const keys = indexKeys(index, value);
      if (index.unique) {
        for (const k of keys) {
          const i = lowerIndex(index.records, k, false);
          const r = index.records[i];
          if (r && cmp(r.key, k) === 0 && (!existing || cmp(r.pk, key) !== 0)) {
            return err("ConstraintError", "Unable to add key to index '" + index.name + "': at least one key does not satisfy the uniqueness requirements.");
          }
        }
      }
      perIndex.push([index, keys]);
    }
    if (existing) {
      store.records.splice(existing.i, 1);
      for (const index of store.indexes.values()) indexRemove(index, key);
    }
    sortedInsert(store.records, { key, value: tree }, storeOrder);
    for (const [index, keys] of perIndex) indexAdd(index, keys, key);
    return null;
  }
  function deleteRecords(store, r) {
    const removed = store.records.filter((rec) => inRange(r, rec.key));
    if (!removed.length) return removed;
    store.records = store.records.filter((rec) => !inRange(r, rec.key));
    for (const index of store.indexes.values()) {
      index.records = index.records.filter((ir) => !removed.some((rec) => cmp(rec.key, ir.pk) === 0));
    }
    return removed;
  }
  function populateIndex(index) {
    const store = index.store;
    index.records = [];
    for (const rec of store.records) {
      const keys = indexKeys(index, deserialize(rec.value));
      if (index.unique) {
        for (const k of keys) {
          const i = lowerIndex(index.records, k, false);
          const r = index.records[i];
          if (r && cmp(r.key, k) === 0) return false;
        }
      }
      indexAdd(index, keys, rec.key);
    }
    return true;
  }

  // --- La persistencia (bf-web/src/storage/backend.rs) ---
  function newDb(name) {
    return { name, version: 0, stores: new Map(), connections: new Set(), queue: [], busy: false, txns: [], logLines: 0, waiting: null };
  }
  // Aplica una operación del log.
  function applyOp(db, op) {
    const st = (n) => db.stores.get(n);
    switch (op[0]) {
      case "ver":
        db.version = op[1];
        break;
      case "cs":
        db.stores.set(op[1], newStore(op[1], op[2], op[3]));
        break;
      case "ds":
        db.stores.delete(op[1]);
        break;
      case "rs": {
        const s = st(op[1]);
        db.stores.delete(op[1]);
        s.name = op[2];
        db.stores.set(op[2], s);
        break;
      }
      case "ci": {
        const s = st(op[1]);
        const ix = newIndex(s, op[2], op[3], op[4], op[5]);
        s.indexes.set(op[2], ix);
        populateIndex(ix);
        break;
      }
      case "di":
        st(op[1]).indexes.delete(op[2]);
        break;
      case "ri": {
        const s = st(op[1]);
        const ix = s.indexes.get(op[2]);
        s.indexes.delete(op[2]);
        ix.name = op[3];
        s.indexes.set(op[3], ix);
        break;
      }
      case "put":
        storeRecord(st(op[1]), keyFromJson(op[2]), op[3], false);
        break;
      case "del": {
        const k = keyFromJson(op[2]);
        deleteRecords(st(op[1]), { lower: k, upper: k, lowerOpen: false, upperOpen: false });
        break;
      }
      case "clear": {
        const s = st(op[1]);
        s.records = [];
        for (const ix of s.indexes.values()) ix.records = [];
        break;
      }
      case "gen":
        st(op[1]).current = op[2];
        break;
    }
  }
  // Carga una base de datos del log (`null` si no existe).
  function loadDb(name) {
    if (DBS.has(name)) return DBS.get(name);
    const text = native.load(name);
    if (text === null) return null;
    const db = newDb(name);
    let batch = [];
    let lines = 0;
    for (const line of text.split("\n")) {
      if (!line) continue;
      lines++;
      let op;
      try {
        op = JSON.parse(line);
      } catch (e) {
        break;
      }
      if (op[0] === "commit") {
        for (const o of batch) applyOp(db, o);
        batch = [];
      } else batch.push(op);
    }
    db.logLines = lines;
    if (db.version === 0) return null;
    DBS.set(name, db);
    return db;
  }
  function liveCount(db) {
    let n = 2;
    for (const s of db.stores.values()) n += 2 + s.indexes.size + s.records.length;
    return n;
  }
  function snapshotOps(db) {
    const ops = [["ver", db.version]];
    for (const s of db.stores.values()) {
      ops.push(["cs", s.name, s.keyPath, s.autoIncrement]);
      ops.push(["gen", s.name, s.current]);
      for (const r of s.records) ops.push(["put", s.name, keyToJson(r.key), r.value]);
      for (const ix of s.indexes.values()) ops.push(["ci", s.name, ix.name, ix.keyPath, ix.unique, ix.multiEntry]);
    }
    return ops;
  }
  // Escribe las operaciones de una transacción (y compacta si el log creció mucho).
  function persist(db, ops) {
    if (!ops.length) return true;
    const text = ops.map((o) => JSON.stringify(o)).join("\n") + '\n["commit"]\n';
    if (!native.append(db.name, text)) return false;
    db.logLines += ops.length + 1;
    if (db.logLines > 500 && db.logLines > 4 * liveCount(db)) {
      const snap = snapshotOps(db);
      if (native.compact(db.name, snap.map((o) => JSON.stringify(o)).join("\n") + '\n["commit"]\n'))
        db.logLines = snap.length + 1;
    }
    return true;
  }

  // --- Disparar eventos (§5.5, "fire a success event" / "fire an error event") ---
  const FIRE_CANCELED = 1, FIRE_THREW = 2;
  const queueTask = (f) => native.queueTask(f);

  function fireSuccess(request) {
    const txn = R.get(request).transaction;
    const t = txn ? X.get(txn) : null;
    const ev = new Event("success", { bubbles: false, cancelable: false });
    if (t && t.state === "inactive") t.state = "active";
    const flags = native.fire(request, ev);
    if (t) {
      if (t.state === "active") t.state = "inactive";
      if (flags & FIRE_THREW) abortTxn(txn, err("AbortError", "A listener threw an exception."));
    }
  }
  function fireError(request) {
    const txn = R.get(request).transaction;
    const t = txn ? X.get(txn) : null;
    const ev = new Event("error", { bubbles: true, cancelable: true });
    if (t && t.state === "inactive") t.state = "active";
    const flags = native.fire(request, ev);
    if (t) {
      if (t.state === "active") t.state = "inactive";
      if (flags & FIRE_THREW) abortTxn(txn, err("AbortError", "A listener threw an exception."));
      else if (!(flags & FIRE_CANCELED)) abortTxn(txn, R.get(request).error);
    }
  }

  // --- IDBRequest / IDBOpenDBRequest (§4.1) ---
  const R = new WeakMap();
  class IDBRequest extends EventTarget {
    constructor(token) {
      illegal(token);
      super();
      R.set(this, { readyState: "pending", result: undefined, error: null, source: null, transaction: null });
    }
    get result() {
      const r = req(this);
      if (r.readyState !== "done") throw err("InvalidStateError", "The request has not finished.");
      return r.result;
    }
    get error() {
      const r = req(this);
      if (r.readyState !== "done") throw err("InvalidStateError", "The request has not finished.");
      return r.error;
    }
    get source() {
      return req(this).source;
    }
    get transaction() {
      return req(this).transaction;
    }
    get readyState() {
      return req(this).readyState;
    }
  }
  enumerable(IDBRequest.prototype);
  defineHandlers(IDBRequest.prototype, ["success", "error"]);
  tag(IDBRequest.prototype, "IDBRequest");
  class IDBOpenDBRequest extends IDBRequest {
    constructor(token) {
      illegal(token);
      super(token);
    }
  }
  defineHandlers(IDBOpenDBRequest.prototype, ["blocked", "upgradeneeded"]);
  tag(IDBOpenDBRequest.prototype, "IDBOpenDBRequest");
  function req(o) {
    const r = R.get(o);
    if (!r) throw new TypeError("Illegal invocation");
    return r;
  }

  // --- IDBVersionChangeEvent (§4.2) ---
  const VC = new WeakMap();
  class IDBVersionChangeEvent extends Event {
    constructor(type, init = {}) {
      if (arguments.length < 1) throw new TypeError("Failed to construct 'IDBVersionChangeEvent': 1 argument required.");
      super(type, init);
      const o = init == null ? {} : init;
      const oldVersion = o.oldVersion === undefined ? 0 : Number(o.oldVersion);
      const newVersion = o.newVersion === undefined || o.newVersion === null ? null : Number(o.newVersion);
      VC.set(this, { oldVersion, newVersion });
    }
    get oldVersion() {
      const v = VC.get(this);
      if (!v) throw new TypeError("Illegal invocation");
      return v.oldVersion;
    }
    get newVersion() {
      const v = VC.get(this);
      if (!v) throw new TypeError("Illegal invocation");
      return v.newVersion;
    }
  }
  enumerable(IDBVersionChangeEvent.prototype);
  tag(IDBVersionChangeEvent.prototype, "IDBVersionChangeEvent");
  function fireVersionChange(target, type, oldVersion, newVersion) {
    return native.fire(target, new IDBVersionChangeEvent(type, { oldVersion, newVersion }));
  }

  // --- IDBDatabase (§4.4) ---
  const D = new WeakMap();
  class IDBDatabase extends EventTarget {
    constructor(token, db) {
      illegal(token);
      super();
      D.set(this, { db, version: db.version, closePending: false, closed: false, upgrade: null, txns: new Set() });
    }
    get name() {
      return conn(this).db.name;
    }
    get version() {
      return conn(this).version;
    }
    get objectStoreNames() {
      const c = conn(this);
      return makeStringList(c.closed && c.names ? c.names : Array.from(c.db.stores.keys()));
    }
    transaction(storeNames, mode = "readonly", options = {}) {
      const c = conn(this);
      if (arguments.length < 1) throw new TypeError("Failed to execute 'transaction' on 'IDBDatabase': 1 argument required.");
      if (mode !== "readonly" && mode !== "readwrite" && mode !== "versionchange")
        throw new TypeError("Failed to execute 'transaction': The provided value '" + mode + "' is not a valid enum value of type IDBTransactionMode.");
      const durability = options && options.durability !== undefined ? String(options.durability) : "default";
      if (!["default", "strict", "relaxed"].includes(durability))
        throw new TypeError("The provided value '" + durability + "' is not a valid enum value of type IDBTransactionDurability.");
      if (c.upgrade && X.get(c.upgrade).state !== "finished")
        throw err("InvalidStateError", "A version change transaction is running.");
      if (c.closePending) throw err("InvalidStateError", "The database connection is closing.");
      let names = typeof storeNames === "string" ? [storeNames] : Array.from(storeNames, String);
      names = Array.from(new Set(names)).sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
      for (const n of names) {
        if (!c.db.stores.has(n)) throw err("NotFoundError", "One of the specified object stores was not found.");
      }
      if (!names.length) throw err("InvalidAccessError", "The storeNames parameter was empty.");
      if (mode === "versionchange") throw new TypeError("Invalid mode 'versionchange'.");
      return createTxn(this, names.map((n) => c.db.stores.get(n)), mode, durability, false);
    }
    close() {
      closeConnection(this);
    }
    createObjectStore(name, options = {}) {
      const c = conn(this);
      if (arguments.length < 1) throw new TypeError("Failed to execute 'createObjectStore': 1 argument required.");
      name = String(name);
      const t = c.upgrade ? X.get(c.upgrade) : null;
      if (!t || t.state === "finished") throw err("InvalidStateError", "The database is not running a version change transaction.");
      if (t.state !== "active") throw err("TransactionInactiveError", "The transaction is not active.");
      const o = options == null ? {} : options;
      let keyPath = o.keyPath === undefined || o.keyPath === null ? null : convertKeyPath(o.keyPath);
      const autoIncrement = !!o.autoIncrement;
      if (keyPath !== null && !validKeyPath(keyPath)) throw err("SyntaxError", "The keyPath option is not a valid key path.");
      if (c.db.stores.has(name)) throw err("ConstraintError", "An object store with the specified name already exists.");
      if (autoIncrement && (keyPath === "" || Array.isArray(keyPath)))
        throw err("InvalidAccessError", "The autoIncrement option was set but the keyPath option was empty or an array.");
      const s = newStore(name, keyPath, autoIncrement);
      c.db.stores.set(name, s);
      t.scope.push(s);
      t.log.push(["cs", name, keyPath, autoIncrement]);
      return storeHandle(c.upgrade, s);
    }
    deleteObjectStore(name) {
      const c = conn(this);
      if (arguments.length < 1) throw new TypeError("1 argument required");
      name = String(name);
      const t = c.upgrade ? X.get(c.upgrade) : null;
      if (!t || t.state === "finished") throw err("InvalidStateError", "The database is not running a version change transaction.");
      if (t.state !== "active") throw err("TransactionInactiveError", "The transaction is not active.");
      const s = c.db.stores.get(name);
      if (!s) throw err("NotFoundError", "The specified object store was not found.");
      c.db.stores.delete(name);
      s.deleted = true;
      for (const ix of s.indexes.values()) ix.deleted = true;
      t.log.push(["ds", name]);
    }
  }
  enumerable(IDBDatabase.prototype);
  defineHandlers(IDBDatabase.prototype, ["abort", "close", "error", "versionchange"]);
  tag(IDBDatabase.prototype, "IDBDatabase");
  function conn(o) {
    const c = D.get(o);
    if (!c) throw new TypeError("Illegal invocation");
    return c;
  }
  // "Close a database connection" (§5.2): se cierra cuando terminan sus transacciones.
  function closeConnection(connection) {
    const c = conn(connection);
    c.closePending = true;
    finishClose(connection);
  }
  function finishClose(connection) {
    const c = conn(connection);
    if (!c.closePending || c.closed) return;
    for (const txn of c.txns) if (X.get(txn).state !== "finished") return;
    c.closed = true;
    c.names = Array.from(c.db.stores.keys());
    c.db.connections.delete(connection);
    checkWaiting(c.db);
  }

  // --- IDBTransaction (§4.9) ---
  const X = new WeakMap();
  class IDBTransaction extends EventTarget {
    constructor(token) {
      illegal(token);
      super();
    }
    get objectStoreNames() {
      const t = txn(this);
      if (t.upgrade) return makeStringList(Array.from(conn(t.db).db.stores.keys()));
      return makeStringList(t.scope.map((s) => s.name));
    }
    get mode() {
      return txn(this).mode;
    }
    get durability() {
      return txn(this).durability;
    }
    get db() {
      return txn(this).db;
    }
    get error() {
      return txn(this).error;
    }
    objectStore(name) {
      const t = txn(this);
      if (arguments.length < 1) throw new TypeError("1 argument required");
      if (t.state === "finished") throw err("InvalidStateError", "The transaction has finished.");
      name = String(name);
      const s = t.scope.find((x) => x.name === name && !x.deleted);
      if (!s) throw err("NotFoundError", "The specified object store was not found.");
      return storeHandle(this, s);
    }
    abort() {
      const t = txn(this);
      if (t.state === "committing" || t.state === "finished")
        throw err("InvalidStateError", "The transaction has already committed or aborted.");
      t.state = "inactive";
      abortTxn(this, null);
    }
    commit() {
      const t = txn(this);
      if (t.state !== "active") throw err("InvalidStateError", "The transaction is not active.");
      t.state = "committing";
      maybeCommit(this);
    }
  }
  enumerable(IDBTransaction.prototype);
  defineHandlers(IDBTransaction.prototype, ["abort", "complete", "error"]);
  tag(IDBTransaction.prototype, "IDBTransaction");
  function txn(o) {
    const t = X.get(o);
    if (!t) throw new TypeError("Illegal invocation");
    return t;
  }

  function createTxn(connection, scope, mode, durability, upgrade) {
    const c = conn(connection);
    const transaction = new IDBTransaction(TOKEN);
    const t = {
      db: connection,
      scope: scope.slice(),
      mode,
      durability,
      upgrade,
      state: "active",
      started: false,
      requests: [],
      running: false,
      error: null,
      log: [],
      snapshots: new Map(),
      handles: new Map(),
      dbSnapshot: null,
    };
    X.set(transaction, t);
    c.txns.add(transaction);
    c.db.txns.push(transaction);
    native.setParent(transaction, connection);
    if (upgrade) t.dbSnapshot = snapshotDb(c.db);
    // Cuando el control vuelve al bucle de eventos, la transacción deja de estar activa.
    queueTask(() => {
      if (t.state === "active") t.state = "inactive";
      maybeCommit(transaction);
    });
    tryStart(c.db);
    return transaction;
  }

  // ¿Se puede empezar `t`? (§2.7.2: las de lectura/escritura con scopes solapados se
  // serializan en orden de creación.)
  function canStart(db, transaction) {
    const t = X.get(transaction);
    for (const other of db.txns) {
      if (other === transaction) break;
      const o = X.get(other);
      if (o.state === "finished") continue;
      if (o.upgrade || t.upgrade) return false;
      if (o.mode === "readonly" && t.mode === "readonly") continue;
      if (o.scope.some((s) => t.scope.includes(s))) return false;
    }
    return true;
  }
  function tryStart(db) {
    for (const transaction of db.txns) {
      const t = X.get(transaction);
      if (!t.started && t.state !== "finished" && canStart(db, transaction)) {
        t.started = true;
        runNext(transaction);
      }
    }
  }

  // Guarda el estado de un store antes de su primer cambio (para el abort).
  function touch(t, store) {
    if (t.upgrade || t.snapshots.has(store)) return;
    const ix = new Map();
    for (const [n, i] of store.indexes) ix.set(n, i.records.slice());
    t.snapshots.set(store, { records: store.records.slice(), current: store.current, indexes: ix });
  }
  function snapshotDb(db) {
    const stores = [];
    for (const s of db.stores.values()) {
      const indexes = [];
      for (const i of s.indexes.values()) indexes.push([i, i.name, i.records.slice()]);
      stores.push([s, s.name, s.records.slice(), s.current, indexes]);
    }
    return { version: db.version, stores };
  }
  function restoreDb(db, snap) {
    for (const s of db.stores.values()) {
      s.deleted = true;
      for (const i of s.indexes.values()) i.deleted = true;
    }
    db.stores = new Map();
    for (const [s, name, records, current, indexes] of snap.stores) {
      s.name = name;
      s.records = records;
      s.current = current;
      s.deleted = false;
      s.indexes = new Map();
      for (const [i, iname, irecs] of indexes) {
        i.name = iname;
        i.records = irecs;
        i.deleted = false;
        s.indexes.set(iname, i);
      }
      db.stores.set(name, s);
    }
    db.version = snap.version;
  }

  // "Asynchronously execute a request" (§5.6).
  function enqueue(transaction, source, op, request) {
    const t = X.get(transaction);
    if (!request) {
      request = new IDBRequest(TOKEN);
      const r = R.get(request);
      r.source = source;
      r.transaction = transaction;
      native.setParent(request, transaction);
    } else {
      const r = R.get(request);
      r.readyState = "pending";
    }
    t.requests.push({ request, op, done: false });
    if (t.started) runNext(transaction);
    return request;
  }

  function runNext(transaction) {
    const t = X.get(transaction);
    if (t.running || t.state === "finished") return;
    const item = t.requests.find((x) => !x.done);
    if (!item) {
      maybeCommit(transaction);
      return;
    }
    t.running = true;
    queueTask(() => {
      t.running = false;
      if (t.state === "finished" || item.done) return;
      item.done = true;
      const r = R.get(item.request);
      let result, error = null;
      try {
        result = item.op();
      } catch (e) {
        error = e;
      }
      r.readyState = "done";
      if (error) {
        r.result = undefined;
        r.error = error;
        fireError(item.request);
      } else {
        r.result = result;
        r.error = null;
        fireSuccess(item.request);
      }
      if (t.state !== "finished") runNext(transaction);
    });
  }

  // El commit automático (§5.4): inactiva (o en commit) y sin requests pendientes.
  function maybeCommit(transaction) {
    const t = X.get(transaction);
    if (t.state === "finished" || t.state === "active" || !t.started || t.running) return;
    if (t.requests.some((x) => !x.done)) return;
    t.state = "committing";
    const c = conn(t.db);
    queueTask(() => {
      if (t.state === "finished") return;
      if (!persist(c.db, t.log)) {
        t.state = "inactive";
        abortTxn(transaction, err("UnknownError", "The data could not be written."));
        return;
      }
      if (c.db.version > 0) DBS.set(c.db.name, c.db);
      t.state = "finished";
      native.fire(transaction, new Event("complete", { bubbles: false, cancelable: false }));
      afterFinish(transaction, false);
    });
  }

  // "Abort a transaction" (§5.7).
  function abortTxn(transaction, error) {
    const t = X.get(transaction);
    if (t.state === "finished") return;
    const c = conn(t.db);
    if (t.upgrade) restoreDb(c.db, t.dbSnapshot);
    for (const [store, snap] of t.snapshots) {
      store.records = snap.records;
      store.current = snap.current;
      for (const [n, recs] of snap.indexes) {
        const i = store.indexes.get(n);
        if (i) i.records = recs;
      }
    }
    t.state = "finished";
    if (error !== null) t.error = error;
    const pending = t.requests.filter((x) => !x.done);
    for (const item of pending) {
      item.done = true;
      const r = R.get(item.request);
      queueTask(() => {
        r.readyState = "done";
        r.result = undefined;
        r.error = err("AbortError", "The transaction was aborted.");
        native.fire(item.request, new Event("error", { bubbles: true, cancelable: true }));
      });
    }
    queueTask(() => {
      if (t.upgrade) c.version = c.db.version;
      native.fire(transaction, new Event("abort", { bubbles: true, cancelable: false }));
      afterFinish(transaction, true);
    });
  }

  function afterFinish(transaction, aborted) {
    const t = X.get(transaction);
    const c = conn(t.db);
    const i = c.db.txns.indexOf(transaction);
    if (i >= 0) c.db.txns.splice(i, 1);
    if (t.upgrade && t.openRequest) finishUpgrade(t, aborted);
    finishClose(t.db);
    tryStart(c.db);
  }

  // --- IDBObjectStore (§4.5) ---
  const S = new WeakMap();
  function storeHandle(transaction, store) {
    const t = X.get(transaction);
    let h = t.handles.get(store);
    if (!h) {
      h = new IDBObjectStore(TOKEN);
      S.set(h, { transaction, store, indexHandles: new Map(), keyPathValue: undefined });
      t.handles.set(store, h);
    }
    return h;
  }
  function sh(o) {
    const s = S.get(o);
    if (!s) throw new TypeError("Illegal invocation");
    return s;
  }
  function checkStore(s, write) {
    if (s.store.deleted) throw err("InvalidStateError", "The object store has been deleted.");
    const t = X.get(s.transaction);
    if (t.state !== "active") throw err("TransactionInactiveError", "The transaction is not active.");
    if (write && t.mode === "readonly") throw err("ReadOnlyError", "The transaction is read-only.");
    return t;
  }
  // `[EnforceRange] unsigned long` (count).
  function toCount(v) {
    if (v === undefined) return 0;
    const n = Number(v);
    if (!Number.isFinite(n) || n < 0 || n > 4294967295) throw new TypeError("Value is outside the 'unsigned long' value range.");
    return Math.trunc(n);
  }
  function toDirection(d) {
    d = d === undefined ? "next" : String(d);
    if (!["next", "nextunique", "prev", "prevunique"].includes(d))
      throw new TypeError("The provided value '" + d + "' is not a valid enum value of type IDBCursorDirection.");
    return d;
  }
  function keyPathValue(kp) {
    return Array.isArray(kp) ? kp.slice() : kp;
  }
  // "Clone a value" (§5.8): la transacción inactiva mientras se serializa.
  function cloneForStorage(t, value) {
    const saved = t.state;
    t.state = "inactive";
    try {
      return serialize(value);
    } finally {
      t.state = saved;
    }
  }

  class IDBObjectStore {
    constructor(token) {
      illegal(token);
    }
    get name() {
      return sh(this).store.name;
    }
    set name(v) {
      const s = sh(this);
      const name = String(v);
      const t = X.get(s.transaction);
      if (s.store.deleted) throw err("InvalidStateError", "The object store has been deleted.");
      if (!t.upgrade) throw err("InvalidStateError", "Not in a version change transaction.");
      if (t.state !== "active") throw err("TransactionInactiveError", "The transaction is not active.");
      if (s.store.name === name) return;
      const db = conn(t.db).db;
      if (db.stores.has(name)) throw err("ConstraintError", "An object store with the specified name already exists.");
      db.stores.delete(s.store.name);
      t.log.push(["rs", s.store.name, name]);
      s.store.name = name;
      db.stores.set(name, s.store);
    }
    get keyPath() {
      const s = sh(this);
      if (s.keyPathValue === undefined) s.keyPathValue = keyPathValue(s.store.keyPath);
      return s.keyPathValue;
    }
    get indexNames() {
      const s = sh(this);
      return makeStringList(Array.from(s.store.indexes.keys()));
    }
    get transaction() {
      return sh(this).transaction;
    }
    get autoIncrement() {
      return sh(this).store.autoIncrement;
    }
    put(value, key) {
      return addOrPut(this, value, key, arguments.length > 1, false);
    }
    add(value, key) {
      return addOrPut(this, value, key, arguments.length > 1, true);
    }
    delete(query) {
      const s = sh(this);
      if (arguments.length < 1) throw new TypeError("1 argument required");
      const t = checkStore(s, true);
      const r = toRange(query, true);
      return enqueue(s.transaction, this, () => {
        touch(t, s.store);
        for (const rec of deleteRecords(s.store, r)) t.log.push(["del", s.store.name, keyToJson(rec.key)]);
        return undefined;
      });
    }
    clear() {
      const s = sh(this);
      const t = checkStore(s, true);
      return enqueue(s.transaction, this, () => {
        touch(t, s.store);
        s.store.records = [];
        for (const ix of s.store.indexes.values()) ix.records = [];
        t.log.push(["clear", s.store.name]);
        return undefined;
      });
    }
    get(query) {
      const s = sh(this);
      if (arguments.length < 1) throw new TypeError("1 argument required");
      checkStore(s, false);
      const r = toRange(query, true);
      return enqueue(s.transaction, this, () => {
        const rec = s.store.records.find((x) => inRange(r, x.key));
        return rec ? deserialize(rec.value) : undefined;
      });
    }
    getKey(query) {
      const s = sh(this);
      if (arguments.length < 1) throw new TypeError("1 argument required");
      checkStore(s, false);
      const r = toRange(query, true);
      return enqueue(s.transaction, this, () => {
        const rec = s.store.records.find((x) => inRange(r, x.key));
        return rec ? keyValue(rec.key) : undefined;
      });
    }
    getAll(query, count) {
      const s = sh(this);
      checkStore(s, false);
      const r = toRange(query, false);
      const n = toCount(count);
      return enqueue(s.transaction, this, () => {
        const out = [];
        for (const rec of s.store.records) {
          if (!inRange(r, rec.key)) continue;
          out.push(deserialize(rec.value));
          if (n && out.length >= n) break;
        }
        return out;
      });
    }
    getAllKeys(query, count) {
      const s = sh(this);
      checkStore(s, false);
      const r = toRange(query, false);
      const n = toCount(count);
      return enqueue(s.transaction, this, () => {
        const out = [];
        for (const rec of s.store.records) {
          if (!inRange(r, rec.key)) continue;
          out.push(keyValue(rec.key));
          if (n && out.length >= n) break;
        }
        return out;
      });
    }
    count(query) {
      const s = sh(this);
      checkStore(s, false);
      const r = toRange(query, false);
      return enqueue(s.transaction, this, () => s.store.records.filter((x) => inRange(r, x.key)).length);
    }
    openCursor(query, direction) {
      return openCursor(this, sh(this), null, query, direction, false);
    }
    openKeyCursor(query, direction) {
      return openCursor(this, sh(this), null, query, direction, true);
    }
    index(name) {
      const s = sh(this);
      if (arguments.length < 1) throw new TypeError("1 argument required");
      name = String(name);
      if (s.store.deleted) throw err("InvalidStateError", "The object store has been deleted.");
      if (X.get(s.transaction).state === "finished") throw err("InvalidStateError", "The transaction has finished.");
      const ix = s.store.indexes.get(name);
      if (!ix || ix.deleted) throw err("NotFoundError", "The specified index was not found.");
      return indexHandle(this, ix);
    }
    createIndex(name, keyPath, options = {}) {
      const s = sh(this);
      if (arguments.length < 2) throw new TypeError("2 arguments required");
      name = String(name);
      const t = X.get(s.transaction);
      if (!t.upgrade) throw err("InvalidStateError", "Not in a version change transaction.");
      if (s.store.deleted) throw err("InvalidStateError", "The object store has been deleted.");
      if (t.state !== "active") throw err("TransactionInactiveError", "The transaction is not active.");
      if (s.store.indexes.has(name)) throw err("ConstraintError", "An index with the specified name already exists.");
      keyPath = convertKeyPath(keyPath);
      if (!validKeyPath(keyPath)) throw err("SyntaxError", "The keyPath argument contains an invalid key path.");
      const o = options == null ? {} : options;
      const unique = !!o.unique;
      const multiEntry = !!o.multiEntry;
      if (Array.isArray(keyPath) && multiEntry)
        throw err("InvalidAccessError", "The keyPath argument was an array and the multiEntry option is true.");
      const ix = newIndex(s.store, name, keyPath, unique, multiEntry);
      s.store.indexes.set(name, ix);
      t.log.push(["ci", s.store.name, name, keyPath, unique, multiEntry]);
      if (!populateIndex(ix)) {
        // Los registros violan `unique`: la transacción se aborta (§4.5, createIndex paso 14).
        const transaction = s.transaction;
        queueTask(() => abortTxn(transaction, err("ConstraintError", "Unable to add key to index: uniqueness requirement violated.")));
      }
      return indexHandle(this, ix);
    }
    deleteIndex(name) {
      const s = sh(this);
      if (arguments.length < 1) throw new TypeError("1 argument required");
      name = String(name);
      const t = X.get(s.transaction);
      if (!t.upgrade) throw err("InvalidStateError", "Not in a version change transaction.");
      if (s.store.deleted) throw err("InvalidStateError", "The object store has been deleted.");
      if (t.state !== "active") throw err("TransactionInactiveError", "The transaction is not active.");
      const ix = s.store.indexes.get(name);
      if (!ix) throw err("NotFoundError", "The specified index was not found.");
      s.store.indexes.delete(name);
      ix.deleted = true;
      t.log.push(["di", s.store.name, name]);
    }
  }
  enumerable(IDBObjectStore.prototype);
  tag(IDBObjectStore.prototype, "IDBObjectStore");

  // add()/put() (§4.5, "add or put").
  function addOrPut(handle, value, key, hasKey, noOverwrite) {
    const s = sh(handle);
    const t = checkStore(s, true);
    const store = s.store;
    if (hasKey && key !== undefined && store.keyPath !== null)
      throw err("DataError", "The object store uses in-line keys and the key parameter was provided.");
    if ((!hasKey || key === undefined) && store.keyPath === null && !store.autoIncrement)
      throw err("DataError", "The object store uses out-of-line keys and has no key generator and the key parameter was not provided.");
    let k = null;
    if (hasKey && key !== undefined) k = toKeyOrThrow(key);
    const tree = cloneForStorage(t, value);
    let clone = null;
    if (store.keyPath !== null) {
      clone = deserialize(tree);
      const kr = extractKey(clone, store.keyPath, false);
      if (kr === null) throw err("DataError", "Evaluating the object store's key path yielded a value that is not a valid key.");
      if (kr === FAIL) {
        if (!store.autoIncrement) throw err("DataError", "Evaluating the object store's key path did not yield a value.");
        if (!canInject(clone, store.keyPath)) throw err("DataError", "A generated key could not be inserted into the value.");
      } else k = kr;
    }
    return enqueue(s.transaction, handle, () => {
      touch(t, store);
      let finalKey = k;
      let finalTree = tree;
      if (store.autoIncrement) {
        if (finalKey === null) {
          if (store.current > 9007199254740992) throw err("ConstraintError", "The key generator has reached its maximum value.");
          finalKey = { t: 1, v: store.current };
          store.current++;
          t.log.push(["gen", store.name, store.current]);
          if (store.keyPath !== null) {
            const v = deserialize(tree);
            inject(v, finalKey, store.keyPath);
            finalTree = serialize(v);
          }
        } else if (finalKey.t === 1 && finalKey.v >= store.current) {
          store.current = Math.min(Math.floor(finalKey.v) + 1, 9007199254740993);
          t.log.push(["gen", store.name, store.current]);
        }
      }
      const e = storeRecord(store, finalKey, finalTree, noOverwrite);
      if (e) throw e;
      t.log.push(["put", store.name, keyToJson(finalKey), finalTree]);
      return keyValue(finalKey);
    });
  }

  // --- IDBIndex (§4.6) ---
  const I = new WeakMap();
  function indexHandle(storeH, index) {
    const s = sh(storeH);
    let h = s.indexHandles.get(index);
    if (!h) {
      h = new IDBIndex(TOKEN);
      I.set(h, { storeHandle: storeH, index, keyPathValue: undefined });
      s.indexHandles.set(index, h);
    }
    return h;
  }
  function ih(o) {
    const x = I.get(o);
    if (!x) throw new TypeError("Illegal invocation");
    return x;
  }
  function checkIndex(x) {
    const s = sh(x.storeHandle);
    if (x.index.deleted || s.store.deleted) throw err("InvalidStateError", "The index or its object store has been deleted.");
    const t = X.get(s.transaction);
    if (t.state !== "active") throw err("TransactionInactiveError", "The transaction is not active.");
    return s;
  }
  function storeValueOf(store, pk) {
    const f = findRecord(store, pk);
    return f ? deserialize(f.r.value) : undefined;
  }
  class IDBIndex {
    constructor(token) {
      illegal(token);
    }
    get name() {
      return ih(this).index.name;
    }
    set name(v) {
      const x = ih(this);
      const name = String(v);
      const s = sh(x.storeHandle);
      const t = X.get(s.transaction);
      if (!t.upgrade) throw err("InvalidStateError", "Not in a version change transaction.");
      if (t.state !== "active") throw err("TransactionInactiveError", "The transaction is not active.");
      if (x.index.deleted || s.store.deleted) throw err("InvalidStateError", "The index has been deleted.");
      if (x.index.name === name) return;
      if (s.store.indexes.has(name)) throw err("ConstraintError", "An index with the specified name already exists.");
      s.store.indexes.delete(x.index.name);
      t.log.push(["ri", s.store.name, x.index.name, name]);
      x.index.name = name;
      s.store.indexes.set(name, x.index);
    }
    get objectStore() {
      return ih(this).storeHandle;
    }
    get keyPath() {
      const x = ih(this);
      if (x.keyPathValue === undefined) x.keyPathValue = keyPathValue(x.index.keyPath);
      return x.keyPathValue;
    }
    get multiEntry() {
      return ih(this).index.multiEntry;
    }
    get unique() {
      return ih(this).index.unique;
    }
    get(query) {
      const x = ih(this);
      if (arguments.length < 1) throw new TypeError("1 argument required");
      const s = checkIndex(x);
      const r = toRange(query, true);
      return enqueue(s.transaction, this, () => {
        const rec = x.index.records.find((ir) => inRange(r, ir.key));
        return rec ? storeValueOf(s.store, rec.pk) : undefined;
      });
    }
    getKey(query) {
      const x = ih(this);
      if (arguments.length < 1) throw new TypeError("1 argument required");
      const s = checkIndex(x);
      const r = toRange(query, true);
      return enqueue(s.transaction, this, () => {
        const rec = x.index.records.find((ir) => inRange(r, ir.key));
        return rec ? keyValue(rec.pk) : undefined;
      });
    }
    getAll(query, count) {
      const x = ih(this);
      const s = checkIndex(x);
      const r = toRange(query, false);
      const n = toCount(count);
      return enqueue(s.transaction, this, () => {
        const out = [];
        for (const ir of x.index.records) {
          if (!inRange(r, ir.key)) continue;
          out.push(storeValueOf(s.store, ir.pk));
          if (n && out.length >= n) break;
        }
        return out;
      });
    }
    getAllKeys(query, count) {
      const x = ih(this);
      const s = checkIndex(x);
      const r = toRange(query, false);
      const n = toCount(count);
      return enqueue(s.transaction, this, () => {
        const out = [];
        for (const ir of x.index.records) {
          if (!inRange(r, ir.key)) continue;
          out.push(keyValue(ir.pk));
          if (n && out.length >= n) break;
        }
        return out;
      });
    }
    count(query) {
      const x = ih(this);
      const s = checkIndex(x);
      const r = toRange(query, false);
      return enqueue(s.transaction, this, () => x.index.records.filter((ir) => inRange(r, ir.key)).length);
    }
    openCursor(query, direction) {
      const x = ih(this);
      return openCursor(this, sh(x.storeHandle), x, query, direction, false);
    }
    openKeyCursor(query, direction) {
      const x = ih(this);
      return openCursor(this, sh(x.storeHandle), x, query, direction, true);
    }
  }
  enumerable(IDBIndex.prototype);
  tag(IDBIndex.prototype, "IDBIndex");

  // --- IDBCursor / IDBCursorWithValue (§4.8) ---
  const CU = new WeakMap();
  class IDBCursor {
    constructor(token) {
      illegal(token);
    }
    get source() {
      return cur(this).source;
    }
    get direction() {
      return cur(this).direction;
    }
    get key() {
      const c = cur(this);
      if (c.key === undefined) return undefined;
      if (c.keyValue === undefined) c.keyValue = keyValue(c.key);
      return c.keyValue;
    }
    get primaryKey() {
      const c = cur(this);
      const pk = c.index ? c.objectStorePosition : c.key;
      if (pk === undefined || pk === null) return undefined;
      if (c.pkValue === undefined) c.pkValue = keyValue(pk);
      return c.pkValue;
    }
    get request() {
      return cur(this).request;
    }
    advance(count) {
      const c = cur(this);
      const n = Number(count);
      if (!Number.isFinite(n) || n < 0 || n > 4294967295) throw new TypeError("Value is outside the 'unsigned long' value range.");
      if (Math.trunc(n) === 0) throw new TypeError("Failed to execute 'advance' on 'IDBCursor': A count argument with value 0 (zero) was supplied, must be greater than 0.");
      checkCursor(c);
      c.gotValue = false;
      enqueue(c.transaction, this, () => iterate(c, null, null, Math.trunc(n)), c.request);
    }
    continue(key) {
      const c = cur(this);
      checkCursor(c);
      let k = null;
      if (key !== undefined) {
        k = toKeyOrThrow(key);
        const d = cmp(k, c.position);
        if ((c.direction === "next" || c.direction === "nextunique") && d <= 0)
          throw err("DataError", "The parameter is less than or equal to this cursor's position.");
        if ((c.direction === "prev" || c.direction === "prevunique") && d >= 0)
          throw err("DataError", "The parameter is greater than or equal to this cursor's position.");
      }
      c.gotValue = false;
      enqueue(c.transaction, this, () => iterate(c, k, null, 1), c.request);
    }
    continuePrimaryKey(key, primaryKey) {
      const c = cur(this);
      if (arguments.length < 2) throw new TypeError("2 arguments required");
      const t = X.get(c.transaction);
      if (t.state !== "active") throw err("TransactionInactiveError", "The transaction is not active.");
      if (sourceDeleted(c)) throw err("InvalidStateError", "The cursor's source has been deleted.");
      if (!c.index) throw err("InvalidAccessError", "The cursor's source is not an index.");
      if (c.direction !== "next" && c.direction !== "prev")
        throw err("InvalidAccessError", "The cursor's direction is not 'next' or 'prev'.");
      if (!c.gotValue) throw err("InvalidStateError", "The cursor is being iterated or has iterated past its end.");
      const k = toKeyOrThrow(key);
      const pk = toKeyOrThrow(primaryKey);
      const d = cmp(k, c.position);
      if (c.direction === "next" && (d < 0 || (d === 0 && cmp(pk, c.objectStorePosition) <= 0)))
        throw err("DataError", "The parameters are not after the cursor's position.");
      if (c.direction === "prev" && (d > 0 || (d === 0 && cmp(pk, c.objectStorePosition) >= 0)))
        throw err("DataError", "The parameters are not before the cursor's position.");
      c.gotValue = false;
      enqueue(c.transaction, this, () => iterate(c, k, pk, 1), c.request);
    }
    update(value) {
      const c = cur(this);
      if (arguments.length < 1) throw new TypeError("1 argument required");
      const t = X.get(c.transaction);
      if (t.state !== "active") throw err("TransactionInactiveError", "The transaction is not active.");
      if (t.mode === "readonly") throw err("ReadOnlyError", "The transaction is read-only.");
      if (sourceDeleted(c)) throw err("InvalidStateError", "The cursor's source has been deleted.");
      if (!c.gotValue || c.keyOnly) throw err("InvalidStateError", "The cursor is being iterated or has iterated past its end.");
      const tree = cloneForStorage(t, value);
      const pk = c.index ? c.objectStorePosition : c.key;
      if (c.store.keyPath !== null) {
        const kr = extractKey(deserialize(tree), c.store.keyPath, false);
        if (kr === FAIL || kr === null || cmp(kr, pk) !== 0)
          throw err("DataError", "The effective object store of this cursor uses in-line keys and evaluating the key path of the value parameter results in a different value than the cursor's effective key.");
      }
      const store = c.store;
      return enqueue(c.transaction, this, () => {
        touch(t, store);
        const e = storeRecord(store, pk, tree, false);
        if (e) throw e;
        t.log.push(["put", store.name, keyToJson(pk), tree]);
        return keyValue(pk);
      });
    }
    delete() {
      const c = cur(this);
      const t = X.get(c.transaction);
      if (t.state !== "active") throw err("TransactionInactiveError", "The transaction is not active.");
      if (t.mode === "readonly") throw err("ReadOnlyError", "The transaction is read-only.");
      if (sourceDeleted(c)) throw err("InvalidStateError", "The cursor's source has been deleted.");
      if (!c.gotValue || c.keyOnly) throw err("InvalidStateError", "The cursor is being iterated or has iterated past its end.");
      const pk = c.index ? c.objectStorePosition : c.key;
      const store = c.store;
      return enqueue(c.transaction, this, () => {
        touch(t, store);
        deleteRecords(store, { lower: pk, upper: pk, lowerOpen: false, upperOpen: false });
        t.log.push(["del", store.name, keyToJson(pk)]);
        return undefined;
      });
    }
  }
  enumerable(IDBCursor.prototype);
  tag(IDBCursor.prototype, "IDBCursor");
  class IDBCursorWithValue extends IDBCursor {
    constructor(token) {
      illegal(token);
      super(token);
    }
    get value() {
      const c = cur(this);
      return c.value;
    }
  }
  enumerable(IDBCursorWithValue.prototype);
  tag(IDBCursorWithValue.prototype, "IDBCursorWithValue");
  function cur(o) {
    const c = CU.get(o);
    if (!c) throw new TypeError("Illegal invocation");
    return c;
  }
  function sourceDeleted(c) {
    return c.store.deleted || (c.index && c.index.deleted);
  }
  function checkCursor(c) {
    const t = X.get(c.transaction);
    if (t.state !== "active") throw err("TransactionInactiveError", "The transaction is not active.");
    if (sourceDeleted(c)) throw err("InvalidStateError", "The cursor's source has been deleted.");
    if (!c.gotValue) throw err("InvalidStateError", "The cursor is being iterated or has iterated past its end.");
  }

  function openCursor(handle, s, x, query, direction, keyOnly) {
    if (x) checkIndex(x);
    else checkStore(s, false);
    const r = toRange(query, false);
    const dir = toDirection(direction);
    const cursor = keyOnly || false ? new IDBCursor(TOKEN) : new IDBCursorWithValue(TOKEN);
    const c = {
      source: handle,
      transaction: s.transaction,
      store: s.store,
      index: x ? x.index : null,
      direction: dir,
      range: r,
      position: undefined,
      objectStorePosition: undefined,
      key: undefined,
      value: undefined,
      gotValue: false,
      keyOnly,
      request: null,
      keyValue: undefined,
      pkValue: undefined,
    };
    CU.set(cursor, c);
    const request = enqueue(s.transaction, handle, () => iterate(c, null, null, 1));
    c.request = request;
    R.get(request).cursor = cursor;
    return request;
  }

  // "Iterate a cursor" (§5.9.2): el cursor (o `null` al final).
  function iterate(c, key, primaryKey, count) {
    const forward = c.direction === "next" || c.direction === "nextunique";
    const unique = c.direction === "nextunique" || c.direction === "prevunique";
    const records = c.index ? c.index.records : c.store.records;
    const pkOf = (rec) => (c.index ? rec.pk : rec.key);
    let pos = c.position;
    let osPos = c.objectStorePosition;
    let found = null;
    for (let step = 0; step < count; step++) {
      found = null;
      if (forward) {
        for (let i = 0; i < records.length; i++) {
          const rec = records[i];
          if (!inRange(c.range, rec.key)) continue;
          if (key !== null) {
            const d = cmp(rec.key, key);
            if (d < 0) continue;
            if (primaryKey !== null && d === 0 && cmp(pkOf(rec), primaryKey) < 0) continue;
          }
          if (pos !== undefined && pos !== null) {
            const d = cmp(rec.key, pos);
            if (unique || !c.index) {
              if (d <= 0) continue;
            } else if (d < 0 || (d === 0 && cmp(rec.pk, osPos) <= 0)) continue;
          }
          found = rec;
          break;
        }
      } else {
        for (let i = records.length - 1; i >= 0; i--) {
          const rec = records[i];
          if (!inRange(c.range, rec.key)) continue;
          if (key !== null) {
            const d = cmp(rec.key, key);
            if (d > 0) continue;
            if (primaryKey !== null && d === 0 && cmp(pkOf(rec), primaryKey) > 0) continue;
          }
          if (pos !== undefined && pos !== null) {
            const d = cmp(rec.key, pos);
            if (unique || !c.index) {
              if (d >= 0) continue;
            } else if (d > 0 || (d === 0 && cmp(rec.pk, osPos) >= 0)) continue;
          }
          found = rec;
          // prevunique: el primer registro (menor clave primaria) con esa clave.
          if (unique && c.index) {
            let j = i;
            while (j > 0 && cmp(records[j - 1].key, rec.key) === 0) j--;
            found = records[j];
          }
          break;
        }
      }
      if (!found) break;
      pos = found.key;
      osPos = c.index ? found.pk : undefined;
      key = null;
      primaryKey = null;
    }
    if (!found) {
      c.key = undefined;
      c.value = undefined;
      c.position = undefined;
      c.objectStorePosition = undefined;
      c.keyValue = undefined;
      c.pkValue = undefined;
      return null;
    }
    c.position = pos;
    c.objectStorePosition = osPos;
    c.key = found.key;
    c.keyValue = undefined;
    c.pkValue = undefined;
    if (!c.keyOnly) c.value = c.index ? storeValueOf(c.store, found.pk) : deserialize(found.value);
    c.gotValue = true;
    return R.get(c.request).cursor;
  }

  // --- IDBFactory (§4.3) ---
  function connectionQueue(name) {
    let db = DBS.get(name);
    if (!db) {
      db = loadDb(name);
    }
    return db;
  }
  // Las aperturas y borrados de un nombre se procesan en orden (§2.1 "connection queue").
  const QUEUES = new Map();
  function enqueueOpen(name, step) {
    let q = QUEUES.get(name);
    if (!q) {
      q = { items: [], busy: false };
      QUEUES.set(name, q);
    }
    q.items.push(step);
    pumpQueue(name);
  }
  function pumpQueue(name) {
    const q = QUEUES.get(name);
    if (!q || q.busy || !q.items.length) return;
    q.busy = true;
    const step = q.items.shift();
    queueTask(() =>
      step(() => {
        q.busy = false;
        pumpQueue(name);
      }),
    );
  }
  function checkWaiting(db) {
    const w = db.waiting;
    if (!w) return;
    for (const c of db.connections) if (c !== w.except && !D.get(c).closed) return;
    db.waiting = null;
    w.resume();
  }
  // Manda `versionchange` a las demás conexiones y espera a que cierren (§5.1 paso 10).
  function waitForOthers(db, except, request, oldVersion, newVersion, resume) {
    const others = Array.from(db.connections).filter((c) => c !== except && !D.get(c).closed);
    for (const c of others) {
      if (!D.get(c).closePending) fireVersionChange(c, "versionchange", oldVersion, newVersion);
    }
    const open = others.filter((c) => !D.get(c).closed);
    if (open.length) {
      fireVersionChange(request, "blocked", oldVersion, newVersion);
      db.waiting = { except, resume };
      checkWaiting(db);
      return;
    }
    resume();
  }

  function finishRequest(request, result, error) {
    const r = R.get(request);
    r.readyState = "done";
    r.result = result;
    r.error = error;
  }

  // "Open a database connection" (§5.1).
  function openSteps(request, name, version, done) {
    let db = connectionQueue(name);
    let ver = version;
    if (ver === undefined) ver = db ? db.version : 1;
    if (!db) db = newDb(name);
    if (db.version > ver) {
      finishRequest(request, undefined, err("VersionError", "The requested version (" + ver + ") is less than the existing version (" + db.version + ")."));
      native.fire(request, new Event("error", { bubbles: true, cancelable: true }));
      done();
      return;
    }
    const connection = new IDBDatabase(TOKEN, db);
    db.connections.add(connection);
    if (db.version < ver) {
      const oldVersion = db.version;
      waitForOthers(db, connection, request, oldVersion, ver, () => {
        if (D.get(connection).closePending) {
          db.connections.delete(connection);
          finishRequest(request, undefined, err("AbortError", "The connection was closed."));
          native.fire(request, new Event("error", { bubbles: true, cancelable: true }));
          done();
          return;
        }
        upgrade(request, connection, db, oldVersion, ver, done);
      });
      return;
    }
    finishRequest(request, connection, null);
    native.fire(request, new Event("success", { bubbles: false, cancelable: false }));
    done();
  }

  // "Upgrade a database" (§5.10).
  function upgrade(request, connection, db, oldVersion, version, done) {
    const c = D.get(connection);
    const transaction = createTxn(connection, Array.from(db.stores.values()), "versionchange", "default", true);
    const t = X.get(transaction);
    t.openRequest = { request, done, oldVersion };
    c.upgrade = transaction;
    db.version = version;
    c.version = version;
    t.log.push(["ver", version]);
    native.setParent(request, null);
    const r = R.get(request);
    r.result = connection;
    r.transaction = transaction;
    r.readyState = "done";
    t.state = "active";
    const flags = fireVersionChange(request, "upgradeneeded", oldVersion, version);
    if (t.state === "active") t.state = "inactive";
    if (flags & FIRE_THREW) abortTxn(transaction, err("AbortError", "A listener threw an exception."));
  }

  function finishUpgrade(t, aborted) {
    const { request, done, oldVersion } = t.openRequest;
    const connection = t.db;
    const c = D.get(connection);
    const r = R.get(request);
    r.transaction = null;
    if (aborted) {
      c.version = oldVersion;
      c.db.version = oldVersion;
      if (oldVersion === 0) DBS.delete(c.db.name);
      c.closePending = true;
      c.closed = true;
      c.names = Array.from(c.db.stores.keys());
      c.db.connections.delete(connection);
      queueTask(() => {
        finishRequest(request, undefined, err("AbortError", "The version change transaction was aborted."));
        native.fire(request, new Event("error", { bubbles: true, cancelable: true }));
        done();
      });
      return;
    }
    DBS.set(c.db.name, c.db);
    queueTask(() => {
      if (c.closePending) {
        finishRequest(request, undefined, err("AbortError", "The connection was closed."));
        native.fire(request, new Event("error", { bubbles: true, cancelable: true }));
      } else {
        finishRequest(request, connection, null);
        native.fire(request, new Event("success", { bubbles: false, cancelable: false }));
      }
      done();
    });
  }

  // "Delete a database" (§5.3).
  function deleteSteps(request, name, done) {
    const db = connectionQueue(name);
    if (!db) {
      finishRequest(request, undefined, null);
      fireVersionChange(request, "success", 0, null);
      done();
      return;
    }
    const oldVersion = db.version;
    waitForOthers(db, null, request, oldVersion, null, () => {
      DBS.delete(name);
      native.remove(name);
      for (const s of db.stores.values()) {
        s.deleted = true;
        for (const i of s.indexes.values()) i.deleted = true;
      }
      finishRequest(request, undefined, null);
      fireVersionChange(request, "success", oldVersion, null);
      done();
    });
  }

  // `[EnforceRange] unsigned long long` (version).
  function toVersion(v) {
    const n = Number(v);
    if (!Number.isFinite(n) || n < 0 || n > 9007199254740991) throw new TypeError("Value is outside the 'unsigned long long' value range.");
    const t = Math.trunc(n);
    if (t === 0) throw new TypeError("The version provided must not be 0.");
    return t;
  }

  class IDBFactory {
    constructor(token) {
      illegal(token);
    }
    open(name, version) {
      if (arguments.length < 1) throw new TypeError("Failed to execute 'open' on 'IDBFactory': 1 argument required.");
      name = String(name);
      const ver = version === undefined ? undefined : toVersion(version);
      const request = new IDBOpenDBRequest(TOKEN);
      enqueueOpen(name, (done) => openSteps(request, name, ver, done));
      return request;
    }
    deleteDatabase(name) {
      if (arguments.length < 1) throw new TypeError("Failed to execute 'deleteDatabase' on 'IDBFactory': 1 argument required.");
      name = String(name);
      const request = new IDBOpenDBRequest(TOKEN);
      enqueueOpen(name, (done) => deleteSteps(request, name, done));
      return request;
    }
    databases() {
      return new P((resolve) => {
        queueTask(() => {
          const names = new Set(native.names());
          for (const [n, db] of DBS) if (db.version > 0) names.add(n);
          const out = [];
          for (const n of Array.from(names).sort()) {
            const db = DBS.get(n) || loadDb(n);
            if (db && db.version > 0) out.push({ name: n, version: db.version });
          }
          resolve(out);
        });
      });
    }
    cmp(first, second) {
      if (arguments.length < 2) throw new TypeError("Failed to execute 'cmp' on 'IDBFactory': 2 arguments required.");
      return cmp(toKeyOrThrow(first), toKeyOrThrow(second));
    }
  }
  enumerable(IDBFactory.prototype);
  tag(IDBFactory.prototype, "IDBFactory");

  // --- StorageManager (Storage Standard §4) ---
  let persisted = false;
  class StorageManager {
    constructor(token) {
      illegal(token);
    }
    persisted() {
      return P.resolve(persisted);
    }
    persist() {
      persisted = true;
      return P.resolve(true);
    }
    estimate() {
      const [usage, quota] = native.estimate();
      return P.resolve({ usage, quota });
    }
  }
  enumerable(StorageManager.prototype);
  tag(StorageManager.prototype, "StorageManager");

  const factory = new IDBFactory(TOKEN);
  const storage = new StorageManager(TOKEN);
  for (const [name, cls] of [
    ["IDBFactory", IDBFactory],
    ["IDBRequest", IDBRequest],
    ["IDBOpenDBRequest", IDBOpenDBRequest],
    ["IDBDatabase", IDBDatabase],
    ["IDBTransaction", IDBTransaction],
    ["IDBObjectStore", IDBObjectStore],
    ["IDBIndex", IDBIndex],
    ["IDBKeyRange", IDBKeyRange],
    ["IDBCursor", IDBCursor],
    ["IDBCursorWithValue", IDBCursorWithValue],
    ["IDBVersionChangeEvent", IDBVersionChangeEvent],
    ["StorageManager", StorageManager],
  ]) {
    define(global, name, cls);
  }
  if (typeof global.DOMStringList !== "function") define(global, "DOMStringList", DOMStringList);
  Object.defineProperty(global, "indexedDB", {
    get() {
      return factory;
    },
    enumerable: true,
    configurable: true,
  });
  const navProto = global.Navigator && global.Navigator.prototype;
  if (navProto) {
    Object.defineProperty(navProto, "storage", {
      get() {
        return storage;
      },
      enumerable: true,
      configurable: true,
    });
  }
});
