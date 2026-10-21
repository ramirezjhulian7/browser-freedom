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
   Windows, Linux y macOS, arma los paquetes y crea un **release en borrador** con todos,
   más `SHA256SUMS` y una atestación de procedencia de GitHub por paquete (gratis: no quita
   los avisos de Windows ni de macOS, pero permite verificar que el archivo salió de este
   repositorio con `gh attestation verify`).
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

- **Firma de código.** Sin firma, Windows SmartScreen pide confirmar la apertura (y en cada
  versión nueva otra vez), Smart App Control de Windows 11 la bloquea sin opción de abrirla
  igual, y macOS la bloquea hasta «Abrir igualmente» en Privacidad y seguridad. Opciones:
  - Windows, gratis: [SignPath Foundation](https://signpath.org) para proyectos de código
    abierto. Pide un release ya publicado, licencia OSI en el repositorio (archivos
    `LICENSE`), una sección «Política de firma de código» con los roles del equipo y la
    declaración de privacidad, nombre y versión del producto dentro de `freedom.exe`, 2FA y
    aprobar cada firma; el firmante visible es «SignPath Foundation».
  - Windows, de pago: Certum Open Source (unos USD 25–30 al año) o Azure Artifact Signing
    (unos USD 10 al mes; para personas, solo EE. UU. y Canadá).
  - macOS: notarización con Apple Developer Program (USD 99 al año); no hay opción gratis.
  - La Microsoft Store no es opción: su política 10.2.1 exige motor Chromium o Gecko.
- **Icono dentro de `freedom.exe`.** El instalador y los accesos usan `freedom.ico`, y la
  ventana dibuja el suyo; el ejecutable suelto todavía muestra el icono genérico en el
  Explorador.
- **ARM64** en Windows y Linux, y un AppImage o Flatpak.
