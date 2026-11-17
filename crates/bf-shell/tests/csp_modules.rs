//! CSP y módulos: un módulo que importa un script con `nonce` hereda su nonce y sus metadatos
//! de parser (HTML §8.1.5.5, *descendant script fetch options*), y la CSP lo permite por ese
//! nonce (CSP 3 §6.7.1.1). Antes, el `import()` dinámico y el `import` estático se pedían sin
//! nonce y `script-src 'nonce-…' 'strict-dynamic'` los bloqueaba (Reddit, y el
//! `import("./bases.min.js")` del monitor de Mercado Libre).

#![forbid(unsafe_code)]

use std::process::Command;

const PAGE: &str = r#"<!DOCTYPE html><html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="script-src 'nonce-abc' 'strict-dynamic'">
<title>t</title>
<script type="module" nonce="abc" src="data:text/javascript,document.title+='|parser'"></script>
<script nonce="abc">
  var s = document.createElement('script'); s.type = 'module'; s.nonce = 'abc';
  s.src = "data:text/javascript,document.title+='|insertado'"; document.head.appendChild(s);
  import("data:text/javascript,export default 1").then(
    function () { document.title += '|dinamico'; },
    function () { document.title += '|dinamico-bloqueado'; });
</script>
<script type="module" nonce="abc">import "data:text/javascript,document.title+='|estatico'";</script>
</head><body></body></html>"#;

#[test]
fn imported_modules_inherit_the_nonce() {
    let dir = std::env::temp_dir().join(format!("freedom-test-csp-mod-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("doc.html"), PAGE).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_freedom"))
        .arg("session")
        .arg(dir.join("doc.html"))
        .args(["wait", "wait", "-o"])
        .arg(dir.join("paso"))
        .output()
        .expect("se pudo ejecutar freedom");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}{stderr}");
    let last = stdout.lines().rfind(|l| l.contains(" url=")).unwrap_or("");
    for part in ["|parser", "|insertado", "|dinamico", "|estatico"] {
        assert!(last.contains(part), "falta {part}: {last}\n{stderr}");
    }
    assert!(!last.contains("bloqueado"), "{last}");
    assert!(!stderr.contains("se bloqueó"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}
