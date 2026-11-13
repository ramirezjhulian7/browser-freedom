//! Los scripts de un documento XHTML en la ventana (`src/xml_scripts.rs`): corren durante el
//! parseo, en orden de documento, viendo solo lo parseado hasta su `</script>`, con
//! `currentScript`, `readyState` y `load`, igual que en HTML (HTML §13.4).

#![forbid(unsafe_code)]

use std::process::Command;

const PAGE: &str = r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>sin scripts</title></head><body>
<p id="a">A</p>
<script>
  document.title = 'uno:' + (document.getElementById('b') ? 'b' : 'sin-b') + ':' +
    document.readyState + ':' + (document.currentScript ? 'cs' : 'sin-cs');
</script>
<p id="b">B</p>
<script src="externo.js"></script>
<script>
  addEventListener('load', function () { document.title += '|load:' + document.readyState; });
  document.getElementById('a').textContent = 'cambiado';
</script>
</body></html>"#;

#[test]
fn xhtml_scripts_run_while_parsing() {
    let dir = std::env::temp_dir().join(format!("freedom-test-xhtml-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("doc.xhtml"), PAGE).unwrap();
    std::fs::write(
        dir.join("externo.js"),
        "document.title += '|dos:' + (document.getElementById('b') ? 'b' : 'sin-b');",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_freedom"))
        .arg("session")
        .arg(dir.join("doc.xhtml"))
        .args(["wait", "-o"])
        .arg(dir.join("paso"))
        .output()
        .expect("se pudo ejecutar freedom");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    let last = stdout.lines().rfind(|l| l.contains(" url=")).unwrap_or("");
    assert!(
        last.contains("título=\"uno:sin-b:loading:cs|dos:b|load:complete\""),
        "{stdout}"
    );
}
