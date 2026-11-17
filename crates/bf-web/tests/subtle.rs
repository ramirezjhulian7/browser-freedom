//! E1: `crypto.subtle` (Web Cryptography API): `digest` (SHA-1/256/384/512) y HMAC
//! (`importKey`/`exportKey` en `raw` y `jwk`, `generateKey`, `sign`, `verify`), con los vectores
//! oficiales (FIPS 180-4, RFC 4231) y los errores de §14.3.

use bf_js::host::DefaultHooks;
use bf_web::{DocumentOwner, Runtime, Url, WebContext};

struct Page {
    rt: Runtime,
    realm: bf_js::vm::RealmId,
}

impl Page {
    fn new(url: &str) -> Page {
        let mut rt = Runtime::new(Box::new(DefaultHooks));
        let (doc, _) = bf_html::parse_document("", &bf_html::ParseOptions::default());
        let realm = rt.main_realm();
        let url = Url::parse(url, None).expect("url");
        bf_web::install(
            &mut rt.agent,
            realm,
            WebContext::new(DocumentOwner::Document(Box::new(doc)), url),
        );
        Page { rt, realm }
    }

    fn show(&mut self, src: &str) -> String {
        match self.rt.eval_script(self.realm, src, "test.js") {
            Ok(v) => self.rt.to_display(&v),
            Err(e) => panic!("error: {e}\n{}", e.stack),
        }
    }

    /// Corre `src` (que deja su resultado en `out`) y vacía los jobs.
    fn run(&mut self, src: &str) -> String {
        self.show(&format!("var out = 'pendiente'; {src}"));
        self.rt.run_until_idle().expect("tareas");
        self.show("String(out)")
    }
}

const HEX: &str = "function hex(b) { return Array.from(new Uint8Array(b), \
                   x => x.toString(16).padStart(2, '0')).join(''); } \
                   function bytes(s) { return new TextEncoder().encode(s); }";

#[test]
fn subtle_exists_in_secure_contexts_only() {
    let mut p = Page::new("https://example.test/");
    assert_eq!(
        p.show(
            "[typeof crypto.subtle.digest, crypto.subtle === crypto.subtle, \
              crypto.subtle instanceof SubtleCrypto, typeof CryptoKey].join()"
        ),
        "function,true,true,function"
    );
    let mut q = Page::new("http://example.test/");
    assert_eq!(q.show("typeof crypto.subtle"), "undefined");
    let mut l = Page::new("http://localhost:8000/");
    assert_eq!(l.show("typeof crypto.subtle"), "object");
}

/// FIPS 180-4: "abc" con los cuatro hashes; el nombre no distingue mayúsculas.
#[test]
fn digest_fips_vectors() {
    let mut p = Page::new("https://example.test/");
    assert_eq!(
        p.run(&format!(
            "{HEX} Promise.all(['SHA-1', 'sha-256', {{name: 'SHA-384'}}, 'SHA-512'].map( \
               a => crypto.subtle.digest(a, bytes('abc')))).then(r => out = r.map(hex).join(' '));"
        )),
        "a9993e364706816aba3e25717850c26c9cd0d89d \
         ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad \
         cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7 \
         ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
    );
}

/// RFC 4231, caso 2 (clave "Jefe"): `importKey` en `raw`, `sign`, `verify` y los atributos de
/// `CryptoKey`.
#[test]
fn hmac_import_sign_verify_rfc_4231() {
    let mut p = Page::new("https://example.test/");
    assert_eq!(
        p.run(&format!(
            "{HEX} (async () => {{ \
               const k = await crypto.subtle.importKey('raw', bytes('Jefe'), \
                 {{name: 'HMAC', hash: 'SHA-256'}}, true, ['verify', 'sign', 'sign']); \
               const m = bytes('what do ya want for nothing?'); \
               const s = await crypto.subtle.sign('HMAC', k, m); \
               const ok = await crypto.subtle.verify({{name: 'hmac'}}, k, s, m); \
               const bad = await crypto.subtle.verify('HMAC', k, new Uint8Array(32), m); \
               out = [hex(s), ok, bad, k.type, k.extractable, k.algorithm.name, \
                      k.algorithm.hash.name, k.algorithm.length, k.usages.join('+'), \
                      k.algorithm === k.algorithm, k.usages === k.usages, \
                      k instanceof CryptoKey].join(); \
             }})();"
        )),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843,true,false,secret,\
         true,HMAC,SHA-256,32,sign+verify,true,true,true"
    );
}

