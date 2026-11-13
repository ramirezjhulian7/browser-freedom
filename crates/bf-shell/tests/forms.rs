//! Envío de formularios `POST` de punta a punta (roadmap 2.4): `freedom session` carga una
//! página con un `<form method=post>` de un servidor local, rellena los campos escribiendo en
//! ellos, envía con Intro (envío implícito, HTML §4.10.21.2) y el servidor comprueba el cuerpo
//! que recibió, en `application/x-www-form-urlencoded` y en `multipart/form-data`.

#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

/// Lo que llegó al servidor en el `POST`.
struct Received {
    content_type: String,
    body: Vec<u8>,
}

/// Una petición: la línea, las cabeceras (nombres en minúsculas) y el cuerpo.
type Request = (String, Vec<(String, String)>, Vec<u8>);

/// Lee una petición; el cuerpo, por `Content-Length`.
fn read_request(stream: &TcpStream) -> Option<Request> {
    let mut r = BufReader::new(stream);
    let mut line = String::new();
    r.read_line(&mut line).ok()?;
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).ok()? == 0 || h == "\r\n" {
            break;
        }
        let (n, v) = h.split_once(':')?;
        headers.push((n.trim().to_ascii_lowercase(), v.trim().to_owned()));
    }
    let len = headers
        .iter()
        .find(|(n, _)| n == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    r.read_exact(&mut body).ok()?;
    Some((line, headers, body))
}

/// Un servidor que sirve `page` en `/` y avisa por el canal de cada `POST /enviar`.
fn serve(page: String) -> (u16, Receiver<Received>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let Some((line, headers, body)) = read_request(&stream) else {
                continue;
            };
            let html = if line.starts_with("POST /enviar ") {
                let content_type = headers
                    .iter()
                    .find(|(n, _)| n == "content-type")
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default();
                let _ = tx.send(Received { content_type, body });
                "<!doctype html><title>Recibido</title><p>Gracias".to_owned()
            } else if line.starts_with("GET / ") {
                page.clone()
            } else {
                String::new()
            };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                 Cache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{html}",
                html.len()
            );
        }
    });
    (port, rx)
}

/// El formulario con `enctype`: texto, área de texto, oculto, casilla marcada, lista, archivo
/// (sin elegir) y el botón de envío por defecto.
fn form_page(enctype: &str) -> String {
    format!(
        "<!doctype html><meta charset=utf-8><title>Formulario</title>\
         <form method=post action=/enviar enctype=\"{enctype}\">\
         <input name=nombre>\
         <textarea name=nota></textarea>\
         <input type=hidden name=oculto value=\"a&amp;b=c\">\
         <input type=checkbox name=acepto checked>\
         <input type=checkbox name=spam>\
         <select name=color><option>rojo<option selected value=verde>Verde</select>\
         <input type=file name=adjunto>\
         <button name=boton value=enviar>Enviar</button>\
         </form>"
    )
}

/// Una carpeta propia del test para las capturas de `session`.
fn out_prefix(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("freedom-test-forms-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Rellena y envía el formulario con `freedom session`; devuelve lo que recibió el servidor y
/// la salida del navegador.
fn submit(enctype: &str) -> (Received, String) {
    let (port, rx) = serve(form_page(enctype));
    let out = Command::new(env!("CARGO_BIN_EXE_freedom"))
        .args([
            "session",
            &format!("http://127.0.0.1:{port}/"),
            "field:nombre",
            "type:José Ñ & \"amigos\"",
            "field:nota",
            "type:uno",
            "key:enter",
            "type:dos",
            "field:nombre",
            "key:enter",
            // Margen si la máquina está cargada: la respuesta llega desde otro hilo.
            "wait",
            "-o",
        ])
        .arg(out_prefix(&enctype.replace('/', "-")))
        .output()
        .expect("se pudo ejecutar freedom");
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "freedom session falló:\n{log}");
    assert!(!log.contains("panicked"), "pánico:\n{log}");
    let received = rx
        .recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|_| panic!("el servidor no recibió el POST:\n{log}"));
    assert!(
        log.contains("título=\"Recibido\""),
        "no llegó la respuesta:\n{log}"
    );
    (received, log)
}

#[test]
fn post_urlencoded() {
    let (got, log) = submit("application/x-www-form-urlencoded");
    assert_eq!(
        got.content_type, "application/x-www-form-urlencoded",
        "{log}"
    );
    assert_eq!(
        String::from_utf8(got.body).unwrap(),
        "nombre=Jos%C3%A9+%C3%91+%26+%22amigos%22&nota=uno%0D%0Ados&oculto=a%26b%3Dc\
         &acepto=on&color=verde&adjunto=&boton=enviar",
        "{log}"
    );
}

#[test]
fn post_multipart() {
    let (got, log) = submit("multipart/form-data");
    let boundary = got
        .content_type
        .strip_prefix("multipart/form-data; boundary=")
        .unwrap_or_else(|| panic!("Content-Type {:?}\n{log}", got.content_type))
        .to_owned();
    let part = |name: &str, value: &str| {
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        )
    };
    let expected = [
        part("nombre", "José Ñ & \"amigos\""),
        part("nota", "uno\r\ndos"),
        part("oculto", "a&b=c"),
        part("acepto", "on"),
        part("color", "verde"),
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"adjunto\"; filename=\"\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n\r\n"
        ),
        part("boton", "enviar"),
        format!("--{boundary}--\r\n"),
    ]
    .concat();
    assert_eq!(String::from_utf8(got.body).unwrap(), expected, "{log}");
}
