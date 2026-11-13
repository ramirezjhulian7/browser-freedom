//! La rueda sobre los controles con scroll interno (roadmap 4.3), de punta a punta con
//! `freedom session` (la lógica de la ventana sin abrirla): sobre un `<textarea>` con más texto
//! del que cabe y sobre un `<select size=4>` con más opciones, la rueda cambia su `scrollTop`
//! y no el de la página; fuera de ellos, desplaza la página.

#![forbid(unsafe_code)]

use std::process::Command;

/// Un `<textarea>` de 60 líneas en 80 px y una lista de 30 opciones con 4 visibles; cada
/// `scroll` escribe en el título el `scrollTop` de los dos.
fn page() -> String {
    let lines: Vec<String> = (0..60).map(|i| format!("línea {i}")).collect();
    let options: String = (0..30)
        .map(|i| format!("<option>opción {i}</option>"))
        .collect();
    format!(
        "<!doctype html><meta charset=utf-8><title>inicio</title>\
         <style>body {{ margin: 0; height: 3000px }} \
         textarea, select {{ position: absolute; top: 200px; width: 200px; font: 16px monospace }}</style>\
         <textarea id=t style='left: 20px; height: 80px'>{}</textarea>\
         <select id=s size=4 style='left: 300px'>{options}</select>\
         <script>\
           function show() {{\
             document.title = 't=' + document.getElementById('t').scrollTop +\
               ' s=' + document.getElementById('s').scrollTop;\
           }}\
           document.getElementById('t').addEventListener('scroll', show);\
           document.getElementById('s').addEventListener('scroll', show);\
         </script>",
        lines.join("\n")
    )
}

/// (título, scroll vertical de la página) de cada línea de estado de la sesión.
fn states(stdout: &str) -> Vec<(String, f32)> {
    stdout
        .lines()
        .filter(|l| l.contains(" url="))
        .map(|l| {
            let title = l
                .split_once("título=\"")
                .and_then(|(_, r)| r.split_once('"'))
                .map(|(t, _)| t.to_owned())
                .unwrap_or_default();
            let scroll = l
                .split_once(" scroll=")
                .and_then(|(_, r)| r.split(' ').next())
                .and_then(|s| s.split_once(','))
                .and_then(|(_, y)| y.parse().ok())
                .unwrap_or(f32::NAN);
            (title, scroll)
        })
        .collect()
}

fn top(title: &str, key: &str) -> f32 {
    title
        .split_whitespace()
        .find_map(|p| p.strip_prefix(key))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0)
}

#[test]
fn wheel_scrolls_textarea_and_list_box_not_the_page() {
    let dir = std::env::temp_dir().join(format!("freedom-test-rueda-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("rueda.html");
    std::fs::write(&file, page()).unwrap();
    // La página empieza bajo la barra de 72 px: el `<textarea>` ocupa y = 272…352 de la
    // ventana y x = 20…220; la lista, x = 300…500.
    let out = Command::new(env!("CARGO_BIN_EXE_freedom"))
        .arg("session")
        .arg(&file)
        .args([
            "wheel:100,310,120",
            "wheel:400,300,120",
            "wheel:700,600,100",
            "-o",
        ])
        .arg(dir.join("paso"))
        .output()
        .expect("se pudo ejecutar freedom");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let log = format!("{stdout}{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{log}");
    let s = states(&stdout);
    assert_eq!(s.len(), 4, "{log}");
    // Sobre el `<textarea>`: se desplaza él, no la página ni la lista.
    assert!(top(&s[1].0, "t=") > 0.0, "{log}");
    assert_eq!(top(&s[1].0, "s="), 0.0, "{log}");
    assert_eq!(s[1].1, 0.0, "la página no se mueve: {log}");
    // Sobre la lista: se desplaza ella; el `<textarea>` se queda donde estaba.
    assert!(top(&s[2].0, "s=") > 0.0, "{log}");
    assert_eq!(top(&s[2].0, "t="), top(&s[1].0, "t="), "{log}");
    assert_eq!(s[2].1, 0.0, "la página no se mueve: {log}");
    // Fuera de los controles, la rueda desplaza la página.
    assert!(s[3].1 > 0.0, "{log}");
}
