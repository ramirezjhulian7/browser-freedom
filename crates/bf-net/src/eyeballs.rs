//! Happy Eyeballs, versión 2 (RFC 8305): conexiones TCP escalonadas entre IPv6 e IPv4.
//!
//! La resolución (§3) la hace `getaddrinfo` (vía [`std::net::ToSocketAddrs`]), que pide AAAA y A
//! a la vez y devuelve las dos familias juntas, ya ordenadas por RFC 6724; por eso no hay
//! *Resolution Delay* aquí. Lo que sigue es §4 (intercalar familias) y §5 (intentos escalonados
//! que siguen vivos en paralelo: el primero que conecta gana).

use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

/// §8: el *Connection Attempt Delay* recomendado.
pub const ATTEMPT_DELAY: Duration = Duration::from_millis(250);

/// §4: intercala las familias a partir de la de la primera dirección (*First Address Family
/// Count* = 1), conservando el orden de cada familia y sin repetidos.
pub fn interleave(addrs: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let mut seen = Vec::with_capacity(addrs.len());
    for a in addrs {
        if !seen.contains(&a) {
            seen.push(a);
        }
    }
    let Some(first_v6) = seen.first().map(SocketAddr::is_ipv6) else {
        return seen;
    };
    let (mut first, mut other): (VecDeque<_>, VecDeque<_>) =
        seen.into_iter().partition(|a| a.is_ipv6() == first_v6);
    let mut out = Vec::with_capacity(first.len() + other.len());
    while !first.is_empty() || !other.is_empty() {
        out.extend(first.pop_front());
        out.extend(other.pop_front());
    }
    out
}

