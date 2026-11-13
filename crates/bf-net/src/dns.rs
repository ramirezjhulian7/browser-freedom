//! Resolución de nombres con caché (TTL acotado) y las dos familias en paralelo (Happy
//! Eyeballs v2, RFC 8305 §3).
//!
//! El [`Resolver`] es inyectable (tests, un resolvedor propio más adelante); el del sistema
//! ([`SystemResolver`]) usa `getaddrinfo`, que no da el TTL de los registros: por eso la caché
//! guarda cada resultado un tiempo fijo y corto ([`DEFAULT_TTL`], como la caché de host de
//! Chrome con el resolvedor del sistema).
//!
//! §3: se piden AAAA y A a la vez. Si llega primero AAAA, se conecta con eso sin esperar; si
//! llega primero A, se espera a AAAA hasta el *Resolution Delay* (50 ms). La familia que
//! no llegó a tiempo queda de reserva ([`Resolution::late`]): si fallan todas las conexiones
//! con la primera, se prueba con esa.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// §8: el *Resolution Delay* recomendado.
pub const RESOLUTION_DELAY: Duration = Duration::from_millis(50);

/// Cuánto vale una respuesta guardada.
pub const DEFAULT_TTL: Duration = Duration::from_secs(60);

/// Cuánto se espera a un resolvedor.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

/// Una familia de direcciones: los registros AAAA o A.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    V6,
    V4,
}

impl Family {
    fn of(ip: &IpAddr) -> Family {
        if ip.is_ipv6() { Family::V6 } else { Family::V4 }
    }
}

/// Quien resuelve nombres.
pub trait Resolver: Send + Sync {
    /// Las direcciones de `host` de la familia `family` (vacío si no tiene).
    fn lookup(&self, host: &str, family: Family) -> io::Result<Vec<IpAddr>>;
}

/// `getaddrinfo` (por [`ToSocketAddrs`]): devuelve las dos familias juntas y se filtra.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemResolver;

impl Resolver for SystemResolver {
    fn lookup(&self, host: &str, family: Family) -> io::Result<Vec<IpAddr>> {
        let mut out = Vec::new();
        for a in (host, 0).to_socket_addrs()? {
            let ip = a.ip();
            if Family::of(&ip) == family && !out.contains(&ip) {
                out.push(ip);
            }
        }
        Ok(out)
    }
}

/// ¿Tiene el equipo alguna ruta hacia fuera? Distingue "sin conexión" de "el nombre no
/// existe" cuando falla la resolución: `getaddrinfo` da el mismo error en los dos casos.
///
/// `connect` de un socket UDP solo consulta la tabla de rutas (no manda ningún paquete) y
/// falla con `NetworkUnreachable` si no hay ruta. Se prueba con direcciones reservadas para
/// documentación (RFC 5737 §3, RFC 3849), que nunca reciben tráfico.
pub fn has_route() -> bool {
    let probe = |bind: &str, to: &str| UdpSocket::bind(bind).and_then(|s| s.connect(to)).is_ok();
    probe("0.0.0.0:0", "192.0.2.1:9") || probe("[::]:0", "[2001:db8::1]:9")
}

/// Lo resuelto de un host: la familia que llegó primero (o las dos) y, si la otra no llegó a
/// tiempo, por dónde va a llegar.
pub struct Resolution {
    /// Las direcciones para conectar ya, IPv6 primero.
    pub addrs: Vec<IpAddr>,
    /// La familia que faltaba, cuando llegue (vacía si falló).
    pub late: Option<Receiver<Vec<IpAddr>>>,
}

#[derive(Debug, Clone)]
struct Cached {
    at: Instant,
    v6: Vec<IpAddr>,
    v4: Vec<IpAddr>,
}

/// Resolución con caché.
pub struct Dns {
    resolver: Arc<dyn Resolver>,
    ttl: Duration,
    cache: Arc<Mutex<HashMap<String, Cached>>>,
    /// Respuestas por familia de las búsquedas en curso: cuando están las dos, van a `cache`.
    partial: Arc<Mutex<HashMap<String, Partial>>>,
}

#[derive(Default)]
struct Partial {
    v6: Option<io::Result<Vec<IpAddr>>>,
    v4: Option<io::Result<Vec<IpAddr>>>,
}

impl Default for Dns {
    fn default() -> Self {
        Dns::new(Arc::new(SystemResolver))
    }
}

impl std::fmt::Debug for Dns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dns").field("ttl", &self.ttl).finish()
    }
}

impl Dns {
    pub fn new(resolver: Arc<dyn Resolver>) -> Dns {
        Dns {
            resolver,
            ttl: DEFAULT_TTL,
            cache: Arc::default(),
            partial: Arc::default(),
        }
    }

