# Instaladores de Freedom

Cómo se arman los paquetes de Windows, Linux y macOS, y cómo se publica un release.

## Publicar un release

1. Sube la versión en el `Cargo.toml` del workspace (`[workspace.package] version`).
2. Revisa las notas de [`NOTAS-DEL-RELEASE.md`](NOTAS-DEL-RELEASE.md): son el texto del
   release.
3. Crea y sube la etiqueta de esa versión:

   ```sh
   git tag v0.1.0
   git push origin v0.1.0
   ```

4. El workflow [`instaladores.yml`](../.github/workflows/instaladores.yml) compila en
   Windows, Linux y macOS, arma los paquetes y crea un **release en borrador** con todos.
5. En GitHub → Releases, revisa el borrador (prueba los instaladores) y publícalo.

Para probar los paquetes sin crear un release: Actions → Instaladores → **Run workflow**; los
paquetes quedan como artefactos del run.

## Qué se genera

| Sistema | Paquete | Script | Herramienta |
|---|---|---|---|
| Windows | `Freedom-<v>-windows-x64-setup.exe` y `…-portable.zip` | `windows/build.ps1` | [Inno Setup 6](https://jrsoftware.org/isinfo.php) (`windows/freedom.iss`) |
| Linux | `freedom_<v>_amd64.deb` y `Freedom-<v>-linux-x86_64.tar.gz` | `linux/build.sh` | `dpkg-deb` |
| macOS | `Freedom-<v>-macos-universal.dmg` (`Freedom.app`) | `macos/build.sh` | `lipo`, `codesign`, `hdiutil` |

Cada script corre desde la raíz del repositorio, toma la versión del `Cargo.toml` (o la que
se le pase) y deja el resultado en `dist/`.

- **Windows:** el ejecutable se compila con `--features gui` (`windows_subsystem =
  "windows"`), así que abre sin consola detrás; con esa feature, las órdenes de línea de
  comandos (`session`, `dump-*`) solo imprimen si la salida se redirige. El instalador es
  por usuario (sin administrador) o para todos, crea el acceso del menú Inicio, uno opcional
  en el escritorio, «Abrir con» para `.html`/`.htm`/`.xhtml` y el desinstalador.
- **Linux:** el `.deb` instala en `/usr` con la entrada del menú y los iconos del tema
  `hicolor`. El `.tar.gz` trae `install.sh` (en `~/.local`, o en `/usr/local` con
  `--system`; `--uninstall` lo quita). Se compila en Ubuntu 22.04 (glibc 2.35).
- **macOS:** ejecutable universal (Apple Silicon e Intel), macOS 11 o posterior, firmado ad
  hoc.

## Iconos

[`icons/`](icons) se genera desde el logo (`assets/freedom.png`) con
`python tools/gen_icons.py` (necesita Pillow): `freedom.ico` (Windows), `freedom.icns`
(macOS) y `png/freedom-<n>.png` (Linux). Es la marca sin fondo, como el icono que la ventana
dibuja en la barra de tareas.

## Pendiente

- **Firma de código.** Sin firma, Windows SmartScreen y macOS Gatekeeper piden confirmar la
  primera apertura. Para firmar hacen falta un certificado de firma de código (Windows) y una
  cuenta de Apple Developer con notarización (macOS); los dos se agregarían como secretos del
  repositorio y un paso más en el workflow.
- **Icono dentro de `freedom.exe`.** El instalador y los accesos usan `freedom.ico`, y la
  ventana dibuja el suyo; el ejecutable suelto todavía muestra el icono genérico en el
  Explorador.
- **ARM64** en Windows y Linux, y un AppImage o Flatpak.
