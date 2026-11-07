//! E6: la caché HTTP en el runner de WPT y el handler `fetch/http-cache/resources/http-cache.py`
//! emulado.
//!
//! Cada documento de test tiene una [`Session`]: la caché HTTP de `bf-net` (RFC 9111) por la
//! que pasa cada salto de [`super::wptserve::WptServe`], y el *stash* de wptserve donde
//! `http-cache.py` guarda lo que vio el servidor. El reloj de los dos es la hora real más el
//! tiempo virtual del agente (el runner adelanta los temporizadores sin esperar), para que el
//! `pause()` de 3 s de `http-cache.js` haga caducar lo que tiene que caducar.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bf_infra::Url;
use bf_net::cache::{CacheRequest, CacheResponse};
use bf_net::{CacheMode, CacheUse, Headers, HttpCache};

use super::headers::HeaderList;
use super::{NetResponse, NetResult};

/// El estado del servidor y de la caché de un documento de test.
pub struct Session {
    cache: HttpCache,
    /// `http-cache.py`: `request.server.stash`, por `uuid`.
    stash: Mutex<HashMap<String, Json>>,
    /// El tiempo virtual del agente, en ms.
    virtual_ms: Arc<AtomicU64>,
}

impl Default for Session {
    fn default() -> Self {
        let virtual_ms = Arc::new(AtomicU64::new(0));
        let v = virtual_ms.clone();
        Session {
            cache: HttpCache::in_memory().with_clock(move || clock(&v)),
            stash: Mutex::new(HashMap::new()),
            virtual_ms,
        }
    }
}

fn clock(virtual_ms: &AtomicU64) -> SystemTime {
    SystemTime::now() + Duration::from_millis(virtual_ms.load(Ordering::SeqCst))
}

impl Session {
    /// El agente va por `ms` en su reloj (nunca hacia atrás).
    pub fn set_virtual_ms(&self, ms: f64) {
        if ms.is_finite() && ms > 0.0 {
            self.virtual_ms.fetch_max(ms as u64, Ordering::SeqCst);
        }
    }

