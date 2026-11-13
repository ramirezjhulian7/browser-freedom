//! E6: el tiempo de las peticiones de un hilo por partes (para las trazas `BF_TRACE` del
//! navegador): resolución DNS, conexión TCP, TLS, arranque de HTTP/2, espera de la respuesta,
//! cuerpo, descompresión y caché HTTP (decidir, leer del disco, guardar). Se acumula en el hilo
//! que llama a [`crate::Fetcher::fetch`] (las conexiones HTTP/2 leen en otro hilo: su tiempo
//! cuenta como `h2` desde que se manda la petición hasta que llega la respuesta entera) y se
//! recoge con [`take`]. Las validaciones en segundo plano (`stale-while-revalidate`) corren en
//! su hilo y no cuentan.

use std::cell::RefCell;
use std::time::{Duration, Instant};

/// Lo acumulado desde el último [`take`].
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct NetTimings {
    pub dns: Duration,
    pub tcp: Duration,
    pub tls: Duration,
    /// El prefacio y los `SETTINGS` de una conexión HTTP/2 nueva.
    pub h2_setup: Duration,
    /// HTTP/1.1: de escribir la petición a tener la cabecera de la respuesta.
    pub wait: Duration,
    /// HTTP/1.1: leer el cuerpo.
    pub body: Duration,
    /// HTTP/2: la petición entera por la conexión (espera y cuerpo).
    pub h2: Duration,
    pub decode: Duration,
    /// La caché HTTP sin la red: buscar, leer el cuerpo del disco y guardar.
    pub cache: Duration,
    /// Peticiones a la red y conexiones nuevas.
    pub requests: u32,
    pub connections: u32,
}

impl NetTimings {
    /// La suma de las partes.
    pub fn total(&self) -> Duration {
        self.dns
            + self.tcp
            + self.tls
            + self.h2_setup
            + self.wait
            + self.body
            + self.h2
            + self.decode
            + self.cache
    }
}

thread_local! {
    static TIMINGS: RefCell<NetTimings> = RefCell::new(NetTimings::default());
}

/// Lo acumulado en este hilo desde la última vez (y empieza de cero).
pub fn take() -> NetTimings {
    TIMINGS.with(|t| std::mem::take(&mut *t.borrow_mut()))
}

/// Suma `d` a la parte que elige `part`.
pub(crate) fn add(part: fn(&mut NetTimings) -> &mut Duration, d: Duration) {
    TIMINGS.with(|t| *part(&mut t.borrow_mut()) += d);
}

/// Corre `f` y suma su tiempo a `part`.
pub(crate) fn time<R>(part: fn(&mut NetTimings) -> &mut Duration, f: impl FnOnce() -> R) -> R {
    let start = Instant::now();
    let r = f();
    add(part, start.elapsed());
    r
}

/// Cuenta una petición a la red o una conexión nueva.
pub(crate) fn count(connection: bool) {
    TIMINGS.with(|t| {
        let mut t = t.borrow_mut();
        if connection {
            t.connections += 1;
        } else {
            t.requests += 1;
        }
    });
}
