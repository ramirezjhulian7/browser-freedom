//! E6: autenticación HTTP (RFC 9110 §11, Basic RFC 7617, Digest RFC 7616) contra un servidor
//! local, con el `authenticator` del `Fetcher`, las credenciales de la URL y
//! `Fetcher::set_credentials`.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use bf_net::auth::{self, AuthPrompt, Credentials};
use bf_net::{FetchOptions, Fetcher};
use bf_url::Url;

/// Las peticiones que vio el servidor: ruta y `Authorization`.
type Log = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// Lo que respondió el servidor: estado y cabeceras extra.
type Reply = (u16, String);

/// Un servidor HTTP/1.1 que decide con `check(ruta, Authorization)` y anota la `Authorization`
/// de cada petición.
fn serve(check: impl Fn(&str, Option<&str>) -> Reply + Send + Sync + 'static) -> (u16, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let log = Arc::new(Mutex::new(Vec::new()));
    let seen = log.clone();
    let check = Arc::new(check);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let (seen, check) = (seen.clone(), check.clone());
            thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let path = line.split(' ').nth(1).unwrap_or("").to_owned();
                    let mut authorization = None;
                    loop {
                        let mut h = String::new();
                        if reader.read_line(&mut h).unwrap_or(0) == 0 {
                            return;
                        }
                        let h = h.trim_end();
                        if h.is_empty() {
                            break;
                        }
                        if let Some((n, v)) = h.split_once(':')
                            && n.eq_ignore_ascii_case("authorization")
                        {
                            authorization = Some(v.trim().to_owned());
                        }
                    }
                    seen.lock()
                        .unwrap()
                        .push((path.clone(), authorization.clone()));
                    let (status, extra) = check(&path, authorization.as_deref());
                    let body = if status == 200 { "ok" } else { "no" };
                    let reply = format!(
                        "HTTP/1.1 {status} X\r\n{extra}Cache-Control: no-store\r\n\
                         Content-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    if stream.write_all(reply.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    (port, log)
}

const BASIC_UP: &str = "Basic dTpw"; // u:p

fn basic_server() -> (u16, Log) {
    serve(|_, a| {
        if a == Some(BASIC_UP) {
            (200, String::new())
        } else {
            (401, "WWW-Authenticate: Basic realm=\"r\"\r\n".into())
        }
    })
}

fn url(port: u16, path: &str) -> Url {
    Url::parse(&format!("http://127.0.0.1:{port}{path}"), None).unwrap()
}

fn get(f: &Fetcher, u: &Url) -> u16 {
    f.fetch(u, &FetchOptions::default()).unwrap().status
}

/// El embebedor aporta credenciales ante el 401; aceptadas, van sin esperar el reto a las rutas
/// del mismo espacio (RFC 7617 §2.2) y no a las de fuera.
#[test]
fn basic_with_the_authenticator() {
    let (port, log) = basic_server();
    let prompts = Arc::new(Mutex::new(Vec::<AuthPrompt>::new()));
    let p = prompts.clone();
    let mut f = Fetcher::new();
    f.authenticator = Some(Arc::new(move |prompt: &AuthPrompt| {
        p.lock().unwrap().push(prompt.clone());
        // La primera vez, mal; después, bien.
        let n = p.lock().unwrap().len();
        Some(if n == 1 {
            Credentials::new("u", "mal")
        } else {
            Credentials::new("u", "p")
        })
    }));
    assert_eq!(get(&f, &url(port, "/docs/a")), 200);
    {
        let prompts = prompts.lock().unwrap();
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts[0].realm.as_deref(), Some("r"));
        assert_eq!(prompts[0].scheme, "basic");
        assert_eq!((prompts[0].retry, prompts[1].retry), (false, true));
    }
    assert_eq!(
        log.lock().unwrap().clone(),
        [
            ("/docs/a".to_owned(), None),
            ("/docs/a".to_owned(), Some("Basic dTptYWw=".to_owned())),
            ("/docs/a".to_owned(), Some(BASIC_UP.to_owned())),
        ]
    );
    log.lock().unwrap().clear();
    assert_eq!(get(&f, &url(port, "/docs/sub/b")), 200);
    assert_eq!(get(&f, &url(port, "/fuera")), 200);
    assert_eq!(
        log.lock().unwrap().clone(),
        [
            ("/docs/sub/b".to_owned(), Some(BASIC_UP.to_owned())),
            ("/fuera".to_owned(), None),
            ("/fuera".to_owned(), Some(BASIC_UP.to_owned())),
        ]
    );
    // Las guardadas se usaron para `/fuera` (mismo realm) sin preguntar otra vez.
    assert_eq!(prompts.lock().unwrap().len(), 2);
}