    fn now(&self) -> f64 {
        clock(&self.virtual_ms)
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64())
    }

    /// Un salto por la caché: `network` es el servidor (recibe las cabeceras a mandar).
    pub fn hop(
        &self,
        method: &str,
        url: &Url,
        headers: &HeaderList,
        mode: CacheMode,
        credentials: bool,
        network: impl FnOnce(HeaderList) -> Option<NetResponse>,
    ) -> NetResult {
        let req_headers = Headers(headers.0.clone());
        let req = CacheRequest {
            method,
            url,
            headers: &req_headers,
            mode,
            credentials,
        };
        let (r, used) = self.cache.fetch(&req, |h| {
            let r = network(HeaderList(h.0.clone()))
                .ok_or_else(|| format!("wptserve: no se pudo servir {}", url.href()))?;
            Ok::<_, String>(CacheResponse {
                status: r.status,
                status_text: r.status_text,
                headers: Headers(r.headers.0),
                body: r.body,
            })
        })?;
        // Fetch §4.5: `only-if-cached` sin nada guardado es un error de red.
        if used == CacheUse::Unavailable && mode == CacheMode::OnlyIfCached {
            return Err("only-if-cached: no está en la caché".into());
        }
        Ok(NetResponse {
            url_list: Vec::new(),
            status: r.status,
            status_text: r.status_text,
            headers: HeaderList(r.headers.0),
            body: r.body,
        })
    }

    /// `http-cache.py` (`main`): `dispatch=test` responde según la configuración que manda el
    /// test en `Test-Requests` y apunta lo que vio; `dispatch=state` devuelve lo apuntado.
    pub fn handle(&self, method: &str, url: &Url, headers: &HeaderList) -> NetResponse {
        let get: Vec<(String, String)> = url
            .query
            .as_deref()
            .map(|q| bf_url::form::parse(q.as_bytes()))
            .unwrap_or_default();
        let param = |k: &str| get.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let mut out = HeaderList::default();
        out.set("Access-Control-Allow-Credentials", "true");
        if method.eq_ignore_ascii_case("OPTIONS") {
            let origin = headers.get("origin").unwrap_or_else(|| "*".into());
            let allow = headers
                .get("access-control-request-headers")
                .unwrap_or_else(|| "*".into());
            out.set("Access-Control-Allow-Origin", &origin);
            out.set("Access-Control-Allow-Methods", "GET");
            out.set("Access-Control-Allow-Headers", &allow);
            out.set("Access-Control-Max-Age", "86400");
            return response(200, "OK", out, b"Preflight request".to_vec());
        }
        let not_found = |mut out: HeaderList, body: &[u8]| {
            out.set("Content-Type", "text/plain");
            response(404, "Not Found", out, body.to_vec())
        };
        let Some(uuid) = param("uuid").filter(|u| !u.is_empty()) else {
            return not_found(out, b"UUID not found");
        };
        match param("dispatch").as_deref() {
            Some("test") => self.handle_test(&uuid, method, url, headers, out),
            Some("state") => {
                out.set("Content-Type", "text/plain");
                let state = self.take(&uuid).unwrap_or(Json::Null);
                response(200, "OK", out, state.to_string().into_bytes())
            }
            _ => not_found(out, b"Fallthrough"),
        }
    }

    fn take(&self, uuid: &str) -> Option<Json> {
        self.stash
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(uuid)
    }

    fn handle_test(
        &self,
        uuid: &str,
        method: &str,
        url: &Url,
        headers: &HeaderList,
        mut out: HeaderList,
    ) -> NetResponse {
        let mut server_state = match self.take(uuid) {
            Some(Json::Arr(a)) => a,
            _ => Vec::new(),
        };
        let requests = headers
            .get("test-requests")
            .and_then(|b| bf_url::data::forgiving_base64_decode(&b))
            .and_then(|b| Json::parse(&String::from_utf8_lossy(&b)));
        let Some(Json::Arr(requests)) = requests else {
            out.set("Content-Type", "text/plain");
            return response(
                400,
                "Bad Request",
                out,
                b"No or bad Test-Requests request header".to_vec(),
            );
        };
        let Some(config) = requests.get(server_state.len()) else {
            out.set("Content-Type", "text/plain");
            return response(500, "Internal Server Error", out, b"IndexError".to_vec());
        };
        let mut noted: Vec<(String, Json)> = Vec::new();
        let now = self.now();
        let href = url.href();
        if let Some(Json::Arr(list)) = config.get("response_headers") {
            for header in list {
                let Json::Arr(h) = header else { continue };
                let (Some(Json::Str(name)), Some(value)) = (h.first(), h.get(1)) else {
                    continue;
                };
                let lower = name.to_ascii_lowercase();
                let value = match value {
                    // Las "magic locations": relativas a la URL de la petición.
                    _ if lower == "location" || lower == "content-location" => {
                        match value.as_text() {
                            v if v.is_empty() => href.clone(),
                            v => format!("{href}&target={v}"),
                        }
                    }
                    // Las "magic dates": segundos desde ahora.
                    Json::Num(n)
                        if n.fract() == 0.0
                            && matches!(lower.as_str(), "date" | "expires" | "last-modified") =>
                    {
                        http_date(now + n)
                    }
                    v => v.as_text(),
                };
                out.set(name, &value);
                if matches!(
                    lower.as_str(),
                    "content-type" | "access-control-allow-origin" | "last-modified" | "etag"
                ) {
                    noted.retain(|(n, _)| *n != lower);
                    noted.push((lower, Json::Str(value)));
                }
            }
        }
        let mut seen: Vec<(String, Json)> = Vec::new();
        for (n, _) in &headers.0 {
            let n = n.to_ascii_lowercase();
            if !seen.iter().any(|(k, _)| *k == n) {
                let v = headers.get(&n).unwrap_or_default();
                seen.push((n, Json::Str(v)));
            }
        }
        server_state.push(Json::Obj(vec![
            ("now".into(), Json::Num(now)),
            ("request_method".into(), Json::Str(method.to_owned())),
            ("request_headers".into(), Json::Obj(seen)),
            ("response_headers".into(), Json::Obj(noted.clone())),
        ]));
        let count = server_state.len();
        let first_noted = match server_state.first().and_then(|s| s.get("response_headers")) {
            Some(Json::Obj(o)) => o.clone(),
            _ => Vec::new(),
        };
        self.stash
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(uuid.to_owned(), Json::Arr(server_state));

        let has = |k: &str| noted.iter().any(|(n, _)| n == k);
        if !has("access-control-allow-origin") {
            out.set("Access-Control-Allow-Origin", "*");
        }
        if !has("content-type") {
            out.set("Content-Type", "text/plain");
        }
        out.set("Server-Request-Count", &count.to_string());

        let (mut code, mut phrase) = match config.get("response_status") {
            Some(Json::Arr(s)) => (
                s.first().and_then(Json::as_num).unwrap_or(200.0) as u16,
                s.get(1).map_or_else(|| "OK".into(), Json::as_text),
            ),
            _ => (200, "OK".to_owned()),
        };
        if config
            .get("expected_type")
            .is_some_and(|t| t.as_text().ends_with("validated"))
        {
            let previous = |k: &str| {
                first_noted
                    .iter()
                    .find(|(n, _)| n == k)
                    .map(|(_, v)| v.as_text())
            };
            if let Some(lm) = previous("last-modified")
                && headers.get("if-modified-since").as_deref() == Some(lm.as_str())
            {
                (code, phrase) = (304, "Not Modified".into());
            }
            if let Some(etag) = previous("etag")
                && headers.get("if-none-match").as_deref() == Some(etag.as_str())
            {
                (code, phrase) = (304, "Not Modified".into());
            }
            if code != 304 {
                (code, phrase) = (999, "304 Not Generated".into());
            }
        }
        let body = match config.get("response_body") {
            None => uuid.as_bytes().to_vec(),
            Some(Json::Null) => Vec::new(),
            Some(b) => b.as_text().into_bytes(),
        };
        let body = if matches!(code, 204 | 304) {
            Vec::new()
        } else {
            body
        };
        response(code, &phrase, out, body)
    }
}

