//! Realms sin `WebContext` (E1): un `ShadowRealm` o un realm que el embebedor no instaló como
//! ventana. Las funciones del host no pueden correr allí: antes tumbaban la pestaña con el pánico
//! "el realm no tiene WebContext" (la página interna "Esta página falló"); ahora lanzan un
//! `TypeError` y el `ShadowRealm` no expone las interfaces del host.

use bf_js::host::DefaultHooks;
use bf_web::{DocumentOwner, Runtime, Url, WebContext};

/// Una ventana con un documento vacío; devuelve el runtime y su realm.
fn window() -> (Runtime, bf_js::vm::RealmId) {
    let mut rt = Runtime::new(Box::new(DefaultHooks));
    let (doc, _) = bf_html::parse_document("<p>x</p>", &bf_html::ParseOptions::default());
    let realm = rt.main_realm();
    let url = Url::parse("https://example.test/", None).expect("url");
    bf_web::install(
        &mut rt.agent,
        realm,
        WebContext::new(DocumentOwner::Document(Box::new(doc)), url),
    );
    (rt, realm)
}

fn show(rt: &mut Runtime, realm: bf_js::vm::RealmId, src: &str) -> String {
    match rt.eval_script(realm, src, "test.js") {
        Ok(v) => rt.to_display(&v),
        Err(e) => panic!("error: {e}\n{}", e.stack),
    }
}

/// `new ShadowRealm().evaluate("new EventTarget()")` llegaba a `event_target_ctor`, que pide el
/// documento del realm en curso: el `ShadowRealm` no tiene `WebContext` y el proceso hacía
/// pánico. HostInitializeShadowRealm ya no le expone las interfaces del host; lo de ECMA-262
/// sigue funcionando.
#[test]
fn shadow_realm_has_no_host_interfaces_and_does_not_crash() {
    let (mut rt, realm) = window();
    assert_eq!(
        show(
            &mut rt,
            realm,
            "var sr = new ShadowRealm(); \
             [sr.evaluate('typeof EventTarget'), sr.evaluate('typeof AbortController'), \
              sr.evaluate('typeof Document'), \
              sr.evaluate('try { new EventTarget(); \"creado\" } catch (e) { e.name }'), \
              sr.evaluate('[1, 2, 3].map(x => x * 2).join()'), \
              sr.evaluate('(f) => f() + 1')(() => 41), \
              typeof EventTarget].join('|')"
        ),
        "undefined|undefined|undefined|ReferenceError|2,4,6|42|function"
    );
}

/// La red de seguridad: una función del host llamada en un realm sin `WebContext` (aquí, uno
/// nuevo del runtime al que el embebedor no instaló como ventana; sus interfaces sí están en el
/// global) lanza un `TypeError` en vez de entrar al código del host.
#[test]
fn host_functions_throw_in_a_realm_without_web_context() {
    let (mut rt, _) = window();
    let bare = rt.create_realm();
    assert_eq!(
        show(
            &mut rt,
            bare,
            "var out = []; \
             for (const f of [() => new EventTarget(), () => new AbortController(), \
                              () => new Text('a'), () => AbortSignal.abort(), \
                              () => EventTarget.prototype.addEventListener.call({}, 'x', null)]) { \
               try { f(); out.push('ok'); } catch (e) { out.push(e.name); } \
             } \
             out.join()"
        ),
        "TypeError,TypeError,TypeError,TypeError,TypeError"
    );
}
