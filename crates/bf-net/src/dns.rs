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
//!
//! E6: §4 pide ordenar los destinos con la *Destination Address Selection* de RFC 6724 §6.
//! Pedir AAAA y A por separado pierde el orden que daría `getaddrinfo`, así que se aplica aquí
//! entre las dos familias ([`ipv4_first`]): con un origen IPv6 que no sirve para el destino
//! (sin ruta, de otro ámbito o de otra etiqueta, como una ULA de una VPN frente a un destino
//! global) va primero IPv4, y la espera del *Resolution Delay* es por la familia preferida.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv6Addr, SocketAddr, ToSocketAddrs, UdpSocket};
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

/// RFC 6724 §2.1: la precedencia y la etiqueta de la tabla de políticas por omisión
/// (el prefijo más largo que coincide). Las IPv4 cuentan como `::ffff:0:0/96`.
fn policy(ip: &IpAddr) -> (u8, u8) {
    let v6 = match ip {
        IpAddr::V4(_) => return (35, 4),
        IpAddr::V6(v6) => v6,
    };
    let seg = v6.segments();
    if *v6 == Ipv6Addr::LOCALHOST {
        (50, 0)
    } else if v6.to_ipv4_mapped().is_some() {
        (35, 4)
    } else if seg[..6] == [0; 6] {
        // ::/96 (las compatibles con IPv4, obsoletas).
        (1, 3)
    } else if seg[0] == 0x2001 && seg[1] == 0 {
        // 2001::/32 (Teredo).
        (5, 5)
    } else if seg[0] == 0x2002 {
        // 2002::/16 (6to4).
        (30, 2)
    } else if seg[0] == 0x3ffe {
        (1, 12)
    } else if seg[0] & 0xffc0 == 0xfec0 {
        (1, 11)
    } else if seg[0] & 0xfe00 == 0xfc00 {
        // fc00::/7 (ULA, RFC 4193).
        (3, 13)
    } else {
        (40, 1)
    }
}

/// RFC 6724 §3.1: el ámbito de una dirección (2: enlace, 5: sitio, 14: global). Las IPv4 de
/// bucle local y de enlace (169.254/16) son de enlace; una ULA es global (RFC 4193 §3.3).
fn scope(ip: &IpAddr) -> u8 {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_loopback() || v4.is_link_local() {
                2
            } else {
                14
            }
        }
        IpAddr::V6(v6) => {
            let seg0 = v6.segments()[0];
            if v6.is_multicast() {
                (seg0 & 0x000f) as u8
            } else if v6.is_loopback() || seg0 & 0xffc0 == 0xfe80 {
                2
            } else if seg0 & 0xffc0 == 0xfec0 {
                5
            } else {
                14
            }
        }
    }
}

/// La dirección de origen que usaría el sistema para llegar a `dest` (RFC 6724 §4, como hace
/// `getaddrinfo`): `connect` de un socket UDP solo consulta la tabla de rutas, sin mandar
/// nada. `None` si no hay ruta.
pub fn system_source(dest: IpAddr) -> Option<IpAddr> {
    let bind = if dest.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    };
    let s = UdpSocket::bind(bind).ok()?;
    s.connect(SocketAddr::new(dest, 9)).ok()?;
    s.local_addr().ok().map(|a| a.ip())
}

/// La dirección de origen para un destino ([`system_source`], inyectable en los tests).
pub type SourceProbe = dyn Fn(IpAddr) -> Option<IpAddr> + Send + Sync;

/// RFC 6724 §6 entre un destino IPv6 y uno IPv4, con el origen de cada uno según `source`:
/// ¿va primero el IPv4? Regla 1 (evitar destinos sin ruta), regla 2 (preferir el ámbito que
/// coincide con el del origen), regla 5 (preferir la etiqueta que coincide con la del origen)
/// y regla 6 (mayor precedencia); las reglas 3, 4, 7 y 8 no distinguen familias aquí y la 10
/// deja el orden (IPv6 primero).
pub fn ipv4_first(v6: IpAddr, v4: IpAddr, source: &SourceProbe) -> bool {
    let (s6, s4) = (source(v6), source(v4));
    let (s6, s4) = match (s6, s4) {
        (None, Some(_)) => return true,
        (_, None) => return false,
        (Some(a), Some(b)) => (a, b),
    };
    let scope_ok = |d: &IpAddr, s: &IpAddr| scope(d) == scope(s);
    if scope_ok(&v6, &s6) != scope_ok(&v4, &s4) {
        return scope_ok(&v4, &s4);
    }
    let label_ok = |d: &IpAddr, s: &IpAddr| policy(d).1 == policy(s).1;
    if label_ok(&v6, &s6) != label_ok(&v4, &s4) {
        return label_ok(&v4, &s4);
    }
    policy(&v4).0 > policy(&v6).0
}