/// JWK de ida y vuelta (RFC 7517/7518), `generateKey` con la longitud del bloque y la clave
/// generada que firma y verifica.
#[test]
fn hmac_jwk_and_generate_key() {
    let mut p = Page::new("https://example.test/");
    assert_eq!(
        p.run(&format!(
            "{HEX} (async () => {{ \
               const g = await crypto.subtle.generateKey({{name: 'HMAC', hash: 'SHA-512'}}, \
                 true, ['sign', 'verify']); \
               const j = await crypto.subtle.exportKey('jwk', g); \
               const raw = await crypto.subtle.exportKey('raw', g); \
               const k = await crypto.subtle.importKey('jwk', j, {{name: 'HMAC', hash: 'SHA-512'}}, \
                 false, ['verify']); \
               const s = await crypto.subtle.sign('HMAC', g, bytes('x')); \
               out = [g.algorithm.length, raw.byteLength, j.kty, j.alg, j.key_ops.join('+'), \
                      j.ext, typeof j.k, await crypto.subtle.verify('HMAC', k, s, bytes('x')), \
                      k.extractable].join(); \
             }})();"
        )),
        "1024,128,oct,HS512,sign+verify,true,string,true,false"
    );
}

/// Los errores de §14.3 y §29: algoritmo no soportado, clave no extraíble, uso que la clave no
/// tiene, usos no válidos o vacíos, datos que no son de la clave y `TypeError` de WebIDL.
#[test]
fn errors_reject_with_the_right_names() {
    let mut p = Page::new("https://example.test/");
    assert_eq!(
        p.run(
            "(async () => { \
               const s = crypto.subtle, r = []; \
               async function name(pr) { try { await pr; return 'ok'; } catch (e) { return e.name; } } \
               const k = await s.importKey('raw', new Uint8Array(16), {name: 'HMAC', hash: 'SHA-256'}, \
                 false, ['sign']); \
               r.push(await name(s.digest('MD5', new Uint8Array(1))), \
                      await name(s.digest('SHA-256', 'texto')), \
                      await name(s.encrypt({name: 'AES-GCM'}, k, new Uint8Array(1))), \
                      await name(s.generateKey({name: 'ECDSA', namedCurve: 'P-256'}, true, ['sign'])), \
                      await name(s.importKey('raw', new Uint8Array(16), 'AES-CBC', true, ['encrypt'])), \
                      await name(s.exportKey('raw', k)), \
                      await name(s.verify('HMAC', k, new Uint8Array(32), new Uint8Array(1))), \
                      await name(s.importKey('raw', new Uint8Array(16), {name: 'HMAC', hash: 'SHA-1'}, \
                        true, ['encrypt'])), \
                      await name(s.importKey('raw', new Uint8Array(16), {name: 'HMAC', hash: 'SHA-1'}, \
                        true, [])), \
                      await name(s.importKey('raw', new Uint8Array(16), {name: 'HMAC', hash: 'SHA-1'}, \
                        true, ['firmar'])), \
                      await name(s.importKey('raw', new Uint8Array(0), {name: 'HMAC', hash: 'SHA-1'}, \
                        true, ['sign'])), \
                      await name(s.importKey('raw', new Uint8Array(4), {name: 'HMAC'}, true, ['sign'])), \
                      await name(s.importKey('jwk', {kty: 'RSA', k: 'AA'}, \
                        {name: 'HMAC', hash: 'SHA-256'}, true, ['sign'])), \
                      await name(s.importKey('jwk', {kty: 'oct', k: 'AAAA', alg: 'HS512'}, \
                        {name: 'HMAC', hash: 'SHA-256'}, true, ['sign'])), \
                      await name(s.sign('HMAC', {}, new Uint8Array(1)))); \
               out = r.join(); \
             })();"
        ),
        "NotSupportedError,TypeError,NotSupportedError,NotSupportedError,NotSupportedError,\
         InvalidAccessError,InvalidAccessError,SyntaxError,SyntaxError,TypeError,DataError,\
         TypeError,DataError,DataError,TypeError"
    );
}
