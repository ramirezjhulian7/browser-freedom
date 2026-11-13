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
  hoc. Ver [macOS en detalle](#macos-en-detalle).

## macOS en detalle

```sh
sh packaging/macos/build.sh             # universal (Apple Silicon e Intel) y .dmg, como el workflow
sh packaging/macos/build.sh --native    # solo la arquitectura de este Mac: una compilación
sh packaging/macos/build.sh --native --no-dmg 0.2.0   # sin .dmg y con otra versión
```

Deja en `dist/`:
- `Freedom.app`;
- `Freedom-<v>-macos-universal.dmg`, o `…-macos-arm64.dmg` / `…-macos-x86_64.dmg` con
  `--native`.

El modo universal instala con `rustup` el target que falte (`x86_64-apple-darwin` o
`aarch64-apple-darwin`).

El paquete:

```
Freedom.app/Contents/
  Info.plist               packaging/macos/Info.plist con la versión
  PkgInfo                  APPL????
  MacOS/freedom            el ejecutable de release
  Resources/freedom.icns   el icono (packaging/icons)
  _CodeSignature/          la firma ad hoc
```

El `Info.plist` declara:
- `NSHighResolutionCapable`: la ventana se pinta en píxeles físicos en pantallas Retina.
- Los tipos de documento que Freedom muestra («Abrir con», rol *Viewer*, rango
  *Alternate*): `.html`/`.htm`, `.xhtml`/`.xht` y `.svg`.
- Los esquemas `http` y `https`: Freedom aparece entre los navegadores del sistema.

El script comprueba el resultado sin abrir ninguna ventana:
- `plutil -lint` del `Info.plist`;
- la firma ad hoc (`codesign --force --deep --sign -`) y `codesign --verify --deep --strict`;
- que `Freedom.app/Contents/MacOS/freedom --version` responde.

El ejecutable de dentro admite las mismas órdenes que el suelto. Por ejemplo,
`dist/Freedom.app/Contents/MacOS/freedom render about:home -o home.png`.

La firma ad hoc evita el aviso de «está dañada» en el Mac donde se compiló. Una app bajada
de internet (con el atributo `com.apple.quarantine`) sigue pidiendo «Abrir igualmente» en
Privacidad y seguridad, porque no está notarizada.

**Limitación:** macOS entrega las aperturas desde el Finder («Abrir con», doble clic en un
`.html`) y los enlaces de otras aplicaciones como eventos de Apple (`odoc`, `GURL`), no como
argumentos. `winit` 0.30 no los expone, así que en esos casos Freedom se abre con la página
de inicio o la sesión anterior, no con el documento. `open -a Freedom --args archivo.html`
sí lo pasa como argumento.

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
