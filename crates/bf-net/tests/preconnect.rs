//! E3: las sugerencias de recursos que tocan la red sin pedir nada (HTML §4.6.7.4
//! `dns-prefetch` y §4.6.7.10 `preconnect`): `Fetcher::prefetch_dns` resuelve el nombre y lo
//! deja en la caché de DNS; `Fetcher::preconnect` abre la conexión y la deja libre en el pool
//! para la primera petición, sin mandar ninguna.

#![forbid(unsafe_code)]

use std::io::{self, BufRead, BufReader, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bf_net::dns::{Family, Resolver};
use bf_net::{Dns, FetchOptions, Fetcher};
use bf_url::Url;

/// Un servidor HTTP/1.1 con keep-alive que cuenta conexiones y peticiones.
fn server() -> (u16, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (conns, reqs) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (c, q) = (conns.clone(), reqs.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            c.fetch_add(1, Ordering::SeqCst);
            let q = q.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    loop {
                        let mut h = String::new();
                        if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                            break;
                        }
                    }
                    q.fetch_add(1, Ordering::SeqCst);
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nCache-Control: no-store\r\n\r\nok",
                    );
                }
            });
        }
    });
    (port, conns, reqs)
}

/// Espera (poco) a que `n` llegue a `want`.
fn settle(n: &AtomicUsize, want: usize) -> usize {
    for _ in 0..100 {
        if n.load(Ordering::SeqCst) >= want {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(50));
    n.load(Ordering::SeqCst)
}

#[test]
fn preconnect_opens_one_idle_connection_that_the_first_request_uses() {
    let (port, conns, reqs) = server();
    let f = Fetcher::new();
    let url = |p: &str| Url::parse(&format!("http://127.0.0.1:{port}{p}"), None).unwrap();
    assert!(f.preconnect(&url("/")));
    // Una conexión y ninguna petición.
    assert_eq!(settle(&conns, 1), 1);
    assert_eq!(reqs.load(Ordering::SeqCst), 0);
    assert_eq!(f.open_connections(&url("/")), 1);
    // Con una conexión abierta, otra preconexión no abre nada (una por origen).
    assert!(!f.preconnect(&url("/x")));
    // La primera petición va por la conexión abierta.
    let r = f.fetch(&url("/a"), &FetchOptions::default()).unwrap();
    assert_eq!(r.body, b"ok");
    assert_eq!(settle(&conns, 1), 1);
    assert_eq!(reqs.load(Ordering::SeqCst), 1);
    assert_eq!(*f.connections_opened.lock().unwrap(), 1);
    // Solo `http` y `https`.
    let data = Url::parse("data:text/plain,x", None).unwrap();
    assert!(!f.preconnect(&data));
    assert!(!f.prefetch_dns(&data));
}

#[test]
fn preconnect_to_a_closed_port_fails_quietly() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let f = Fetcher::new();
    let url = Url::parse(&format!("http://127.0.0.1:{port}/"), None).unwrap();
    assert!(!f.preconnect(&url));
    // El lugar del pool quedó libre.
    assert_eq!(f.open_connections(&url), 0);
}

/// Un resolvedor que da 127.0.0.1 a todo y cuenta las búsquedas.
struct Counting(AtomicUsize);

impl Resolver for Counting {
    fn lookup(&self, _host: &str, family: Family) -> io::Result<Vec<IpAddr>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(match family {
            Family::V4 => vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
            Family::V6 => Vec::new(),
        })
    }
}

#[test]
fn dns_prefetch_resolves_without_connecting_and_the_request_uses_the_cache() {
    let (port, conns, reqs) = server();
    let resolver = Arc::new(Counting(AtomicUsize::new(0)));
    let mut f = Fetcher::new();
    f.dns = Arc::new(Dns::new(resolver.clone()));
    let url = Url::parse(&format!("http://sugerencias.test:{port}/a"), None).unwrap();
    assert!(f.prefetch_dns(&url));
    let lookups = resolver.0.load(Ordering::SeqCst);
    assert!(lookups > 0);
    // Sin conexiones ni peticiones.
    assert_eq!(settle(&conns, 0), 0);
    assert_eq!(reqs.load(Ordering::SeqCst), 0);
    // La petición encuentra el nombre en la caché: no lo vuelve a resolver.
    let r = f.fetch(&url, &FetchOptions::default()).unwrap();
    assert_eq!(r.body, b"ok");
    assert_eq!(resolver.0.load(Ordering::SeqCst), lookups);
}
