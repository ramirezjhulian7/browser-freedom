//! E3: el tiempo de TLS con una sesión nueva y con una reanudada (RFC 8446 §2.2) contra un
//! servidor real. Necesita red: `cargo test -p bf-net --release --test tls_resume_live --
//! --ignored --nocapture`. Cada `Fetcher` abre su propia conexión; la caché de sesiones es la
//! de la configuración TLS del proceso, así que el primero hace el handshake completo y los
//! siguientes lo reanudan.

#![forbid(unsafe_code)]

use bf_net::{FetchOptions, Fetcher};
use bf_url::Url;

#[test]
#[ignore = "necesita red"]
fn tls_time_full_and_resumed() {
    let url = Url::parse(
        &std::env::var("BF_TLS_URL")
            .unwrap_or_else(|_| "https://es.wikipedia.org/robots.txt".into()),
        None,
    )
    .unwrap();
    let mut rows = Vec::new();
    for _ in 0..6 {
        let f = Fetcher::new();
        let _ = bf_net::timing::take();
        f.fetch(&url, &FetchOptions::default()).unwrap();
        let t = bf_net::timing::take();
        rows.push((
            t.resumed == 1,
            t.tls.as_secs_f64() * 1000.0,
            t.tcp.as_secs_f64() * 1000.0,
        ));
    }
    for (resumed, tls, tcp) in &rows {
        println!(
            "TLS {tls:.1} ms ({}), TCP {tcp:.1} ms",
            if *resumed { "reanudado" } else { "completo" }
        );
    }
    // Cada ticket sirve una vez (RFC 8446 §C.4); un servidor que manda dos por handshake
    // completo y ninguno al reanudar (Wikipedia) da: completo, reanudado, reanudado, completo…
    assert!(!rows[0].0, "el primero es completo");
    assert!(rows[1].0, "el segundo se reanuda: {rows:?}");
}