    /// Con otro tiempo de vida para lo guardado (`Duration::ZERO`: sin caché).
    pub fn with_ttl(mut self, ttl: Duration) -> Dns {
        self.ttl = ttl;
        self
    }

    /// Olvida lo guardado.
    pub fn clear(&self) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// Lo guardado para `host`, si sigue vigente.
    fn cached(&self, host: &str) -> Option<Cached> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        match cache.get(host) {
            Some(c) if c.at.elapsed() < self.ttl => Some(c.clone()),
            Some(_) => {
                cache.remove(host);
                None
            }
            None => None,
        }
    }

    /// §3: resuelve `host` (sin corchetes) con las dos familias en paralelo. Una IP literal no
    /// se resuelve.
    pub fn resolve(&self, host: &str) -> io::Result<Resolution> {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Ok(Resolution {
                addrs: vec![ip],
                late: None,
            });
        }
        let key = host.to_ascii_lowercase();
        if let Some(c) = self.cached(&key) {
            return Ok(Resolution {
                addrs: c.v6.into_iter().chain(c.v4).collect(),
                late: None,
            });
        }
        let (tx, rx) = mpsc::channel::<(Family, io::Result<Vec<IpAddr>>)>();
        for family in [Family::V6, Family::V4] {
            let tx = tx.clone();
            let resolver = self.resolver.clone();
            let (host, key) = (host.to_owned(), key.clone());
            let (cache, partial, ttl) = (self.cache.clone(), self.partial.clone(), self.ttl);
            thread::Builder::new()
                .name("bf-net-dns".into())
                .spawn(move || {
                    let result = resolver.lookup(&host, family);
                    remember(&cache, &partial, ttl, &key, family, &result);
                    let _ = tx.send((family, result));
                })?;
        }
        drop(tx);
        let mut v6: Option<io::Result<Vec<IpAddr>>> = None;
        let mut v4: Option<io::Result<Vec<IpAddr>>> = None;
        let mut last_error = None;
        // La primera respuesta; con A primero, AAAA hasta el *Resolution Delay*.
        let mut wait = LOOKUP_TIMEOUT;
        loop {
            match rx.recv_timeout(wait) {
                Ok((Family::V6, r)) => v6 = Some(r),
                Ok((Family::V4, r)) => v4 = Some(r),
                Err(_) => break,
            }
            let usable = |r: &Option<io::Result<Vec<IpAddr>>>| {
                r.as_ref()
                    .is_some_and(|r| r.as_ref().is_ok_and(|a| !a.is_empty()))
            };
            if (v6.is_some() && v4.is_some()) || usable(&v6) {
                break;
            }
            if usable(&v4) {
                wait = RESOLUTION_DELAY;
            }
        }
        let mut addrs = Vec::new();
        for r in [v6.as_mut(), v4.as_mut()].into_iter().flatten() {
            match r {
                Ok(a) => addrs.append(a),
                Err(e) => last_error = Some(io::Error::new(e.kind(), e.to_string())),
            }
        }
        let missing = v6.is_none() || v4.is_none();
        if addrs.is_empty() {
            // Nada todavía: la que falta es lo único que queda (si llega).
            if missing
                && let Ok((_, Ok(a))) = rx.recv_timeout(LOOKUP_TIMEOUT)
                && !a.is_empty()
            {
                return Ok(Resolution {
                    addrs: a,
                    late: None,
                });
            }
            return Err(last_error.unwrap_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "el host no tiene direcciones")
            }));
        }
        let late = missing.then(|| {
            let (late_tx, late_rx) = mpsc::channel();
            thread::spawn(move || {
                let a = match rx.recv_timeout(LOOKUP_TIMEOUT) {
                    Ok((_, Ok(a))) => a,
                    _ => Vec::new(),
                };
                let _ = late_tx.send(a);
            });
            late_rx
        });
        Ok(Resolution { addrs, late })
    }

    /// Todas las direcciones de `host` (espera a las dos familias).
    pub fn lookup_all(&self, host: &str) -> io::Result<Vec<IpAddr>> {
        let mut r = self.resolve(host)?;
        if let Some(late) = r.late.take() {
            r.addrs
                .extend(late.recv_timeout(LOOKUP_TIMEOUT).unwrap_or_default());
        }
        Ok(r.addrs)
    }

    /// Las direcciones de `host` con `port`, como [`SocketAddr`].
    pub fn socket_addrs(addrs: &[IpAddr], port: u16) -> Vec<SocketAddr> {
        addrs.iter().map(|ip| SocketAddr::new(*ip, port)).collect()
    }
}