/// §5: intenta `addrs` en orden. Cada intento empieza `delay` después del anterior, o en cuanto
/// el anterior falla; los que ya empezaron siguen en paralelo y gana el primero que conecta. Si
/// todos fallan, devuelve el último error. `timeout` acota la espera una vez lanzados todos (cada
/// `attempt` debería tener además su propio límite).
///
/// Los intentos perdedores terminan en su hilo y su conexión se cierra al soltarla.
pub fn race<T, F>(
    addrs: &[SocketAddr],
    delay: Duration,
    timeout: Duration,
    attempt: F,
) -> io::Result<T>
where
    T: Send + 'static,
    F: Fn(SocketAddr) -> io::Result<T> + Send + Sync + 'static,
{
    match addrs {
        [] => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "el host no tiene direcciones",
            ));
        }
        [only] => return attempt(*only),
        _ => {}
    }
    let attempt = Arc::new(attempt);
    let (tx, rx) = mpsc::channel();
    let mut next = 0;
    let mut pending = 0usize;
    let mut last = io::Error::new(io::ErrorKind::NotFound, "el host no tiene direcciones");
    loop {
        if let Some(&addr) = addrs.get(next) {
            next += 1;
            let tx = tx.clone();
            let attempt = Arc::clone(&attempt);
            let spawned = thread::Builder::new()
                .name("bf-net-connect".into())
                .spawn(move || {
                    // Si ya ganó otro intento, el receptor no está y la conexión se suelta.
                    let _ = tx.send(attempt(addr));
                });
            match spawned {
                Ok(_) => pending += 1,
                Err(e) => last = e,
            }
        }
        if pending == 0 {
            if next < addrs.len() {
                continue;
            }
            return Err(last);
        }
        let wait = if next < addrs.len() { delay } else { timeout };
        match rx.recv_timeout(wait) {
            Ok(Ok(conn)) => return Ok(conn),
            Ok(Err(e)) => {
                pending -= 1;
                last = e;
            }
            Err(mpsc::RecvTimeoutError::Timeout) if next < addrs.len() => {}
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "se agotó el tiempo de conexión",
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, TcpListener, TcpStream};
    use std::time::Instant;

    fn v4(n: u8) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::new(192, 0, 2, n), 80))
    }

    fn v6(n: u16) -> SocketAddr {
        SocketAddr::from((Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, n), 80))
    }

    #[test]
    fn interleaves_families_from_the_first() {
        assert_eq!(
            interleave(vec![v6(1), v6(2), v6(3), v4(1), v4(2)]),
            vec![v6(1), v4(1), v6(2), v4(2), v6(3)]
        );
        assert_eq!(
            interleave(vec![v4(1), v4(2), v6(1), v4(1)]),
            vec![v4(1), v6(1), v4(2)]
        );
        assert_eq!(interleave(vec![v4(1), v4(2)]), vec![v4(1), v4(2)]);
        assert!(interleave(Vec::new()).is_empty());
    }

    #[test]
    fn slow_first_attempt_does_not_block_the_next() {
        let t = Instant::now();
        let got = race(
            &[v6(1), v4(1)],
            Duration::from_millis(100),
            Duration::from_secs(10),
            |a| {
                if a.is_ipv6() {
                    thread::sleep(Duration::from_secs(3));
                    Err(io::Error::from(io::ErrorKind::TimedOut))
                } else {
                    Ok(a)
                }
            },
        )
        .unwrap();
        assert_eq!(got, v4(1));
        let took = t.elapsed();
        assert!(took >= Duration::from_millis(100), "{took:?}");
        assert!(took < Duration::from_secs(2), "{took:?}");
    }

    #[test]
    fn earlier_attempt_can_still_win() {
        // El primero tarda más que el escalón pero menos que el segundo: gana el primero.
        let got = race(
            &[v6(1), v4(1)],
            Duration::from_millis(50),
            Duration::from_secs(10),
            |a| {
                let ms = if a.is_ipv6() { 200 } else { 2000 };
                thread::sleep(Duration::from_millis(ms));
                Ok(a)
            },
        )
        .unwrap();
        assert_eq!(got, v6(1));
    }

    #[test]
    fn failure_starts_the_next_attempt_at_once() {
        let t = Instant::now();
        let got = race(
            &[v6(1), v6(2), v4(1)],
            Duration::from_secs(5),
            Duration::from_secs(10),
            |a| {
                if a == v4(1) {
                    Ok(a)
                } else {
                    Err(io::Error::from(io::ErrorKind::ConnectionRefused))
                }
            },
        )
        .unwrap();
        assert_eq!(got, v4(1));
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    }

    #[test]
    fn all_failing_returns_the_last_error() {
        let err = race(
            &[v6(1), v4(1)],
            Duration::from_millis(10),
            Duration::from_secs(10),
            |a| -> io::Result<()> { Err(io::Error::other(a.to_string())) },
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg == v6(1).to_string() || msg == v4(1).to_string(),
            "{msg}"
        );
        assert!(race(&[], ATTEMPT_DELAY, ATTEMPT_DELAY, |_| Ok(())).is_err());
    }

    #[test]
    fn hanging_attempts_time_out() {
        let t = Instant::now();
        let err = race(
            &[v6(1), v4(1)],
            Duration::from_millis(10),
            Duration::from_millis(200),
            |_| -> io::Result<()> {
                thread::sleep(Duration::from_secs(3));
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    }

    /// El caso real: un servidor solo en 127.0.0.1 y `::1` primero. En Windows, un puerto cerrado
    /// en `::1` tarda ~2 s en rechazar (reintentos de SYN); con los intentos escalonados la
    /// conexión llega por IPv4 tras el primer escalón.
    #[test]
    fn loopback_ipv4_only_server() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let addrs = [
            SocketAddr::from((Ipv6Addr::LOCALHOST, port)),
            SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        ];
        let t = Instant::now();
        let stream = race(&addrs, ATTEMPT_DELAY, Duration::from_secs(10), |a| {
            TcpStream::connect_timeout(&a, Duration::from_secs(10))
        })
        .unwrap();
        assert_eq!(stream.peer_addr().unwrap(), addrs[1]);
        assert!(
            t.elapsed() < Duration::from_millis(1500),
            "{:?}",
            t.elapsed()
        );
    }
}
