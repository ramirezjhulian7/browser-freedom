**Navega libre.** Freedom es un navegador web independiente: no está construido sobre Chrome,
Firefox ni Safari, sino sobre un motor propio, hecho desde cero.

> **Vista previa.** Freedom todavía no ejecuta JavaScript. Los sitios para leer (Wikipedia,
> blogs, noticias, documentación) se ven bien; los que dependen de JavaScript (redes
> sociales, correo web, mapas, tiendas) todavía no funcionan o se ven incompletos.

## Descarga

| Sistema | Archivo |
|---|---|
| **Windows** 10 y 11 (64 bits) | `Freedom-…-windows-x64-setup.exe` (instalador) o `…-portable.zip` (sin instalar) |
| **macOS** 11 o posterior (Apple Silicon e Intel) | `Freedom-…-macos-universal.dmg` |
| **Ubuntu, Debian** y derivados | `freedom_…_amd64.deb` |
| **Otras distribuciones Linux** (64 bits) | `Freedom-…-linux-x86_64.tar.gz` |

## Cómo instalarlo

**Windows.** Abre el instalador. No pide permisos de administrador. Como Freedom todavía no
tiene firma digital, Windows puede mostrar «Windows protegió tu PC»: elige **Más
información → Ejecutar de todas formas**.

**macOS.** Abre el `.dmg` y arrastra Freedom a **Aplicaciones**. Como Freedom todavía no
está notarizado por Apple, la primera vez macOS no lo deja abrir: cierra el aviso, ve a
**Ajustes del Sistema → Privacidad y seguridad**, baja hasta el final y pulsa **Abrir
igualmente** junto a Freedom. Desde ahí se abre normal.

**Ubuntu y Debian.** En una terminal, en la carpeta de la descarga:

```sh
sudo apt install ./freedom_*_amd64.deb
```

**Otras distribuciones.** Descomprime el `.tar.gz` y ejecuta `./install.sh`. Se instala para
tu usuario, sin sudo; `sudo ./install.sh --system` lo instala para todos.

## Qué trae

- Pestañas, atrás y adelante, zoom y tema claro u oscuro según tu sistema.
- Búsqueda en Google desde la barra de direcciones.
- Seleccionar, copiar y pegar texto de las páginas, de la barra y de los formularios.
- Privacidad: el historial y las cookies se borran al cerrar Freedom, y no envía datos de uso.

## Verificar la descarga (opcional)

Para comprobar que el archivo no fue alterado, compara su huella con la de `SHA256SUMS`:

- Windows (PowerShell): `Get-FileHash .\Freedom-…-setup.exe`
- macOS: `shasum -a 256 Freedom-…-macos-universal.dmg`
- Linux: `sha256sum -c SHA256SUMS --ignore-missing`

Cada paquete tiene además una atestación de GitHub que prueba que se compiló en este
repositorio: `gh attestation verify <archivo> --repo ramirezjhulian7/browser-freedom`.

¿Algo no funciona? Repórtalo en
[Issues](https://github.com/ramirezjhulian7/browser-freedom/issues) con la dirección de la
página.