fn response(status: u16, text: &str, mut headers: HeaderList, body: Vec<u8>) -> NetResponse {
    if !headers.contains("content-length") {
        headers.append("Content-Length", &body.len().to_string());
    }
    NetResponse {
        url_list: Vec::new(),
        status,
        status_text: text.to_owned(),
        headers,
        body,
    }
}

/// La fecha HTTP (IMF-fixdate, RFC 9110 §5.6.7) de `secs` desde 1970.
pub fn http_date(secs: f64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let t = secs.floor() as i64;
    let days = t.div_euclid(86_400);
    let rem = t.rem_euclid(86_400);
    // civil_from_days (H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        DAYS[days.rem_euclid(7) as usize],
        d,
        MONTHS[(m - 1) as usize],
        y,
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// Un valor JSON (lo justo para la configuración de `http-cache.js`).
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn parse(s: &str) -> Option<Json> {
        let mut p = JsonParser {
            s: s.as_bytes(),
            i: 0,
        };
        let v = p.value()?;
        p.ws();
        (p.i == p.s.len()).then_some(v)
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(o) => o.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_num(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// El valor como texto de cabecera (`str()` de Python para números y booleanos).
    fn as_text(&self) -> String {
        match self {
            Json::Str(s) => s.clone(),
            Json::Num(n) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{}", *n as i64),
            Json::Num(n) => format!("{n}"),
            Json::Bool(true) => "True".into(),
            Json::Bool(false) => "False".into(),
            Json::Null => "None".into(),
            v => v.to_string(),
        }
    }
}

impl std::fmt::Display for Json {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn string(f: &mut std::fmt::Formatter<'_>, s: &str) -> std::fmt::Result {
            f.write_str("\"")?;
            for c in s.chars() {
                match c {
                    '"' => f.write_str("\\\"")?,
                    '\\' => f.write_str("\\\\")?,
                    '\n' => f.write_str("\\n")?,
                    '\r' => f.write_str("\\r")?,
                    '\t' => f.write_str("\\t")?,
                    c if (c as u32) < 0x20 => write!(f, "\\u{:04x}", c as u32)?,
                    c => write!(f, "{c}")?,
                }
            }
            f.write_str("\"")
        }
        match self {
            Json::Null => f.write_str("null"),
            Json::Bool(b) => write!(f, "{b}"),
            Json::Num(n) if n.is_finite() => write!(f, "{n}"),
            Json::Num(_) => f.write_str("null"),
            Json::Str(s) => string(f, s),
            Json::Arr(a) => {
                f.write_str("[")?;
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{v}")?;
                }
                f.write_str("]")
            }
            Json::Obj(o) => {
                f.write_str("{")?;
                for (i, (k, v)) in o.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    string(f, k)?;
                    write!(f, ": {v}")?;
                }
                f.write_str("}")
            }
        }
    }
}

