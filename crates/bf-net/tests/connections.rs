//! E6: conexiones HTTP/1.1 contra servidores locales (keep-alive, a lo sumo 6 por origen y cola
//! para el resto), errores de red con su causa ([`bf_net::NetErrorKind`]) y
//! `stale-while-revalidate` (RFC 5861) validado en segundo plano por el `Fetcher`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bf_net::{CacheUse, FetchOptions, Fetcher, HttpCache, MAX_CONNECTIONS_PER_HOST, NetErrorKind};
use bf_url::Url;

/// Una petición vista por el servidor: ruta y cabeceras (en minúsculas).
type Seen = (String, Vec<(String, String)>);

/// Lo que vio un servidor de prueba.
#[derive(Default)]
struct Stats {
    /// Conexiones aceptadas.
    accepted: AtomicUsize,
    /// Conexiones abiertas ahora y el máximo que hubo a la vez.
    open: AtomicUsize,
    max_open: AtomicUsize,
    /// Las peticiones: ruta y cabeceras (en minúsculas).
    requests: Mutex<Vec<Seen>>,
}

impl Stats {
    fn requests(&self) -> Vec<Seen> {
        self.requests.lock().unwrap().clone()
    }
}

/// Una respuesta del servidor de prueba: cabeceras extra y cuerpo.
type Handler = dyn Fn(&str, &[(String, String)]) -> (u16, String, Vec<u8>) + Send + Sync;

/// Un servidor HTTP/1.1 con keep-alive; cada petición espera `delay` antes de responder.
fn serve(delay: Duration, handler: Arc<Handler>) -> (u16, Arc<Stats>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let stats = Arc::new(Stats::default());
    let s = stats.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let s = s.clone();
            let handler = handler.clone();
            s.accepted.fetch_add(1, Ordering::SeqCst);
            let now = s.open.fetch_add(1, Ordering::SeqCst) + 1;
            s.max_open.fetch_max(now, Ordering::SeqCst);
            thread::spawn(move || {
                connection(stream, delay, &*handler, &s);
                s.open.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    (port, stats)
}

fn connection(mut stream: TcpStream, delay: Duration, handler: &Handler, stats: &Stats) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let path = line.split(' ').nth(1).unwrap_or("").to_owned();
        let mut headers = Vec::new();
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h).unwrap_or(0) == 0 {
                return;
            }
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((n, v)) = h.split_once(':') {
                headers.push((n.trim().to_ascii_lowercase(), v.trim().to_owned()));
            }
        }
        stats
            .requests
            .lock()
            .unwrap()
            .push((path.clone(), headers.clone()));
        thread::sleep(delay);
        let (status, extra, body) = handler(&path, &headers);
        let head = format!(
            "HTTP/1.1 {status} X\r\n{extra}Content-Length: {}\r\n\r\n",
            body.len()
        );
        if stream.write_all(head.as_bytes()).is_err() || stream.write_all(&body).is_err() {
            return;
        }
    }
}

fn url(port: u16, path: &str) -> Url {
    Url::parse(&format!("http://127.0.0.1:{port}{path}"), None).unwrap()
}

fn ok(_: &str, _: &[(String, String)]) -> (u16, String, Vec<u8>) {
    (200, "Cache-Control: no-store\r\n".into(), b"ok".to_vec())
}

/// RFC 9112 §9.3: las peticiones seguidas reutilizan una sola conexión.
#[test]
fn keep_alive_reuses_one_connection() {
    let (port, stats) = serve(Duration::ZERO, Arc::new(ok));
    let f = Fetcher::new();
    for i in 0..3 {
        let res = f.fetch(&url(port, &format!("/{i}")), &FetchOptions::default());
        assert_eq!(res.unwrap().body, b"ok");
    }
    assert_eq!(stats.accepted.load(Ordering::SeqCst), 1);
    assert_eq!(*f.connections_opened.lock().unwrap(), 1);
    assert_eq!(f.open_connections(&url(port, "/")), 1);
}

