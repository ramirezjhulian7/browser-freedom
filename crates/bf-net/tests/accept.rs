//! El `Accept` por *destination* (Fetch §4, el algoritmo *fetch*, el paso de `Accept`): una
//! imagen, una hoja o un marco sin `Accept` propio llevan el que recomienda Fetch, no `*/*`.
//! Con `*/*`, el servidor de teselas de OpenStreetMap respondía la imagen de bloqueo 418 a las
//! teselas que pedía su propia web.

#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

use bf_infra::{Destination, Url};
use bf_net::{FetchOptions, Fetcher};

/// Un servidor HTTP/1.1 que responde con el `Accept` que recibió.
fn server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut r = BufReader::new(stream.try_clone().unwrap());
            let mut accept = String::new();
            loop {
                let mut h = String::new();
                if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("accept:") {
                    accept = v.trim().to_owned();
                }
            }
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nCache-Control: no-store\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{accept}",
                    accept.len()
                )
                .as_bytes(),
            );
        }
    });
    port
}

#[test]
fn accept_follows_the_destination() {
    let port = server();
    let fetcher = Fetcher::new();
    let accept = |path: &str, destination: Option<Destination>, opts: FetchOptions| {
        let url = Url::parse(&format!("http://127.0.0.1:{port}/{path}"), None).unwrap();
        let opts = FetchOptions {
            destination,
            ..opts
        };
        String::from_utf8(fetcher.fetch(&url, &opts).unwrap().body).unwrap()
    };
    assert_eq!(
        accept("t.png", Some(Destination::Image), FetchOptions::default()),
        "image/png,image/svg+xml,image/*;q=0.8,*/*;q=0.5"
    );
    assert_eq!(
        accept("a.css", Some(Destination::Style), FetchOptions::default()),
        "text/css,*/*;q=0.1"
    );
    assert_eq!(
        accept("m.html", Some(Destination::Frame), FetchOptions::default()),
        bf_net::DOCUMENT_ACCEPT
    );
    assert_eq!(
        accept("s.js", Some(Destination::Script), FetchOptions::default()),
        "*/*"
    );
    assert_eq!(accept("x", None, FetchOptions::default()), "*/*");
    // Un `Accept` propio no se toca.
    assert_eq!(
        accept("d", Some(Destination::Image), FetchOptions::document()),
        bf_net::DOCUMENT_ACCEPT
    );
    // Las peticiones de los scripts y del motor (`fetch_request`): las `<img>` del documento
    // también, salvo con un `Accept` del autor.
    let request = |destination, accept: Option<&str>| {
        let url = Url::parse(&format!("http://127.0.0.1:{port}/r"), None).unwrap();
        let mut req = bf_net::RequestOptions {
            destination,
            ..bf_net::RequestOptions::default()
        };
        if let Some(a) = accept {
            req.headers.push("Accept", a);
        }
        let (res, _) = fetcher.fetch_request(&url, &req).unwrap();
        String::from_utf8(res.body).unwrap()
    };
    assert_eq!(
        request(Some(Destination::Image), None),
        "image/png,image/svg+xml,image/*;q=0.8,*/*;q=0.5"
    );
    assert_eq!(request(None, None), "*/*");
    assert_eq!(
        request(Some(Destination::Image), Some("image/webp")),
        "image/webp"
    );
}
