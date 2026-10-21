//! IndexedDB 3.0 (`bf_web::storage`): factory, upgrade/versionchange/blocked, transacciones
//! (auto-commit, abort, orden y aislamiento), object stores, índices, cursores, key ranges,
//! claves y valores, y la persistencia en disco al reabrir.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use bf_js::host::DefaultHooks;
use bf_web::{DocumentOwner, Runtime, Url, WebContext, WebContextHandle};

struct Page {
    rt: Runtime,
    ctx: WebContextHandle,
}

const PRELUDE: &str = r#"
var result = null, done = false, log = [];
function finish(v) { if (!done) { result = String(v); done = true; } }
function fail(e) { finish('ERR ' + (e && e.name) + ': ' + (e && e.message)); }
function req(r) { return new Promise((res, rej) => { r.onsuccess = () => res(r.result); r.onerror = (e) => { e.preventDefault(); rej(r.error); }; }); }
function openDb(name, version, upgrade) {
  var r = indexedDB.open(name, version);
  r.onupgradeneeded = (e) => { if (upgrade) upgrade(r.result, r.transaction, e); };
  return req(r);
}
function txDone(t) { return new Promise((res, rej) => { t.oncomplete = () => res('complete'); t.onabort = () => rej(t.error); }); }
var uniq = 0;
function fresh() { return 'db' + (++uniq) + '_' + Math.random(); }
"#;

impl Page {
    fn new(dir: Option<PathBuf>) -> Page {
        let mut rt = Runtime::new(Box::new(DefaultHooks));
        let (doc, _) =
            bf_html::parse_document("<!DOCTYPE html><p>x", &bf_html::ParseOptions::default());
        let realm = rt.main_realm();
        let url = Url::parse("https://example.com/app.html", None).expect("url");
        let ctx = bf_web::install(
            &mut rt.agent,
            realm,
            WebContext::new(DocumentOwner::Document(Box::new(doc)), url),
        );
        bf_web::storage::set_storage_dir(&mut rt.agent, realm, dir);
        let mut p = Page { rt, ctx };
        p.exec(PRELUDE);
        p
    }

    fn exec(&mut self, src: &str) {
        let realm = self.ctx.realm;
        if let Err(e) = self.rt.eval_script(realm, src, "test.js") {
            panic!("error: {e}\n{}", e.stack);
        }
    }

    fn eval_string(&mut self, src: &str) -> String {
        let realm = self.ctx.realm;
        match self
            .rt
            .eval_script(realm, &format!("String({src})"), "test.js")
        {
            Ok(v) => self.rt.convert::<String>(&v).expect("string"),
            Err(e) => panic!("error: {e}\n{}", e.stack),
        }
    }

