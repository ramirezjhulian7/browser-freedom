//! Los errores de red de `bf-net` y su causa, para que el shell muestre una página de error
//! clara (qué falló: el nombre, la conexión, el tiempo, el certificado o la respuesta).
//!
//! [`FetchError`] conserva sus variantes (el shell y `bf-web` las nombran); la clasificación
//! fina está en [`FetchError::kind`], que devuelve un [`NetErrorKind`]. Los errores de TLS
//! llevan el motivo ([`TlsReason`]) además del texto de rustls.

use std::io;

/// Un error de una petición de red.
#[derive(Debug)]
pub enum FetchError {
    UnsupportedScheme(String),
    InvalidUrl,
    /// Error de E/S: conexión rechazada, cortada, red inalcanzable o respuesta inválida
    /// (`io::ErrorKind::InvalidData`); [`FetchError::kind`] lo distingue.
    Io(io::Error),
    /// No se pudo resolver el nombre del host.
    Dns(io::Error),
    /// Se agotó el tiempo de conexión o de lectura.
    Timeout,
    /// El handshake TLS falló o el certificado no es válido; [`TlsError::reason`] dice por qué.
    Tls(TlsError),
    TooManyRedirects,
    /// `file:` o `data:` que no se pudo leer.
    NotFound,
}

impl FetchError {
    /// E6: el mismo error (para las peticiones que compartieron la red, que reciben cada una
    /// el suyo).
    pub fn duplicate(&self) -> FetchError {
        let io = |e: &io::Error| io::Error::new(e.kind(), e.to_string());
        match self {
            FetchError::UnsupportedScheme(s) => FetchError::UnsupportedScheme(s.clone()),
            FetchError::InvalidUrl => FetchError::InvalidUrl,
            FetchError::Io(e) => FetchError::Io(io(e)),
            FetchError::Dns(e) => FetchError::Dns(io(e)),
            FetchError::Timeout => FetchError::Timeout,
            FetchError::Tls(t) => FetchError::Tls(t.clone()),
            FetchError::TooManyRedirects => FetchError::TooManyRedirects,
            FetchError::NotFound => FetchError::NotFound,
        }
    }
}

/// La causa de un [`FetchError`], para elegir el mensaje de la página de error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetErrorKind {
    /// No se pudo resolver el nombre (DNS).
    Dns,
    /// El servidor rechazó la conexión (nadie escucha en ese puerto: RST al SYN).
    ConnectionRefused,
    /// La conexión se cortó o se cerró antes de terminar la respuesta.
    ConnectionReset,
    /// No hay ruta a la red o al host.
    Unreachable,
    /// Se agotó el tiempo de conexión o de espera de la respuesta.
    Timeout,
    /// TLS: el certificado o el handshake.
    Tls(TlsReason),
    /// El servidor respondió algo que no es HTTP válido (o un cuerpo que no se puede
    /// decodificar).
    InvalidResponse,
    TooManyRedirects,
    InvalidUrl,
    UnsupportedScheme,
    /// `file:`/`data:`/`blob:` que no existe.
    NotFound,
    /// Cualquier otro error de E/S.
    Other,
}

/// Por qué falló TLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsReason {
    /// El certificado no lo emite una autoridad de confianza (o es autofirmado).
    UnknownIssuer,
    /// El certificado venció.
    Expired,
    /// El certificado todavía no es válido (o el reloj del sistema está atrasado).
    NotYetValid,
    /// El certificado no es para este nombre.
    NameMismatch,
    Revoked,
    /// Otro problema del certificado (codificación, firma, extensión, algoritmo).
    BadCertificate,
    /// El handshake falló (alerta del servidor, versiones o cifrados incompatibles, ALPN).
    Handshake,
    Other,
}

/// Un error de TLS: el motivo y el texto de rustls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsError {
    pub reason: TlsReason,
    pub message: String,
}

impl TlsError {
    pub fn new(reason: TlsReason, message: impl Into<String>) -> TlsError {
        TlsError {
            reason,
            message: message.into(),
        }
    }

    /// Un error de rustls, con su motivo.
    pub fn from_rustls(e: &rustls::Error) -> TlsError {
        use rustls::CertificateError as C;
        let reason = match e {
            rustls::Error::InvalidCertificate(c) => match c {
                C::UnknownIssuer => TlsReason::UnknownIssuer,
                C::Expired | C::ExpiredContext { .. } => TlsReason::Expired,
                C::NotValidYet | C::NotValidYetContext { .. } => TlsReason::NotYetValid,
                C::NotValidForName | C::NotValidForNameContext { .. } => TlsReason::NameMismatch,
                C::Revoked => TlsReason::Revoked,
                _ => TlsReason::BadCertificate,
            },
            rustls::Error::NoCertificatesPresented
            | rustls::Error::UnsupportedNameType
            | rustls::Error::InvalidCertRevocationList(_) => TlsReason::BadCertificate,
            rustls::Error::AlertReceived(_)
            | rustls::Error::PeerIncompatible(_)
            | rustls::Error::PeerMisbehaved(_)
            | rustls::Error::InappropriateMessage { .. }
            | rustls::Error::InappropriateHandshakeMessage { .. }
            | rustls::Error::InvalidMessage(_)
            | rustls::Error::DecryptError
            | rustls::Error::NoApplicationProtocol
            | rustls::Error::HandshakeNotComplete
            | rustls::Error::PeerSentOversizedRecord => TlsReason::Handshake,
            _ => TlsReason::Other,
        };
        TlsError::new(reason, e.to_string())
    }
}

