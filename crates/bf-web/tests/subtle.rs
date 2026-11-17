//! E1: `crypto.subtle` (Web Cryptography API): `digest` (SHA-1/256/384/512), HMAC
//! (`importKey`/`exportKey` en `raw` y `jwk`, `generateKey`, `sign`, `verify`), AES-CBC, AES-CTR
//! y AES-GCM (`encrypt`, `decrypt`), PBKDF2 y HKDF (`deriveBits`, `deriveKey`), con los vectores
//! oficiales (FIPS 180-4, RFC 4231, SP 800-38A, GCM, RFC 6070, RFC 5869) y los errores de §14.3.

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
        "NotSupportedError,TypeError,TypeError,NotSupportedError,ok,\
         InvalidAccessError,InvalidAccessError,SyntaxError,SyntaxError,TypeError,DataError,\
         TypeError,DataError,DataError,TypeError"
    );
}

/// §28.3 (AES-CBC) con SP 800-38A F.2.1 (la primera pareja de bloques; el resto es el relleno
/// PKCS#7 de §28.3.3), §28.2 (AES-CTR, F.5.1) y §28.4 (AES-GCM, caso de prueba 4 de McGrew y
/// Viega con 32 bits de etiqueta): cifrar, descifrar y el `algorithm` de la clave.
#[test]
fn aes_cbc_ctr_gcm_vectors() {
    let mut p = Page::new("https://example.test/");
    assert_eq!(
        p.run(&format!(
            "{HEX} function unhex(s) {{ return new Uint8Array(s.match(/../g).map(x => parseInt(x, 16))); }} \
             (async () => {{ \
               const s = crypto.subtle, r = []; \
               const key = unhex('2b7e151628aed2a6abf7158809cf4f3c'); \
               const pt = unhex('6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51'); \
               const cbc = await s.importKey('raw', key, 'AES-CBC', true, ['encrypt', 'decrypt']); \
               const iv = unhex('000102030405060708090a0b0c0d0e0f'); \
               const c1 = await s.encrypt({{name: 'aes-cbc', iv}}, cbc, pt); \
               r.push(hex(c1).slice(0, 64), c1.byteLength, \
                      hex(await s.decrypt({{name: 'AES-CBC', iv}}, cbc, c1)) === hex(pt), \
                      JSON.stringify(cbc.algorithm), cbc.usages.join('+')); \
               const ctr = await s.importKey('raw', key, 'AES-CTR', false, ['encrypt']); \
               const counter = unhex('f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff'); \
               r.push(hex(await s.encrypt({{name: 'AES-CTR', counter, length: 64}}, ctr, pt))); \
               const gk = unhex('feffe9928665731c6d6a8f9467308308'); \
               const gcm = await s.importKey('jwk', {{kty: 'oct', k: '_v_pkoZlcxxtao-UZzCDCA', \
                 alg: 'A128GCM', ext: true}}, 'AES-GCM', true, ['encrypt', 'decrypt']); \
               const giv = unhex('cafebabefacedbaddecaf888'); \
               const aad = unhex('feedfacedeadbeeffeedfacedeadbeefabaddad2'); \
               const gpt = unhex('d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a72' + \
                 '1c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39'); \
               const gc = await s.encrypt({{name: 'AES-GCM', iv: giv, additionalData: aad}}, gcm, gpt); \
               r.push(hex(gc).slice(-32), \
                      hex(await s.decrypt({{name: 'AES-GCM', iv: giv, additionalData: aad}}, gcm, gc)) === hex(gpt), \
                      hex(await s.exportKey('raw', gcm)) === hex(gk), \
                      (await s.exportKey('jwk', gcm)).alg); \
               out = r.join(); \
             }})();"
        )),
        "7649abac8119b246cee98e9b12e9197d5086cb9b507219ee95db113a917678b2,48,true,\
         {\"name\":\"AES-CBC\",\"length\":128},encrypt+decrypt,\
         874d6191b620e3261bef6864990db6ce9806f66b7970fdff8617187bb9fffdff,\
         5bc94fbc3221a5db94fae95ae7121a47,true,true,A128GCM"
    );
}