    fn run(&mut self, src: &str) -> String {
        self.exec("done = false; result = null; log = [];");
        let code = format!("(async () => {{ {src} }})().catch(fail);");
        self.exec(&code);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let _ = self.rt.run_until_idle();
            if self.eval_string("done") == "true" {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "tiempo agotado; log = {}",
                self.eval_string("JSON.stringify(log)")
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        self.eval_string("result")
    }
}

macro_rules! idb {
    ($name:ident, $src:expr, $expected:expr) => {
        #[test]
        fn $name() {
            assert_eq!(Page::new(None).run($src), $expected);
        }
    };
}

// --- Factory y apertura ---

idb!(
    open_creates_v1,
    "var db = await openDb(fresh()); finish(db.version + ' ' + (db instanceof IDBDatabase) + ' ' + db.objectStoreNames.length);",
    "1 true 0"
);
idb!(
    upgrade_event_versions,
    "var n = fresh(); var r = indexedDB.open(n, 3); r.onupgradeneeded = (e) => log.push(e.oldVersion + '->' + e.newVersion + ' ' + (e instanceof IDBVersionChangeEvent) + ' ' + r.transaction.mode); var db = await req(r); finish(log.join() + ' ' + db.version);",
    "0->3 true versionchange 3"
);
idb!(
    open_request_props,
    "var r = indexedDB.open(fresh()); finish([r instanceof IDBOpenDBRequest, r.readyState, r.source, r.transaction].join());",
    "true,pending,,"
);
idb!(
    result_before_done_throws,
    "var r = indexedDB.open(fresh()); try { r.result; finish('no'); } catch (e) { finish(e.name); }",
    "InvalidStateError"
);
idb!(
    version_error,
    "var n = fresh(); var db = await openDb(n, 5); db.close(); try { await openDb(n, 2); finish('no'); } catch (e) { finish(e.name); }",
    "VersionError"
);
idb!(
    open_existing_no_upgrade,
    "var n = fresh(); (await openDb(n, 2, (db) => db.createObjectStore('s'))).close(); var called = false; var db = await openDb(n, undefined, () => called = true); finish(db.version + ' ' + called + ' ' + db.objectStoreNames[0]);",
    "2 false s"
);
idb!(
    bad_version_type_error,
    "try { indexedDB.open('x', 0); finish('no'); } catch (e) { finish(e.name); }",
    "TypeError"
);
idb!(
    cmp_keys,
    "finish([indexedDB.cmp(1, 2), indexedDB.cmp('a', 1), indexedDB.cmp([1], 'z'), indexedDB.cmp(new Date(5), 9), indexedDB.cmp([1, 2], [1, 2]), indexedDB.cmp(new Uint8Array([1]), 'z')].join());",
    "-1,1,1,1,0,1"
);
idb!(
    cmp_invalid_key,
    "try { indexedDB.cmp({}, 1); finish('no'); } catch (e) { finish(e.name); }",
    "DataError"
);
idb!(
    delete_database,
    "var n = fresh(); (await openDb(n, 4)).close(); var r = indexedDB.deleteDatabase(n); r.addEventListener('success', (e) => log.push(e.oldVersion + ',' + e.newVersion)); await req(r); var db = await openDb(n); finish(log.join() + ' ' + db.version);",
    "4,null 1"
);
idb!(
    databases_list,
    "var n = fresh(); (await openDb(n, 7)).close(); var list = await indexedDB.databases(); var e = list.find((x) => x.name === n); finish(e.version);",
    "7"
);
idb!(
    versionchange_and_blocked,
    "var n = fresh(); var db1 = await openDb(n, 1); db1.onversionchange = (e) => { log.push('vc ' + e.oldVersion + '->' + e.newVersion); setTimeout(() => db1.close(), 5); }; var r = indexedDB.open(n, 2); r.onblocked = (e) => log.push('blocked'); var db2 = await req(r); finish(log.join(', ') + ' ' + db2.version);",
    "vc 1->2, blocked 2"
);
idb!(
    versionchange_close_immediately,
    "var n = fresh(); var db1 = await openDb(n, 1); db1.onversionchange = () => db1.close(); var r = indexedDB.open(n, 2); r.onblocked = () => log.push('blocked'); await req(r); finish('[' + log.join() + ']');",
    "[]"
);
idb!(
    upgrade_abort_reverts,
    "var n = fresh(); (await openDb(n, 1, (db) => db.createObjectStore('a'))).close(); var r = indexedDB.open(n, 2); r.onupgradeneeded = () => { r.result.createObjectStore('b'); r.transaction.abort(); }; try { await req(r); } catch (e) { log.push(e.name); } var db = await openDb(n); finish(log.join() + ' ' + db.version + ' ' + Array.from(db.objectStoreNames).join('|'));",
    "AbortError 1 a"
);

// --- Object stores ---

idb!(
    create_store_props,
    "await openDb(fresh(), 1, (db) => { var s = db.createObjectStore('s', {keyPath: 'id', autoIncrement: true}); finish([s.name, s.keyPath, s.autoIncrement, s instanceof IDBObjectStore, db.objectStoreNames.contains('s')].join()); });",
    "s,id,true,true,true"
);
idb!(
    create_store_outside_upgrade,
    "var db = await openDb(fresh()); try { db.createObjectStore('x'); finish('no'); } catch (e) { finish(e.name); }",
    "InvalidStateError"
);
idb!(
    create_store_duplicate,
    "await openDb(fresh(), 1, (db) => { db.createObjectStore('s'); try { db.createObjectStore('s'); } catch (e) { finish(e.name); } });",
    "ConstraintError"
);
idb!(
    create_store_bad_keypath,
    "await openDb(fresh(), 1, (db) => { try { db.createObjectStore('s', {keyPath: 'a b'}); } catch (e) { finish(e.name); } });",
    "SyntaxError"
);
idb!(
    autoinc_empty_keypath,
    "await openDb(fresh(), 1, (db) => { try { db.createObjectStore('s', {keyPath: '', autoIncrement: true}); } catch (e) { finish(e.name); } });",
    "InvalidAccessError"
);
idb!(
    put_get_out_of_line,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var t = db.transaction('s', 'readwrite'); var s = t.objectStore('s'); s.put({a: 1}, 'k'); var v = await req(s.get('k')); finish(JSON.stringify(v));",
    "{\"a\":1}"
);
idb!(
    put_inline_keypath,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s', {keyPath: 'id'})); var s = db.transaction('s', 'readwrite').objectStore('s'); var k = await req(s.put({id: 5, x: 'y'})); var v = await req(s.get(5)); finish(k + ' ' + v.x);",
    "5 y"
);
idb!(
    nested_keypath,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s', {keyPath: 'a.b.c'})); var s = db.transaction('s', 'readwrite').objectStore('s'); var k = await req(s.put({a: {b: {c: 'deep'}}})); finish(k);",
    "deep"
);
idb!(
    array_keypath,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s', {keyPath: ['x', 'y']})); var s = db.transaction('s', 'readwrite').objectStore('s'); var k = await req(s.put({x: 1, y: 'b'})); finish(JSON.stringify(k));",
    "[1,\"b\"]"
);
idb!(
    autoincrement_generator,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s', {autoIncrement: true})); var s = db.transaction('s', 'readwrite').objectStore('s'); var a = await req(s.add('x')); var b = await req(s.add('y')); await req(s.put('z', 10)); var c = await req(s.add('w')); finish([a, b, c].join());",
    "1,2,11"
);
idb!(
    autoincrement_injects_key,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s', {keyPath: 'id', autoIncrement: true})); var s = db.transaction('s', 'readwrite').objectStore('s'); var k = await req(s.add({n: 1})); var v = await req(s.get(k)); finish(k + ' ' + v.id + ' ' + v.n);",
    "1 1 1"
);
idb!(
    add_duplicate_constraint,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var t = db.transaction('s', 'readwrite'); var s = t.objectStore('s'); s.add(1, 'k'); var r = s.add(2, 'k'); r.onerror = (e) => { log.push(r.error.name); e.preventDefault(); }; await txDone(t); finish(log.join());",
    "ConstraintError"
);
idb!(
    error_aborts_transaction,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var t = db.transaction('s', 'readwrite'); var s = t.objectStore('s'); s.add(1, 'k'); s.add(2, 'k'); try { await txDone(t); finish('no'); } catch (e) { var g = await req(db.transaction('s').objectStore('s').count()); finish(e.name + ' ' + g); }",
    "ConstraintError 0"
);
idb!(
    readonly_error,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var s = db.transaction('s').objectStore('s'); try { s.put(1, 1); finish('no'); } catch (e) { finish(e.name); }",
    "ReadOnlyError"
);
idb!(
    data_error_inline_with_key,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s', {keyPath: 'id'})); var s = db.transaction('s', 'readwrite').objectStore('s'); try { s.put({id: 1}, 2); finish('no'); } catch (e) { finish(e.name); }",
    "DataError"
);
idb!(
    data_error_no_key,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var s = db.transaction('s', 'readwrite').objectStore('s'); try { s.put('x'); finish('no'); } catch (e) { finish(e.name); }",
    "DataError"
);
idb!(
    data_clone_error,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var s = db.transaction('s', 'readwrite').objectStore('s'); try { s.put(function () {}, 1); finish('no'); } catch (e) { finish(e.name); }",
    "DataCloneError"
);
idb!(
    get_all_and_keys,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var s = db.transaction('s', 'readwrite').objectStore('s'); for (var i = 5; i > 0; i--) s.put('v' + i, i); var all = await req(s.getAll()); var keys = await req(s.getAllKeys(IDBKeyRange.bound(2, 4), 2)); finish(all.join() + ' ' + keys.join());",
    "v1,v2,v3,v4,v5 2,3"
);
idb!(
    count_and_delete_range,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var s = db.transaction('s', 'readwrite').objectStore('s'); for (var i = 1; i <= 6; i++) s.put(i, i); s.delete(IDBKeyRange.bound(2, 4)); var c = await req(s.count()); var c2 = await req(s.count(IDBKeyRange.lowerBound(5))); finish(c + ' ' + c2);",
    "3 2"
);
idb!(
    clear_store,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var s = db.transaction('s', 'readwrite').objectStore('s'); s.put(1, 1); s.put(2, 2); s.clear(); finish(await req(s.count()));",
    "0"
);
idb!(
    get_key,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var s = db.transaction('s', 'readwrite').objectStore('s'); s.put('a', 3); s.put('b', 7); finish(await req(s.getKey(IDBKeyRange.lowerBound(4))));",
    "7"
);
idb!(
    get_missing_undefined,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var s = db.transaction('s').objectStore('s'); finish(String(await req(s.get(1))));",
    "undefined"
);
idb!(
    delete_store,
    "var n = fresh(); (await openDb(n, 1, (db) => { db.createObjectStore('a'); db.createObjectStore('b'); })).close(); var db = await openDb(n, 2, (db) => db.deleteObjectStore('a')); finish(Array.from(db.objectStoreNames).join());",
    "b"
);
idb!(
    rename_store,
    "var n = fresh(); (await openDb(n, 1, (db) => db.createObjectStore('viejo').put(1, 1))).close(); var db = await openDb(n, 2, (db, t) => { t.objectStore('viejo').name = 'nuevo'; }); var v = await req(db.transaction('nuevo').objectStore('nuevo').get(1)); finish(Array.from(db.objectStoreNames).join() + ' ' + v);",
    "nuevo 1"
);
idb!(
    value_clone_types,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var s = db.transaction('s', 'readwrite').objectStore('s'); var o = {d: new Date(5), m: new Map([['a', 1]]), st: new Set([2]), u8: new Uint8Array([1, 2]), re: /x/g, n: NaN, z: -0, big: 10n, arr: [1, , 3]}; o.self = o; s.put(o, 1); var v = await req(s.get(1)); finish([v.d.getTime(), v.m.get('a'), v.st.has(2), v.u8[1], v.re.flags, Number.isNaN(v.n), Object.is(v.z, -0), v.big, 1 in v.arr, v.self === v, v !== o].join());",
    "5,1,true,2,g,true,true,10,false,true,true"
);
idb!(
    store_blob,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var s = db.transaction('s', 'readwrite').objectStore('s'); s.put(new Blob(['hola'], {type: 'text/plain'}), 1); var v = await req(s.get(1)); finish((v instanceof Blob) + ' ' + v.type + ' ' + await v.text());",
    "true text/plain hola"
);

// --- Claves y rangos ---

idb!(
    key_types_order,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var s = db.transaction('s', 'readwrite').objectStore('s'); var keys = [[1], 'b', new Date(2), 3, new Uint8Array([7]).buffer, 'a', -Infinity]; for (var k of keys) s.put(1, k); var got = await req(s.getAllKeys()); finish(got.map((k) => k instanceof Date ? 'D' + k.getTime() : k instanceof ArrayBuffer ? 'B' : Array.isArray(k) ? 'A' : String(k)).join());",
    "-Infinity,3,D2,a,b,B,A"
);
idb!(
    key_range_basics,
    "var r = IDBKeyRange.bound(1, 5, true, false); finish([r.lower, r.upper, r.lowerOpen, r.upperOpen, r.includes(1), r.includes(5), r.includes(3)].join());",
    "1,5,true,false,false,true,true"
);
idb!(
    key_range_only_and_bounds,
    "var o = IDBKeyRange.only('x'); var l = IDBKeyRange.lowerBound(2); var u = IDBKeyRange.upperBound(9, true); finish([o.lower, o.upper, l.upper, l.upperOpen, u.lower, u.upperOpen].join('|'));",
    "x|x||true||true"
);
idb!(
    key_range_invalid,
    "try { IDBKeyRange.bound(5, 1); finish('no'); } catch (e) { finish(e.name); }",
    "DataError"
);
idb!(
    invalid_key_nan,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var s = db.transaction('s', 'readwrite').objectStore('s'); try { s.put(1, NaN); finish('no'); } catch (e) { finish(e.name); }",
    "DataError"
);
idb!(
    binary_and_array_keys,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var s = db.transaction('s', 'readwrite').objectStore('s'); s.put('bin', new Uint8Array([1, 2])); s.put('arr', [1, [2, 'x']]); var a = await req(s.get(new Uint8Array([1, 2]))); var b = await req(s.get([1, [2, 'x']])); finish(a + ' ' + b);",
    "bin arr"
);

// --- Índices ---

idb!(
    index_get,
    "var db = await openDb(fresh(), 1, (db) => { var s = db.createObjectStore('p', {keyPath: 'id'}); s.createIndex('byName', 'name'); }); var s = db.transaction('p', 'readwrite').objectStore('p'); s.put({id: 1, name: 'ana'}); s.put({id: 2, name: 'beto'}); var ix = s.index('byName'); var v = await req(ix.get('beto')); var k = await req(ix.getKey('ana')); finish(v.id + ' ' + k + ' ' + ix.keyPath + ' ' + ix.unique);",
    "2 1 name false"
);
idb!(
    index_unique_constraint,
    "var db = await openDb(fresh(), 1, (db) => { var s = db.createObjectStore('p', {keyPath: 'id'}); s.createIndex('mail', 'mail', {unique: true}); }); var t = db.transaction('p', 'readwrite'); var s = t.objectStore('p'); s.put({id: 1, mail: 'a@x'}); var r = s.put({id: 2, mail: 'a@x'}); r.onerror = (e) => { log.push(r.error.name); e.preventDefault(); }; await txDone(t); finish(log.join() + ' ' + await req(db.transaction('p').objectStore('p').count()));",
    "ConstraintError 1"
);
idb!(
    index_multi_entry,
    "var db = await openDb(fresh(), 1, (db) => { var s = db.createObjectStore('p', {keyPath: 'id'}); s.createIndex('tags', 'tags', {multiEntry: true}); }); var s = db.transaction('p', 'readwrite').objectStore('p'); s.put({id: 1, tags: ['a', 'b', 'a']}); s.put({id: 2, tags: ['b']}); var ix = s.index('tags'); var c = await req(ix.count('b')); var all = await req(ix.getAllKeys('a')); var total = await req(ix.count()); finish(c + ' ' + all.join() + ' ' + total);",
    "2 1 3"
);
idb!(
    index_getall_order,
    "var db = await openDb(fresh(), 1, (db) => { db.createObjectStore('p', {keyPath: 'id'}).createIndex('age', 'age'); }); var s = db.transaction('p', 'readwrite').objectStore('p'); s.put({id: 'c', age: 30}); s.put({id: 'a', age: 20}); s.put({id: 'b', age: 30}); var v = await req(s.index('age').getAll()); finish(v.map((x) => x.id).join());",
    "a,b,c"
);
idb!(
    index_on_existing_data,
    "var n = fresh(); (await openDb(n, 1, (db) => { var s = db.createObjectStore('p'); s.put({k: 'x'}, 1); s.put({k: 'y'}, 2); })).close(); var db = await openDb(n, 2, (db, t) => t.objectStore('p').createIndex('k', 'k')); finish(await req(db.transaction('p').objectStore('p').index('k').getKey('y')));",
    "2"
);
idb!(
    index_unique_on_existing_aborts,
    "var n = fresh(); (await openDb(n, 1, (db) => { var s = db.createObjectStore('p'); s.put({k: 'x'}, 1); s.put({k: 'x'}, 2); })).close(); var r = indexedDB.open(n, 2); r.onupgradeneeded = () => r.transaction.objectStore('p').createIndex('k', 'k', {unique: true}); try { await req(r); finish('no'); } catch (e) { finish(e.name); }",
    "AbortError"
);
idb!(
    delete_index,
    "var n = fresh(); (await openDb(n, 1, (db) => db.createObjectStore('p').createIndex('i', 'i'))).close(); var db = await openDb(n, 2, (db, t) => t.objectStore('p').deleteIndex('i')); finish(db.transaction('p').objectStore('p').indexNames.length);",
    "0"
);
idb!(
    index_not_found,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('p')); try { db.transaction('p').objectStore('p').index('nope'); finish('no'); } catch (e) { finish(e.name); }",
    "NotFoundError"
);
idb!(
    index_multientry_array_keypath,
    "await openDb(fresh(), 1, (db) => { try { db.createObjectStore('p').createIndex('i', ['a', 'b'], {multiEntry: true}); } catch (e) { finish(e.name); } });",
    "InvalidAccessError"
);