impl std::fmt::Display for TlsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl FetchError {
    /// La causa del error.
    pub fn kind(&self) -> NetErrorKind {
        match self {
            FetchError::UnsupportedScheme(_) => NetErrorKind::UnsupportedScheme,
            FetchError::InvalidUrl => NetErrorKind::InvalidUrl,
            FetchError::Dns(_) => NetErrorKind::Dns,
            FetchError::Timeout => NetErrorKind::Timeout,
            FetchError::Tls(e) => NetErrorKind::Tls(e.reason),
            FetchError::TooManyRedirects => NetErrorKind::TooManyRedirects,
            FetchError::NotFound => NetErrorKind::NotFound,
            FetchError::Io(e) => {
                use io::ErrorKind as K;
                match e.kind() {
                    K::ConnectionRefused => NetErrorKind::ConnectionRefused,
                    K::ConnectionReset
                    | K::ConnectionAborted
                    | K::BrokenPipe
                    | K::UnexpectedEof
                    | K::NotConnected => NetErrorKind::ConnectionReset,
                    K::NetworkUnreachable
                    | K::HostUnreachable
                    | K::AddrNotAvailable
                    | K::NetworkDown => NetErrorKind::Unreachable,
                    K::TimedOut | K::WouldBlock => NetErrorKind::Timeout,
                    K::InvalidData => NetErrorKind::InvalidResponse,
                    _ => NetErrorKind::Other,
                }
            }
        }
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::UnsupportedScheme(s) => write!(f, "esquema no soportado: {s}"),
            FetchError::InvalidUrl => f.write_str("URL inválida"),
            FetchError::Io(e) => write!(f, "error de red: {e}"),
            FetchError::Dns(e) => write!(f, "no se pudo resolver el host: {e}"),
            FetchError::Timeout => f.write_str("se agotó el tiempo de espera"),
            FetchError::Tls(e) => write!(f, "error de TLS: {e}"),
            FetchError::TooManyRedirects => f.write_str("demasiadas redirecciones"),
            FetchError::NotFound => f.write_str("recurso no encontrado"),
        }
    }
}

impl std::error::Error for FetchError {}

impl From<io::Error> for FetchError {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => FetchError::Timeout,
            _ => FetchError::Io(e),
        }
    }
}

/// Un error de E/S del handshake: si viene de rustls, es un error de TLS con su motivo.
pub(crate) fn tls_io_error(e: io::Error) -> FetchError {
    if let Some(t) = e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>())
    {
        return FetchError::Tls(TlsError::from_rustls(t));
    }
    FetchError::from(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        let io = |k| FetchError::Io(io::Error::new(k, "x"));
        assert_eq!(
            io(io::ErrorKind::ConnectionRefused).kind(),
            NetErrorKind::ConnectionRefused
        );
        assert_eq!(
            io(io::ErrorKind::UnexpectedEof).kind(),
            NetErrorKind::ConnectionReset
        );
        assert_eq!(
            io(io::ErrorKind::InvalidData).kind(),
            NetErrorKind::InvalidResponse
        );
        assert_eq!(
            FetchError::from(io::Error::from(io::ErrorKind::TimedOut)).kind(),
            NetErrorKind::Timeout
        );
        let cert = |c| TlsError::from_rustls(&rustls::Error::InvalidCertificate(c)).reason;
        assert_eq!(
            cert(rustls::CertificateError::UnknownIssuer),
            TlsReason::UnknownIssuer
        );
        assert_eq!(cert(rustls::CertificateError::Expired), TlsReason::Expired);
        assert_eq!(
            cert(rustls::CertificateError::NotValidForName),
            TlsReason::NameMismatch
        );
        assert_eq!(
            cert(rustls::CertificateError::BadSignature),
            TlsReason::BadCertificate
        );
        let e = FetchError::Tls(TlsError::from_rustls(&rustls::Error::NoApplicationProtocol));
        assert_eq!(e.kind(), NetErrorKind::Tls(TlsReason::Handshake));
        assert!(e.to_string().starts_with("error de TLS: "));
    }
}