/// Lo resuelto de un host: la familia que llegó primero (o las dos) y, si la otra no llegó a
/// tiempo, por dónde va a llegar.
pub struct Resolution {
    /// Las direcciones para conectar ya, la familia preferida (RFC 6724, [`ipv4_first`])
    /// primero.
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
    /// El origen que usaría el sistema para cada destino (RFC 6724).
    source: Arc<SourceProbe>,
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
            source: Arc::new(system_source),
        }
    }

    /// Con otra forma de saber el origen de cada destino (tests de RFC 6724).
    pub fn with_source_probe(
        mut self,
        probe: impl Fn(IpAddr) -> Option<IpAddr> + Send + Sync + 'static,
    ) -> Dns {
        self.source = Arc::new(probe);
        self
    }

    /// ¿Va primero IPv4 entre `v6` y `v4` (RFC 6724, [`ipv4_first`])? Sin una de las dos, la
    /// otra.
    fn v4_preferred(&self, v6: &[IpAddr], v4: &[IpAddr]) -> bool {
        match (v6.first(), v4.first()) {
            (Some(a), Some(b)) => ipv4_first(*a, *b, &*self.source),
            (Some(a), None) => {
                // Sin A todavía: contra una IPv4 de documentación (RFC 5737), solo para saber
                // si IPv4 tiene ruta y si el origen IPv6 sirve para el destino.
                let doc = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1));
                ipv4_first(*a, doc, &*self.source)
            }
            _ => true,
        }
    }

    /// `v6` y `v4` en el orden de RFC 6724.
    fn ordered(&self, v6: Vec<IpAddr>, v4: Vec<IpAddr>) -> Vec<IpAddr> {
        if self.v4_preferred(&v6, &v4) {
            v4.into_iter().chain(v6).collect()
        } else {
            v6.into_iter().chain(v4).collect()
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
                addrs: self.ordered(c.v6, c.v4),
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
        // La primera respuesta de la familia preferida; con la otra primero, la preferida
        // hasta el *Resolution Delay*. Sin RFC 6724 de por medio, la preferida es AAAA.
        let mut wait = LOOKUP_TIMEOUT;
        let usable = |r: &Option<io::Result<Vec<IpAddr>>>| {
            r.as_ref()
                .is_some_and(|r| r.as_ref().is_ok_and(|a| !a.is_empty()))
        };
        // E6: si AAAA llegó, ¿va detrás de A (RFC 6724)?
        let mut v6_behind: Option<bool> = None;
        loop {
            match rx.recv_timeout(wait) {
                Ok((Family::V6, r)) => v6 = Some(r),
                Ok((Family::V4, r)) => v4 = Some(r),
                Err(_) => break,
            }
            if v6_behind.is_none()
                && let Some(Ok(a)) = &v6
                && !a.is_empty()
            {
                let first_v4 = match &v4 {
                    Some(Ok(b)) => b.clone(),
                    _ => Vec::new(),
                };
                v6_behind = Some(self.v4_preferred(a, &first_v4));
            }
            let preferred_ready = if v6_behind == Some(true) {
                usable(&v4)
            } else {
                usable(&v6)
            };
            if (v6.is_some() && v4.is_some()) || preferred_ready {
                break;
            }
            if usable(&v4) || usable(&v6) {
                wait = RESOLUTION_DELAY;
            }
        }
        let mut take = |r: Option<&mut io::Result<Vec<IpAddr>>>| match r {
            Some(Ok(a)) => std::mem::take(a),
            Some(Err(e)) => {
                last_error = Some(io::Error::new(e.kind(), e.to_string()));
                Vec::new()
            }
            None => Vec::new(),
        };
        let (a6, a4) = (take(v6.as_mut()), take(v4.as_mut()));
        let addrs = if v6_behind == Some(true) {
            a4.into_iter().chain(a6).collect::<Vec<_>>()
        } else {
            a6.into_iter().chain(a4).collect()
        };
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

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// RFC 6724 §6 entre las dos familias, con el origen que daría el sistema.
    #[test]
    fn rfc6724_between_families() {
        let (d6, d4) = (ip("2620:0:862:ed1a::1"), ip("208.80.154.224"));
        let with = |s6: Option<&'static str>, s4: Option<&'static str>| {
            move |d: IpAddr| if d.is_ipv6() { s6 } else { s4 }.map(ip)
        };
        // Origen global: IPv6 primero (regla 6, precedencia 40 > 35).
        assert!(!ipv4_first(
            d6,
            d4,
            &with(Some("2600::5"), Some("192.168.1.2"))
        ));
        // Una ULA (la de una VPN) frente a un destino global: etiquetas 13 y 1 (regla 5).
        assert!(ipv4_first(
            d6,
            d4,
            &with(Some("fd7a:115c:a1e0::1"), Some("192.168.1.2"))
        ));
        // Origen de enlace para un destino global (regla 2).
        assert!(ipv4_first(
            d6,
            d4,
            &with(Some("fe80::1"), Some("192.168.1.2"))
        ));
        // Sin ruta IPv6 (regla 1); sin ruta IPv4, al revés.
        assert!(ipv4_first(d6, d4, &with(None, Some("192.168.1.2"))));
        assert!(!ipv4_first(d6, d4, &with(Some("2600::5"), None)));
        // Destino ULA con origen ULA: coinciden y gana IPv4 por precedencia (3 < 35).
        assert!(ipv4_first(
            ip("fd00::1"),
            d4,
            &with(Some("fd7a::1"), Some("192.168.1.2"))
        ));
        // Bucle local: ::1 primero (precedencia 50).
        assert!(!ipv4_first(
            v6(),
            v4(),
            &with(Some("::1"), Some("127.0.0.1"))
        ));
    }

    /// §3 y §4 con RFC 6724: si el origen IPv6 no sirve, A va primero y, con AAAA primero,
    /// se espera a A el *Resolution Delay*; si A no llega a tiempo, AAAA y A de reserva.
    #[test]
    fn ula_source_puts_a_first() {
        let fake = |v4_delay: u64| {
            Arc::new(Fake {
                v6: (vec![ip("2620:0:862:ed1a::1")], Duration::ZERO),
                v4: (vec![ip("208.80.154.224")], Duration::from_millis(v4_delay)),
                lookups: AtomicUsize::new(0),
            })
        };
        let probe = |d: IpAddr| {
            Some(if d.is_ipv6() {
                ip("fd7a:115c:a1e0::b201:f684")
            } else {
                ip("192.168.1.2")
            })
        };
        let dns = Dns::new(fake(10)).with_source_probe(probe);
        let r = dns.resolve("es.wikipedia.test").unwrap();
        assert_eq!(r.addrs, [ip("208.80.154.224"), ip("2620:0:862:ed1a::1")]);
        assert!(r.late.is_none());
        // De la caché, el mismo orden.
        let r = dns.resolve("es.wikipedia.test").unwrap();
        assert_eq!(r.addrs[0], ip("208.80.154.224"));
        // A muy tarde: no se espera más que el *Resolution Delay*.
        let dns = Dns::new(fake(1500)).with_source_probe(probe);
        let start = Instant::now();
        let r = dns.resolve("lento.test").unwrap();
        assert!(
            start.elapsed() < Duration::from_millis(600),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(r.addrs, [ip("2620:0:862:ed1a::1")]);
        assert_eq!(
            r.late.expect("A de reserva").recv().unwrap(),
            [ip("208.80.154.224")]
        );
        // Con un origen IPv6 global, AAAA primero y sin esperar.
        let dns = Dns::new(fake(1500)).with_source_probe(|d: IpAddr| {
            Some(if d.is_ipv6() {
                ip("2600::5")
            } else {
                ip("192.168.1.2")
            })
        });
        let start = Instant::now();
        let r = dns.resolve("rapido.test").unwrap();
        assert!(
            start.elapsed() < Duration::from_millis(40),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(r.addrs, [ip("2620:0:862:ed1a::1")]);
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