// --- Cursores ---

idb!(
    cursor_iterate,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var t = db.transaction('s', 'readwrite'); var s = t.objectStore('s'); for (var i = 1; i <= 4; i++) s.put('v' + i, i); var r = s.openCursor(); r.onsuccess = () => { var c = r.result; if (c) { log.push(c.key + '=' + c.value); c.continue(); } else finish(log.join() + ' ' + (r.source === s)); };",
    "1=v1,2=v2,3=v3,4=v4 true"
);
idb!(
    cursor_prev_and_range,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var s = db.transaction('s', 'readwrite').objectStore('s'); for (var i = 1; i <= 5; i++) s.put(i, i); var r = s.openCursor(IDBKeyRange.bound(2, 4), 'prev'); r.onsuccess = () => { var c = r.result; if (c) { log.push(c.key); c.continue(); } else finish(log.join() + ' ' + 'prev'); };",
    "4,3,2 prev"
);
idb!(
    cursor_advance,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var s = db.transaction('s', 'readwrite').objectStore('s'); for (var i = 1; i <= 9; i++) s.put(i, i); var r = s.openCursor(); r.onsuccess = () => { var c = r.result; if (c) { log.push(c.key); c.advance(3); } else finish(log.join()); };",
    "1,4,7"
);
idb!(
    cursor_continue_key,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var s = db.transaction('s', 'readwrite').objectStore('s'); for (var i = 1; i <= 9; i++) s.put(i, i); var r = s.openCursor(); var first = true; r.onsuccess = () => { var c = r.result; if (c) { log.push(c.key); if (first) { first = false; c.continue(7); } else c.continue(); } else finish(log.join()); };",
    "1,7,8,9"
);
idb!(
    cursor_update_and_delete,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var t = db.transaction('s', 'readwrite'); var s = t.objectStore('s'); for (var i = 1; i <= 4; i++) s.put(i, i); var r = s.openCursor(); r.onsuccess = () => { var c = r.result; if (!c) return; if (c.key % 2) c.update(c.value * 10); else c.delete(); c.continue(); }; await txDone(t); var all = await req(db.transaction('s').objectStore('s').getAll()); finish(all.join());",
    "10,30"
);
idb!(
    key_cursor_no_value,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var s = db.transaction('s', 'readwrite').objectStore('s'); s.put('x', 1); var r = s.openKeyCursor(); r.onsuccess = () => { var c = r.result; finish((c instanceof IDBCursor) + ' ' + (c instanceof IDBCursorWithValue) + ' ' + c.key + ' ' + ('value' in c)); };",
    "true false 1 false"
);
idb!(
    index_cursor_unique,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('p', {keyPath: 'id'}).createIndex('g', 'g')); var s = db.transaction('p', 'readwrite').objectStore('p'); [[1, 'a'], [2, 'b'], [3, 'a'], [4, 'b'], [5, 'c']].forEach(([id, g]) => s.put({id, g})); var r = s.index('g').openCursor(null, 'nextunique'); r.onsuccess = () => { var c = r.result; if (c) { log.push(c.key + c.primaryKey); c.continue(); } else finish(log.join()); };",
    "a1,b2,c5"
);
idb!(
    index_cursor_prevunique,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('p', {keyPath: 'id'}).createIndex('g', 'g')); var s = db.transaction('p', 'readwrite').objectStore('p'); [[1, 'a'], [2, 'b'], [3, 'a'], [4, 'b']].forEach(([id, g]) => s.put({id, g})); var r = s.index('g').openCursor(null, 'prevunique'); r.onsuccess = () => { var c = r.result; if (c) { log.push(c.key + c.primaryKey); c.continue(); } else finish(log.join()); };",
    "b2,a1"
);
idb!(
    continue_primary_key,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('p', {keyPath: 'id'}).createIndex('g', 'g')); var s = db.transaction('p', 'readwrite').objectStore('p'); [[1, 'a'], [2, 'a'], [3, 'a'], [4, 'b']].forEach(([id, g]) => s.put({id, g})); var r = s.index('g').openCursor(); var first = true; r.onsuccess = () => { var c = r.result; if (c) { log.push(c.primaryKey); if (first) { first = false; c.continuePrimaryKey('a', 3); } else c.continue(); } else finish(log.join()); };",
    "1,3,4"
);
idb!(
    cursor_continue_backwards_error,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var s = db.transaction('s', 'readwrite').objectStore('s'); s.put(1, 5); var r = s.openCursor(); r.onsuccess = () => { var c = r.result; try { c.continue(3); } catch (e) { finish(e.name); } };",
    "DataError"
);
idb!(
    cursor_twice_error,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var s = db.transaction('s', 'readwrite').objectStore('s'); s.put(1, 1); s.put(2, 2); var r = s.openCursor(); r.onsuccess = () => { var c = r.result; if (!c) return; c.continue(); try { c.continue(); } catch (e) { finish(e.name); } };",
    "InvalidStateError"
);