/// 10 peticiones a la vez: 6 conexiones (el límite por origen), y las 4 que esperan en cola
/// reutilizan esas mismas conexiones cuando quedan libres.
#[test]
fn six_connections_per_host_and_a_queue() {
    let (port, stats) = serve(Duration::from_millis(300), Arc::new(ok));
    let f = Arc::new(Fetcher::new());
    let start = Instant::now();
    let threads: Vec<_> = (0..10)
        .map(|i| {
            let f = f.clone();
            thread::spawn(move || {
                let res = f.fetch(&url(port, &format!("/{i}")), &FetchOptions::default());
                assert_eq!(res.unwrap().body, b"ok");
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(MAX_CONNECTIONS_PER_HOST, 6);
    assert_eq!(stats.max_open.load(Ordering::SeqCst), 6);
    assert_eq!(stats.accepted.load(Ordering::SeqCst), 6);
    assert_eq!(stats.requests().len(), 10);
    assert_eq!(f.open_connections(&url(port, "/")), 6);
    // Dos tandas de 300 ms: las 4 de la cola esperaron.
    assert!(start.elapsed() >= Duration::from_millis(550));
}

/// Los errores dicen su causa: conexión rechazada, respuesta que no es HTTP, conexión cortada
/// sin respuesta y nombre que no existe.
#[test]
fn network_errors_have_a_cause() {
    let f = Fetcher::new();
    let kind = |u: &Url| f.fetch(u, &FetchOptions::default()).unwrap_err().kind();
    // Nadie escucha en el puerto.
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    assert_eq!(kind(&url(port, "/")), NetErrorKind::ConnectionRefused);
    // Una respuesta que no es HTTP; una conexión que se cierra sin responder.
    for (reply, expected) in [
        (
            &b"hola, esto no es HTTP\r\n\r\n"[..],
            NetErrorKind::InvalidResponse,
        ),
        (&b""[..], NetErrorKind::ConnectionReset),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            s.write_all(reply).unwrap();
        });
        assert_eq!(kind(&url(port, "/")), expected);
        server.join().unwrap();
    }
    // RFC 6761 §6.4: `.invalid` no resuelve nunca.
    let u = Url::parse("http://freedom-test.invalid/", None).unwrap();
    assert_eq!(kind(&u), NetErrorKind::Dns);
}

/// RFC 5861 §3: caducada dentro de `stale-while-revalidate`, se sirve de la caché sin esperar y
/// el `Fetcher` la valida en otro hilo; el 304 la deja fresca otra vez. Pasada la ventana, se
/// valida antes de responder.
#[test]
fn stale_while_revalidate_in_the_background() {
    let handler = |_: &str, h: &[(String, String)]| {
        if h.iter().any(|(n, v)| n == "if-none-match" && v == "\"v1\"") {
            return (
                304,
                "Cache-Control: max-age=10, stale-while-revalidate=60\r\n".into(),
                Vec::new(),
            );
        }
        (
            200,
            "Cache-Control: max-age=10, stale-while-revalidate=60\r\nETag: \"v1\"\r\n".into(),
            b"uno".to_vec(),
        )
    };
    let (port, stats) = serve(Duration::ZERO, Arc::new(handler));
    let clock = Arc::new(AtomicU64::new(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    ));
    let c = clock.clone();
    let mut f = Fetcher::new();
    f.cache = Some(
        HttpCache::in_memory()
            .with_clock(move || UNIX_EPOCH + Duration::from_secs(c.load(Ordering::SeqCst))),
    );
    let u = url(port, "/swr");
    let get = || f.fetch(&u, &FetchOptions::default()).unwrap();
    assert_eq!(get().cache, CacheUse::Network);
    // Caducada hace 5 s (dentro de los 60): de la caché, y la validación va aparte.
    clock.fetch_add(15, Ordering::SeqCst);
    let res = get();
    assert_eq!(
        (res.cache, res.body.as_slice()),
        (CacheUse::Hit, &b"uno"[..])
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while stats.requests().len() < 2 {
        assert!(Instant::now() < deadline, "no llegó la validación");
        thread::sleep(Duration::from_millis(20));
    }
    let requests = stats.requests();
    assert!(
        requests[1]
            .1
            .iter()
            .any(|(n, v)| n == "if-none-match" && v == "\"v1\"")
    );
    // El 304 la refrescó: otra vez de la caché, sin red.
    thread::sleep(Duration::from_millis(300));
    assert_eq!(get().cache, CacheUse::Hit);
    assert_eq!(stats.requests().len(), 2);
    // Fuera de la ventana: se valida antes de responder.
    clock.fetch_add(200, Ordering::SeqCst);
    assert_eq!(get().cache, CacheUse::Revalidated);
    assert_eq!(stats.requests().len(), 3);
}