struct JsonParser<'a> {
    s: &'a [u8],
    i: usize,
}

impl JsonParser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn eat(&mut self, b: u8) -> bool {
        self.ws();
        if self.s.get(self.i) == Some(&b) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn literal(&mut self, word: &str, v: Json) -> Option<Json> {
        self.s[self.i..].starts_with(word.as_bytes()).then(|| {
            self.i += word.len();
            v
        })
    }

    fn value(&mut self) -> Option<Json> {
        self.ws();
        match *self.s.get(self.i)? {
            b'n' => self.literal("null", Json::Null),
            b't' => self.literal("true", Json::Bool(true)),
            b'f' => self.literal("false", Json::Bool(false)),
            b'"' => self.string().map(Json::Str),
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                if self.eat(b']') {
                    return Some(Json::Arr(a));
                }
                loop {
                    a.push(self.value()?);
                    if self.eat(b']') {
                        return Some(Json::Arr(a));
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            b'{' => {
                self.i += 1;
                let mut o = Vec::new();
                if self.eat(b'}') {
                    return Some(Json::Obj(o));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    if !self.eat(b':') {
                        return None;
                    }
                    o.push((k, self.value()?));
                    if self.eat(b'}') {
                        return Some(Json::Obj(o));
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            _ => {
                let start = self.i;
                while self.i < self.s.len()
                    && matches!(
                        self.s[self.i],
                        b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
                    )
                {
                    self.i += 1;
                }
                std::str::from_utf8(&self.s[start..self.i])
                    .ok()?
                    .parse()
                    .ok()
                    .map(Json::Num)
            }
        }
    }

    fn hex4(&mut self) -> Option<u32> {
        let h = std::str::from_utf8(self.s.get(self.i..self.i + 4)?).ok()?;
        self.i += 4;
        u32::from_str_radix(h, 16).ok()
    }

    fn string(&mut self) -> Option<String> {
        if self.s.get(self.i) != Some(&b'"') {
            return None;
        }
        self.i += 1;
        let mut out = String::new();
        loop {
            let start = self.i;
            while self.i < self.s.len() && !matches!(self.s[self.i], b'"' | b'\\') {
                self.i += 1;
            }
            out.push_str(std::str::from_utf8(&self.s[start..self.i]).ok()?);
            match *self.s.get(self.i)? {
                b'"' => {
                    self.i += 1;
                    return Some(out);
                }
                _ => {
                    self.i += 1;
                    let c = *self.s.get(self.i)?;
                    self.i += 1;
                    match c {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let mut cp = self.hex4()?;
                            if (0xd800..0xdc00).contains(&cp)
                                && self.s.get(self.i..self.i + 2) == Some(b"\\u")
                            {
                                self.i += 2;
                                let lo = self.hex4()?;
                                cp = 0x10000 + ((cp - 0xd800) << 10) + (lo.wrapping_sub(0xdc00));
                            }
                            out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                        }
                        _ => return None,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip() {
        let src = r#"[{"name": "a\"bé", "response_headers": [["Expires", 100000], ["X", "y", false]], "x": null, "t": true, "n": -1.5}]"#;
        let v = Json::parse(src).unwrap();
        let Json::Arr(a) = &v else { panic!() };
        assert_eq!(a[0].get("name"), Some(&Json::Str("a\"bé".into())));
        assert_eq!(a[0].get("n"), Some(&Json::Num(-1.5)));
        assert_eq!(Json::parse(&v.to_string()), Some(v));
        assert_eq!(Json::parse("[1,]"), None);
    }

    #[test]
    fn http_dates() {
        assert_eq!(http_date(784_111_777.0), "Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(http_date(0.0), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(
            bf_net::cache::parse_http_date(&http_date(1_700_000_000.0)),
            Some(1_700_000_000)
        );
    }
}