// --- Transacciones ---

idb!(
    transaction_complete_event,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var t = db.transaction('s', 'readwrite'); t.objectStore('s').put(1, 1); finish(await txDone(t) + ' ' + t.mode + ' ' + (t.db === db) + ' ' + t.objectStoreNames.length);",
    "complete readwrite true 1"
);
idb!(
    transaction_abort_reverts,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var t = db.transaction('s', 'readwrite'); var s = t.objectStore('s'); s.put(1, 1); var r = s.put(2, 2); r.onsuccess = () => t.abort(); try { await txDone(t); } catch (e) { log.push(String(e)); } finish(log.join() + ' ' + await req(db.transaction('s').objectStore('s').count()));",
    "null 0"
);
idb!(
    abort_pending_requests_error,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var t = db.transaction('s', 'readwrite'); var r = t.objectStore('s').put(1, 1); r.onerror = (e) => { log.push(r.error.name); }; t.abort(); t.onabort = () => finish(log.join() + ' abort');",
    "AbortError abort"
);
idb!(
    transaction_inactive_after_task,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var t = db.transaction('s'); var s = t.objectStore('s'); setTimeout(() => { try { s.get(1); finish('no'); } catch (e) { finish(e.name); } }, 0);",
    "TransactionInactiveError"
);
idb!(
    transaction_not_found,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); try { db.transaction('nope'); finish('no'); } catch (e) { finish(e.name); }",
    "NotFoundError"
);
idb!(
    transaction_empty_scope,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); try { db.transaction([]); finish('no'); } catch (e) { finish(e.name); }",
    "InvalidAccessError"
);
idb!(
    transaction_bad_mode,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); try { db.transaction('s', 'bogus'); finish('no'); } catch (e) { finish(e.name); }",
    "TypeError"
);
idb!(
    readwrite_serialized,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var t1 = db.transaction('s', 'readwrite'); var t2 = db.transaction('s', 'readwrite'); t1.objectStore('s').put('uno', 1).onsuccess = () => log.push('t1 put'); t2.objectStore('s').get(1).onsuccess = (e) => log.push('t2 get ' + e.target.result); t1.oncomplete = () => log.push('t1 complete'); await txDone(t2); finish(log.join(', '));",
    "t1 put, t1 complete, t2 get uno"
);
idb!(
    readonly_parallel,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var t1 = db.transaction('s'); var t2 = db.transaction('s'); t1.objectStore('s').count().onsuccess = () => log.push('a'); t2.objectStore('s').count().onsuccess = () => log.push('b'); await Promise.all([txDone(t1), txDone(t2)]); finish(log.join());",
    "a,b"
);
idb!(
    transaction_commit,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var t = db.transaction('s', 'readwrite'); t.objectStore('s').put(1, 1); t.commit(); try { t.objectStore('s').put(2, 2); finish('no'); } catch (e) { log.push(e.name); } await txDone(t); finish(log.join());",
    "TransactionInactiveError"
);
idb!(
    request_error_bubbles_to_transaction,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var t = db.transaction('s', 'readwrite'); var s = t.objectStore('s'); s.add(1, 1); var r = s.add(1, 1); t.onerror = (e) => { log.push('txn ' + (e.target === r)); e.preventDefault(); }; db.onerror = (e) => log.push('db ' + (e.target === r)); await txDone(t); finish(log.join());",
    "txn true,db true"
);
idb!(
    listener_throw_aborts,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var t = db.transaction('s', 'readwrite'); t.objectStore('s').put(1, 1).onsuccess = () => { throw new Error('boom'); }; try { await txDone(t); finish('no'); } catch (e) { finish(e.name); }",
    "AbortError"
);
idb!(
    object_store_same_handle,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var t = db.transaction('s'); finish(t.objectStore('s') === t.objectStore('s'));",
    "true"
);
idb!(
    events_trusted,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); var r = db.transaction('s').objectStore('s').count(); r.onsuccess = (e) => finish(e.isTrusted + ' ' + e.type + ' ' + e.bubbles);",
    "true success false"
);
idb!(
    close_then_transaction,
    "var db = await openDb(fresh(), 1, (db) => db.createObjectStore('s')); db.close(); try { db.transaction('s'); finish('no'); } catch (e) { finish(e.name); }",
    "InvalidStateError"
);

