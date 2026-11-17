//! El paso `dump` de `freedom session` (`App::session_dump` en `src/window/mod.rs`): junto a la
//! captura del paso deja el DOM serializado, el árbol de maqueta y la consola de la pestaña.

#![forbid(unsafe_code)]

use std::process::Command;

const PAGE: &str = r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>volcado</title></head><body>
<p id="a">hola</p>
<script>
  var p = document.createElement('p');
  p.id = 'js';
  p.textContent = 'desde el script';
  document.body.appendChild(p);
  console.log('mensaje de la página');
  console.warn('un aviso');
  noExiste();
</script>
</body></html>"#;

#[test]
fn dump_writes_dom_layout_and_console() {
    let dir = std::env::temp_dir().join(format!("freedom-test-dump-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("doc.html"), PAGE).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_freedom"))
        .arg("session")
        .arg(dir.join("doc.html"))
        .args(["wait", "dump", "-o"])
        .arg(dir.join("paso"))
        .output()
        .expect("se pudo ejecutar freedom");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let read = |name: &str| {
        std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
    };
    // El paso 2 (`dump`) deja su captura y los tres archivos.
    assert!(dir.join("paso-02.png").exists());

    // El DOM después de los scripts, serializado como HTML.
    let dom = read("paso-02.dom.html");
    assert!(dom.starts_with("<!-- file://"), "{dom}");
    assert!(dom.contains("<!DOCTYPE html>"), "{dom}");
    assert!(dom.contains(r#"<p id="a">hola</p>"#), "{dom}");
    assert!(dom.contains(r#"<p id="js">desde el script</p>"#), "{dom}");

    // El árbol de fragmentos de la maqueta, con los dos párrafos.
    let layout = read("paso-02.layout.txt");
    assert!(layout.contains("Block <p#a>"), "{layout}");
    assert!(layout.contains("Text \"hola\""), "{layout}");
    assert!(layout.contains("Text \"desde el script\""), "{layout}");

    // La consola, con su nivel: el `log`, el `warn` y la excepción sin atrapar.
    let console = read("paso-02.console.txt");
    assert!(console.contains("[log] mensaje de la página"), "{console}");
    assert!(console.contains("[warn] un aviso"), "{console}");
    assert!(
        console
            .lines()
            .any(|l| l.starts_with("[error]") && l.contains("noExiste")),
        "{console}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