/// Sin credenciales (el embebedor cancela, o no hay `authenticator`) el 401 llega al llamador,
/// que lee el reto y puede dar credenciales con `set_credentials` para reintentar.
#[test]
fn cancel_and_set_credentials() {
    let (port, log) = basic_server();
    let mut f = Fetcher::new();
    f.authenticator = Some(Arc::new(|_: &AuthPrompt| None));
    let res = f.fetch(&url(port, "/x"), &FetchOptions::default()).unwrap();
    assert_eq!(res.status, 401);
    let challenges = auth::challenges(&res.headers, "www-authenticate");
    assert_eq!(challenges[0].realm(), Some("r"));
    let f = Fetcher::new();
    assert_eq!(get(&f, &url(port, "/x")), 401);
    f.set_credentials(&url(port, "/x"), "r", Credentials::new("u", "p"));
    log.lock().unwrap().clear();
    assert_eq!(get(&f, &url(port, "/x")), 200);
    // Se mandan al llegar el reto, no antes.
    assert_eq!(log.lock().unwrap().len(), 2);
}

/// Fetch §4.5 paso 8.21: las credenciales de la URL van como Basic desde la primera petición.
#[test]
fn credentials_in_the_url() {
    let (port, log) = basic_server();
    let f = Fetcher::new();
    let u = Url::parse(&format!("http://u:p@127.0.0.1:{port}/y"), None).unwrap();
    assert_eq!(get(&f, &u), 200);
    assert_eq!(
        log.lock().unwrap().clone(),
        [("/y".to_owned(), Some(BASIC_UP.to_owned()))]
    );
}

/// RFC 7616 con SHA-256 y `qop=auth`: el servidor comprueba la respuesta; la segunda petición
/// va sin esperar el reto con `nc=00000002`; un `stale=true` repite con el `nonce` nuevo sin
/// preguntar al embebedor.
#[test]
fn digest_sha256_with_stale_nonce() {
    let nonce = Arc::new(Mutex::new("n1".to_owned()));
    let n = nonce.clone();
    let (port, log) = serve(move |path, a| {
        let current = n.lock().unwrap().clone();
        let challenge = |stale: bool| {
            (
                401,
                format!(
                    "WWW-Authenticate: Digest realm=\"d\", qop=\"auth\", algorithm=SHA-256, \
                     nonce=\"{current}\", opaque=\"o\"{}\r\n",
                    if stale { ", stale=true" } else { "" }
                ),
            )
        };
        let Some(a) = a else {
            return challenge(false);
        };
        let c = auth::parse_challenges(a).remove(0);
        let p = |k: &str| c.param(k).unwrap_or("").to_owned();
        if p("nonce") != current {
            return challenge(true);
        }
        let h = |s: String| -> String {
            auth::sha256(s.as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        let ha1 = h(format!("{}:d:secreto", p("username")));
        let ha2 = h(format!("GET:{}", p("uri")));
        let expected = h(format!(
            "{ha1}:{current}:{}:{}:auth:{ha2}",
            p("nc"),
            p("cnonce")
        ));
        if c.scheme == "digest" && p("uri") == path && p("response") == expected {
            (200, String::new())
        } else {
            challenge(false)
        }
    });
    let asked = Arc::new(AtomicUsize::new(0));
    let a = asked.clone();
    let mut f = Fetcher::new();
    f.authenticator = Some(Arc::new(move |prompt: &AuthPrompt| {
        a.fetch_add(1, Ordering::SeqCst);
        assert_eq!(prompt.scheme, "digest");
        Some(Credentials::new("Mufasa", "secreto"))
    }));
    assert_eq!(get(&f, &url(port, "/p/uno")), 200);
    assert_eq!(get(&f, &url(port, "/p/dos")), 200);
    let nc = |i: usize| {
        let log = log.lock().unwrap();
        let a = log[i].1.clone().unwrap_or_default();
        auth::parse_challenges(&a)
            .first()
            .and_then(|c| c.param("nc").map(str::to_owned))
    };
    assert_eq!(log.lock().unwrap().len(), 3);
    assert_eq!(nc(1).as_deref(), Some("00000001"));
    assert_eq!(nc(2).as_deref(), Some("00000002"));
    // El servidor cambia el nonce: `stale=true` y se repite con el nuevo.
    *nonce.lock().unwrap() = "n2".into();
    assert_eq!(get(&f, &url(port, "/p/tres")), 200);
    assert_eq!(log.lock().unwrap().len(), 5);
    assert_eq!(asked.load(Ordering::SeqCst), 1);
}

/// Una petición sin credenciales (`credentials: omit`) no se autentica.
#[test]
fn no_auth_without_credentials_mode() {
    let (port, _) = basic_server();
    let mut f = Fetcher::new();
    f.authenticator = Some(Arc::new(|_: &AuthPrompt| -> Option<Credentials> {
        panic!("no se pregunta sin credenciales")
    }));
    let opts = bf_net::RequestOptions {
        credentials: false,
        ..Default::default()
    };
    let (res, _) = f.fetch_request(&url(port, "/z"), &opts).unwrap();
    assert_eq!(res.status, 401);
}