/// Anota la respuesta de una familia; con las dos, va a la caché (si alguna dio direcciones).
fn remember(
    cache: &Mutex<HashMap<String, Cached>>,
    partial: &Mutex<HashMap<String, Partial>>,
    ttl: Duration,
    key: &str,
    family: Family,
    result: &io::Result<Vec<IpAddr>>,
) {
    let copy = match result {
        Ok(a) => Ok(a.clone()),
        Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
    };
    let mut partial = partial.lock().unwrap_or_else(|e| e.into_inner());
    let entry = partial.entry(key.to_owned()).or_default();
    match family {
        Family::V6 => entry.v6 = Some(copy),
        Family::V4 => entry.v4 = Some(copy),
    }
    if let (Some(v6), Some(v4)) = (&entry.v6, &entry.v4) {
        let v6 = v6.as_ref().ok().cloned().unwrap_or_default();
        let v4 = v4.as_ref().ok().cloned().unwrap_or_default();
        partial.remove(key);
        if !ttl.is_zero() && !(v6.is_empty() && v4.is_empty()) {
            cache.lock().unwrap_or_else(|e| e.into_inner()).insert(
                key.to_owned(),
                Cached {
                    at: Instant::now(),
                    v6,
                    v4,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Un resolvedor de prueba: direcciones fijas, una demora por familia y un contador.
    pub struct Fake {
        pub v6: (Vec<IpAddr>, Duration),
        pub v4: (Vec<IpAddr>, Duration),
        pub lookups: AtomicUsize,
    }

    impl Resolver for Fake {
        fn lookup(&self, _: &str, family: Family) -> io::Result<Vec<IpAddr>> {
            self.lookups.fetch_add(1, Ordering::SeqCst);
            let (addrs, delay) = match family {
                Family::V6 => &self.v6,
                Family::V4 => &self.v4,
            };
            thread::sleep(*delay);
            Ok(addrs.clone())
        }
    }

    fn v6() -> IpAddr {
        IpAddr::V6(Ipv6Addr::LOCALHOST)
    }

    fn v4() -> IpAddr {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    }

    fn fake(v6_delay: u64, v4_delay: u64) -> Arc<Fake> {
        Arc::new(Fake {
            v6: (vec![v6()], Duration::from_millis(v6_delay)),
            v4: (vec![v4()], Duration::from_millis(v4_delay)),
            lookups: AtomicUsize::new(0),
        })
    }

    #[test]
    fn both_families_and_the_cache() {
        let r = fake(0, 0);
        let dns = Dns::new(r.clone());
        assert_eq!(dns.lookup_all("a.test").unwrap(), [v6(), v4()]);
        assert_eq!(r.lookups.load(Ordering::SeqCst), 2);
        // De la caché, sin preguntar (y sin distinguir mayúsculas).
        let again = dns.resolve("A.test").unwrap();
        assert_eq!(
            (again.addrs, again.late.is_none()),
            (vec![v6(), v4()], true)
        );
        assert_eq!(r.lookups.load(Ordering::SeqCst), 2);
        // Sin caché, cada vez.
        let dns = Dns::new(r.clone()).with_ttl(Duration::ZERO);
        dns.lookup_all("a.test").unwrap();
        dns.lookup_all("a.test").unwrap();
        assert_eq!(r.lookups.load(Ordering::SeqCst), 6);
        // Las IP no se resuelven.
        assert_eq!(dns.resolve("127.0.0.1").unwrap().addrs, [v4()]);
        assert_eq!(r.lookups.load(Ordering::SeqCst), 6);
    }

    /// §3: con A primero se espera a AAAA solo el *Resolution Delay*; AAAA queda de reserva.
    #[test]
    fn a_first_waits_only_the_resolution_delay() {
        let dns = Dns::new(fake(1500, 0));
        let start = Instant::now();
        let r = dns.resolve("lento6.test").unwrap();
        assert!(
            start.elapsed() < Duration::from_millis(600),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(r.addrs, [v4()]);
        let late = r.late.expect("AAAA de reserva");
        assert_eq!(late.recv().unwrap(), [v6()]);
        // Cuando llegaron las dos, quedaron en la caché.
        let cached = dns.resolve("lento6.test").unwrap();
        assert_eq!(cached.addrs, [v6(), v4()]);
    }

    /// §3: con AAAA primero se conecta sin esperar a A.
    #[test]
    fn aaaa_first_does_not_wait() {
        let dns = Dns::new(fake(0, 1500));
        let start = Instant::now();
        let r = dns.resolve("lento4.test").unwrap();
        assert!(
            start.elapsed() < Duration::from_millis(600),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(r.addrs, [v6()]);
        assert_eq!(r.late.expect("A de reserva").recv().unwrap(), [v4()]);
    }

    #[test]
    fn no_addresses_is_an_error() {
        let r = Arc::new(Fake {
            v6: (Vec::new(), Duration::ZERO),
            v4: (Vec::new(), Duration::ZERO),
            lookups: AtomicUsize::new(0),
        });
        assert!(Dns::new(r).resolve("nada.test").is_err());
    }
}
