//! Una ráfaga de tareas no deja a la pestaña sin responder (HTML §8.1.7.3: el bucle de eventos
//! elige una tarea en cada vuelta y entre ellas tiene la oportunidad de *update the
//! rendering*). Durante 3 s la página encadena `setTimeout(…, 0)`, mensajes de un
//! `MessageChannel` y `postMessage` a la ventana; mientras dura, la ventana pinta, atiende un
//! clic (que necesita la maqueta de la página para encontrar el enlace) y desplaza la página
//! con la rueda. Antes de `run_due_tasks_for` (ronda 6 de E3), la pestaña corría hasta 10 000
//! tareas seguidas y el clic se atendía a los 3 s, terminada la ráfaga (`clic:despues`).

#![forbid(unsafe_code)]

use std::process::Command;

const PAGE: &str = r##"<!DOCTYPE html><html><head><meta charset="utf-8"><title>t</title></head>
<body style="margin:0"><p><a href="#" onclick="document.title = 'clic:' + (Date.now() < fin ? 'durante' : 'despues'); return false">pulsar</a></p>
<div style="height:5000px"></div>
<script>
  var fin = Date.now() + 3000;
  // Una cadena de `setTimeout(…, 0)`.
  (function tick() { if (Date.now() < fin) setTimeout(tick, 0); })();
  // Una de mensajes de un `MessageChannel` (como el `setImmediate` de las bibliotecas).
  var ch = new MessageChannel();
  ch.port1.onmessage = function () { if (Date.now() < fin) ch.port2.postMessage(0); };
  ch.port2.postMessage(0);
  // Y una de `postMessage` a la ventana, con algo de trabajo en cada tarea: 10 000 seguidas
  // pasan de los 3 s.
  window.onmessage = function () {
    for (var i = 0, x = 0; i < 3000; i++) x += i;
    if (Date.now() < fin) postMessage(0, '*'); else document.title += '|fin';
  };
  postMessage(0, '*');
</script>
</body></html>"##;

#[test]
fn bursts_of_tasks_keep_the_page_responsive() {
    let dir = std::env::temp_dir().join(format!("freedom-test-rafagas-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("doc.html"), PAGE).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_freedom"))
        .arg("session")
        .arg(dir.join("doc.html"))
        .args(["link:pulsar", "wheel:600,400,300"])
        .args(std::iter::repeat_n("wait", 14))
        .arg("-o")
        .arg(dir.join("paso"))
        .output()
        .expect("se pudo ejecutar freedom");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    let states: Vec<&str> = stdout.lines().filter(|l| l.contains(" url=")).collect();
    let field = |line: &str, name: &str| {
        line.split_once(name)
            .map(|(_, rest)| rest.split(' ').next().unwrap_or("").to_owned())
            .unwrap_or_default()
    };
    // El clic (paso 1) se atendió durante la ráfaga…
    assert!(states[1].contains("título=\"clic:durante\""), "{stdout}");
    // …la rueda (paso 2) desplazó la página…
    assert_eq!(field(states[2], "scroll="), "0,300", "{stdout}");
    // …y la ráfaga terminó después.
    assert!(
        states
            .last()
            .unwrap()
            .contains("título=\"clic:durante|fin\""),
        "{stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
