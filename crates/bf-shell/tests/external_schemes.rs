//! Navegar a un esquema que el navegador no carga (`mailto:`, `tel:` o el de otra aplicación,
//! como el `meli://` que pide Mercado Libre desde un script) no cambia la página (HTML
//! §7.4.2.2, *navigate*: la URL se entrega a otra aplicación). Antes la pestaña mostraba la
//! página de error "Esquema no soportado" en lugar del sitio.

#![forbid(unsafe_code)]

use std::process::Command;

const PAGE: &str = r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>sigo aquí</title></head><body>
<p><a href="mailto:alguien@example.com">escribir</a></p>
<script>
  setTimeout(function () { location.href = 'meli://home?d2id=1'; }, 0);
</script>
</body></html>"#;

#[test]
fn external_schemes_keep_the_page() {
    let dir = std::env::temp_dir().join(format!("freedom-test-esquemas-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let page = dir.join("doc.html");
    std::fs::write(&page, PAGE).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_freedom"))
        .arg("session")
        .arg(&page)
        .args(["wait", "wait", "link:escribir", "wait", "-o"])
        .arg(dir.join("paso"))
        .output()
        .expect("se pudo ejecutar freedom");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    let states: Vec<&str> = stdout.lines().filter(|l| l.contains(" url=")).collect();
    assert_eq!(states.len(), 5, "{stdout}");
    for line in &states[1..] {
        // Ni el script (`meli:`) ni el enlace (`mailto:`) navegan.
        assert!(line.contains(" url=file://"), "{line}");
        assert!(line.contains("título=\"sigo aquí\""), "{line}");
        assert!(line.contains("historial=1/1"), "{line}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