/// Los errores de AES: tamaños de clave (`DataError`), `generateKey` con longitud mala
/// (`OperationError`), iv de otro tamaño, relleno roto y etiqueta que no cuadra
/// (`OperationError`), clave de otro algoritmo (`InvalidAccessError`) y usos que AES no tiene
/// (`SyntaxError`).
#[test]
fn aes_errors() {
    let mut p = Page::new("https://example.test/");
    assert_eq!(
        p.run(
            "(async () => { \
               const s = crypto.subtle, r = []; \
               async function name(pr) { try { await pr; return 'ok'; } catch (e) { return e.name; } } \
               const k = await s.generateKey({name: 'AES-CBC', length: 256}, false, ['encrypt', 'decrypt']); \
               const g = await s.generateKey({name: 'AES-GCM', length: 128}, true, ['decrypt']); \
               const iv = new Uint8Array(16); \
               r.push(k.algorithm.length, k.extractable, \
                      await name(s.importKey('raw', new Uint8Array(20), 'AES-CTR', true, ['encrypt'])), \
                      await name(s.generateKey({name: 'AES-CBC', length: 100}, true, ['encrypt'])), \
                      await name(s.generateKey({name: 'AES-CBC'}, true, ['encrypt'])), \
                      await name(s.encrypt({name: 'AES-CBC', iv: new Uint8Array(8)}, k, new Uint8Array(1))), \
                      await name(s.decrypt({name: 'AES-CBC', iv}, k, new Uint8Array(16))), \
                      await name(s.decrypt({name: 'AES-GCM', iv}, g, new Uint8Array(20))), \
                      await name(s.encrypt({name: 'AES-GCM', iv}, g, new Uint8Array(1))), \
                      await name(s.encrypt({name: 'AES-CTR', counter: iv, length: 64}, k, new Uint8Array(1))), \
                      await name(s.importKey('raw', new Uint8Array(16), 'AES-GCM', true, ['sign'])), \
                      await name(s.encrypt({name: 'AES-GCM', iv, tagLength: 50}, g, new Uint8Array(1))), \
                      await name(s.sign('HMAC', g, new Uint8Array(1)))); \
               out = r.join(); \
             })();"
        ),
        "256,false,DataError,OperationError,TypeError,OperationError,OperationError,\
         OperationError,InvalidAccessError,InvalidAccessError,SyntaxError,InvalidAccessError,\
         InvalidAccessError"
    );
}

/// §31 (PBKDF2) y §32 (HKDF): `importKey` en `raw`, `deriveBits` con los vectores de RFC 6070 y
/// RFC 5869, y `deriveKey` hacia una clave HMAC (la longitud sale del bloque del hash).
#[test]
fn pbkdf2_and_hkdf_derive() {
    let mut p = Page::new("https://example.test/");
    assert_eq!(
        p.run(&format!(
            "{HEX} (async () => {{ \
               const s = crypto.subtle, r = []; \
               async function name(pr) {{ try {{ await pr; return 'ok'; }} catch (e) {{ return e.name; }} }} \
               const pw = await s.importKey('raw', bytes('password'), 'PBKDF2', false, ['deriveBits', 'deriveKey']); \
               r.push(hex(await s.deriveBits({{name: 'PBKDF2', hash: 'SHA-1', salt: bytes('salt'), \
                 iterations: 2}}, pw, 160))); \
               const ikm = await s.importKey('raw', new Uint8Array(22).fill(11), 'HKDF', false, ['deriveBits']); \
               r.push(hex(await s.deriveBits({{name: 'HKDF', hash: 'SHA-256', salt: new Uint8Array(0), \
                 info: new Uint8Array(0)}}, ikm, 336))); \
               const hk = await s.deriveKey({{name: 'PBKDF2', hash: 'SHA-256', salt: bytes('s'), iterations: 1}}, \
                 pw, {{name: 'HMAC', hash: 'SHA-256'}}, true, ['sign']); \
               r.push(hk.algorithm.length, JSON.stringify(pw.algorithm), \
                      await name(s.importKey('raw', bytes('x'), 'PBKDF2', true, ['deriveBits'])), \
                      await name(s.deriveBits({{name: 'HKDF', hash: 'SHA-256', salt: new Uint8Array(0), \
                        info: new Uint8Array(0)}}, pw, 256)), \
                      await name(s.deriveBits({{name: 'PBKDF2', hash: 'SHA-1', salt: bytes('salt'), \
                        iterations: 0}}, pw, 160)), \
                      await name(s.exportKey('raw', pw))); \
               out = r.join(); \
             }})();"
        )),
        "ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957,\
         8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8,\
         512,{\"name\":\"PBKDF2\"},SyntaxError,InvalidAccessError,OperationError,NotSupportedError"
    );
}

/// §14.3.3 y §14.3.4: los datos se copian después de normalizar el algoritmo, así que lo que un
/// getter de `name` escriba en el buffer entra en la firma (lo que miden los subtests de
/// `WebCryptoAPI/sign_verify/hmac.https.any.js` que alteran los datos).
#[test]
fn hmac_copies_data_after_normalizing() {
    let mut p = Page::new("https://example.test/");
    assert_eq!(
        p.run(&format!(
            "{HEX} (async () => {{ \
               const s = crypto.subtle; \
               const k = await s.importKey('raw', new Uint8Array(16), {{name: 'HMAC', hash: 'SHA-256'}}, \
                 false, ['sign', 'verify']); \
               const data = new Uint8Array([1, 2, 3]); \
               const alg = {{ get name() {{ data[0] = 9; return 'HMAC'; }} }}; \
               const a = hex(await s.sign(alg, k, data)); \
               const b = hex(await s.sign('HMAC', k, new Uint8Array([9, 2, 3]))); \
               const sig = await s.sign('HMAC', k, new Uint8Array([7, 2, 3])); \
               const d2 = new Uint8Array([9, 2, 3]); \
               const ok = await s.verify({{ get name() {{ d2[0] = 7; return 'HMAC'; }} }}, k, sig, d2); \
               out = [a === b, ok].join(); \
             }})();"
        )),
        "true,true"
    );
}