// --- navigator.storage ---

idb!(
    storage_manager,
    "var e = await navigator.storage.estimate(); var p1 = await navigator.storage.persisted(); var p = await navigator.storage.persist(); finish([typeof e.usage, e.quota > 0, p1, p, navigator.storage instanceof StorageManager].join());",
    "number,true,false,true,true"
);

// --- Persistencia ---

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("freedom-idb-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn persists_after_reopen() {
    let dir = temp_dir("persist");
    {
        let mut p = Page::new(Some(dir.clone()));
        let r = p.run(
            "var db = await openDb('agenda', 2, (db) => { var s = db.createObjectStore('c', {keyPath: 'id', autoIncrement: true}); s.createIndex('mail', 'mail', {unique: true}); }); \
             var t = db.transaction('c', 'readwrite'); var s = t.objectStore('c'); s.add({mail: 'a@x', d: new Date(7)}); s.add({mail: 'b@x'}); await txDone(t); finish('ok');",
        );
        assert_eq!(r, "ok");
    }
    // Un agente nuevo (otra "sesión") lee del disco.
    let mut p = Page::new(Some(dir.clone()));
    let r = p.run(
        "var db = await openDb('agenda'); var s = db.transaction('c', 'readwrite').objectStore('c'); \
         var v = await req(s.index('mail').get('a@x')); var k = await req(s.add({mail: 'c@x'})); \
         finish(db.version + ' ' + v.id + ' ' + v.d.getTime() + ' ' + k + ' ' + Array.from(db.objectStoreNames));",
    );
    assert_eq!(r, "2 1 7 3 c");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn aborted_writes_are_not_persisted() {
    let dir = temp_dir("abort");
    {
        let mut p = Page::new(Some(dir.clone()));
        p.run(
            "var db = await openDb('x', 1, (db) => db.createObjectStore('s')); var t = db.transaction('s', 'readwrite'); t.objectStore('s').put(1, 1); await txDone(t); \
             var t2 = db.transaction('s', 'readwrite'); t2.objectStore('s').put(2, 2).onsuccess = () => t2.abort(); try { await txDone(t2); } catch (e) {} finish('ok');",
        );
    }
    let mut p = Page::new(Some(dir.clone()));
    assert_eq!(
        p.run("var db = await openDb('x'); finish(await req(db.transaction('s').objectStore('s').getAllKeys()));"),
        "1"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_database_removes_file() {
    let dir = temp_dir("delete");
    {
        let mut p = Page::new(Some(dir.clone()));
        p.run("(await openDb('borrar', 3)).close(); await req(indexedDB.deleteDatabase('borrar')); finish('ok');");
    }
    let mut p = Page::new(Some(dir.clone()));
    assert_eq!(
        p.run("var db = await openDb('borrar'); finish(db.version);"),
        "1"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn many_writes_compact_log() {
    let dir = temp_dir("compact");
    {
        let mut p = Page::new(Some(dir.clone()));
        p.run(
            "var db = await openDb('c', 1, (db) => db.createObjectStore('s')); \
             for (var i = 0; i < 300; i++) { var t = db.transaction('s', 'readwrite'); t.objectStore('s').put(i, 1); await txDone(t); } finish('ok');",
        );
    }
    let mut p = Page::new(Some(dir.clone()));
    assert_eq!(
        p.run("var db = await openDb('c'); finish(await req(db.transaction('s').objectStore('s').get(1)));"),
        "299"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
