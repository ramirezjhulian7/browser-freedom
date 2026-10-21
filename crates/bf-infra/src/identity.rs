//! Cómo se presenta Freedom a los sitios: el `User-Agent` (cabecera y `navigator.userAgent`),
//! las User-Agent Client Hints (`Sec-CH-UA*` y `navigator.userAgentData`) y
//! `navigator.platform`.
//!
//! Da los mismos datos que Chrome (sistema, arquitectura, móvil o no, marcas con versión),
//! con los mismos formatos y la misma reducción del `User-Agent`, pero se identifica como lo
//! que es: el producto y el motor son Freedom. No lleva `AppleWebKit`, `Chrome` ni `Safari`,
//! que harían que los sitios lo tomaran por Blink o WebKit.

/// El nombre del navegador y de su motor.
pub const NAME: &str = "Freedom";

/// La versión completa (`0.1.0`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// La versión mayor, la que llevan las marcas de baja entropía (`Sec-CH-UA`).
pub const MAJOR: &str = env!("CARGO_PKG_VERSION_MAJOR");

/// El `User-Agent` (HTML §8.9.1.1, «default User-Agent value»). Como Chrome, con el sistema
/// reducido a un valor fijo por plataforma (UA reduction): Windows 10/11 de 64 bits, macOS
/// en `Intel Mac OS X 10_15_7` y Linux en `x86_64`.
#[cfg(target_os = "windows")]
pub const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Freedom/",
    env!("CARGO_PKG_VERSION")
);
#[cfg(target_os = "macos")]
pub const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Freedom/",
    env!("CARGO_PKG_VERSION")
);
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (X11; Linux x86_64) Freedom/",
    env!("CARGO_PKG_VERSION")
);

/// `navigator.platform` (HTML §8.9.1.1), con los valores que usan los navegadores.
#[cfg(target_os = "windows")]
pub const NAVIGATOR_PLATFORM: &str = "Win32";
#[cfg(target_os = "macos")]
pub const NAVIGATOR_PLATFORM: &str = "MacIntel";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const NAVIGATOR_PLATFORM: &str = "Linux x86_64";

/// La plataforma de las Client Hints (`Sec-CH-UA-Platform`, `userAgentData.platform`).
#[cfg(target_os = "windows")]
pub const UA_PLATFORM: &str = "Windows";
#[cfg(target_os = "macos")]
pub const UA_PLATFORM: &str = "macOS";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const UA_PLATFORM: &str = "Linux";

/// `architecture` de las Client Hints de alta entropía.
#[cfg(target_arch = "aarch64")]
pub const ARCHITECTURE: &str = "arm";
#[cfg(not(target_arch = "aarch64"))]
pub const ARCHITECTURE: &str = "x86";

/// `bitness` de las Client Hints de alta entropía.
#[cfg(target_pointer_width = "64")]
pub const BITNESS: &str = "64";
#[cfg(not(target_pointer_width = "64"))]
pub const BITNESS: &str = "32";

/// La marca GREASE de las Client Hints (UA-CH §3.1, «GREASE-like brand»): obliga a los sitios
/// a leer la lista como lista y no a comparar un valor fijo.
pub const GREASE_BRAND: (&str, &str) = ("Not.A/Brand", "99");

/// Las marcas con la versión mayor (`brands`): Freedom y la de GREASE.
pub fn brands() -> [(&'static str, &'static str); 2] {
    [(NAME, MAJOR), GREASE_BRAND]
}

/// Las marcas con la versión completa (`fullVersionList`).
pub fn full_version_list() -> [(&'static str, &'static str); 2] {
    [(NAME, VERSION), (GREASE_BRAND.0, "99.0.0.0")]
}

/// Una lista de marcas como Structured Field (RFC 8941): `"Freedom";v="0", …`.
pub fn brand_header(list: &[(&str, &str)]) -> String {
    list.iter()
        .map(|(brand, version)| format!("\"{brand}\";v=\"{version}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Las Client Hints de baja entropía que Chrome manda en cada petición segura:
/// `Sec-CH-UA`, `Sec-CH-UA-Mobile` y `Sec-CH-UA-Platform`.
pub fn client_hint_headers() -> [(&'static str, String); 3] {
    [
        ("Sec-CH-UA", brand_header(&brands())),
        ("Sec-CH-UA-Mobile", "?0".to_owned()),
        ("Sec-CH-UA-Platform", format!("\"{UA_PLATFORM}\"")),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_as_freedom_not_chrome() {
        assert!(USER_AGENT.starts_with("Mozilla/5.0 ("));
        assert!(USER_AGENT.ends_with(&format!(") Freedom/{VERSION}")));
        for token in ["AppleWebKit", "Chrome", "Safari", "Gecko/", "Firefox"] {
            assert!(!USER_AGENT.contains(token), "{token} en {USER_AGENT}");
        }
    }

    #[test]
    fn client_hints() {
        let [ua, mobile, platform] = client_hint_headers();
        assert_eq!(
            ua.1,
            format!("\"Freedom\";v=\"{MAJOR}\", \"Not.A/Brand\";v=\"99\"")
        );
        assert_eq!(mobile.1, "?0");
        assert_eq!(platform.1, format!("\"{UA_PLATFORM}\""));
    }
}
