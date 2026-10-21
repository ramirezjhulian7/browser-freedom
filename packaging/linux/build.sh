#!/bin/sh
# Arma los paquetes de Linux de Freedom en dist/, desde la raíz del repositorio:
#
#   sh packaging/linux/build.sh [versión]
#
# - Freedom-<versión>-linux-x86_64.tar.gz: el ejecutable, iconos, el .desktop e install.sh.
# - freedom_<versión>_amd64.deb: para Debian, Ubuntu y derivados (apt install ./…deb).
#
# Compila con `cargo build --release -p bf-shell` si no hay ejecutable. La versión, si no se
# da, sale del Cargo.toml del workspace.
set -eu

cd "$(dirname "$0")/../.."
version="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)}"
arch=x86_64
deb_arch=amd64
exe=target/release/freedom
dist=dist
sizes="16 24 32 48 64 128 256 512"

[ -x "$exe" ] || cargo build --release -p bf-shell
mkdir -p "$dist"

# --- .tar.gz ------------------------------------------------------------------------------
name="Freedom-$version-linux-$arch"
stage="$dist/$name"
rm -rf "$stage"
mkdir -p "$stage/icons"
install -m 755 "$exe" "$stage/freedom"
strip "$stage/freedom" 2>/dev/null || true
install -m 755 packaging/linux/install.sh "$stage/install.sh"
install -m 644 packaging/linux/freedom.desktop "$stage/freedom.desktop"
for n in $sizes; do
    install -m 644 "packaging/icons/png/freedom-$n.png" "$stage/icons/freedom-$n.png"
done
cat >"$stage/LEEME.txt" <<EOF
Freedom $version para Linux (x86_64)

Instalar para tu usuario (sin sudo):   ./install.sh
Instalar para todos:                   sudo ./install.sh --system
Quitarlo:                              ./install.sh --uninstall

También puedes abrirlo sin instalar:   ./freedom
Más información: https://github.com/ramirezjhulian7/browser-freedom
EOF
tar -C "$dist" -czf "$dist/$name.tar.gz" "$name"
rm -rf "$stage"

# --- .deb ---------------------------------------------------------------------------------
root="$dist/deb-root"
rm -rf "$root"
mkdir -p "$root/DEBIAN" "$root/usr/bin" "$root/usr/share/applications" \
    "$root/usr/share/doc/freedom"
install -m 755 "$exe" "$root/usr/bin/freedom"
strip "$root/usr/bin/freedom" 2>/dev/null || true
install -m 644 packaging/linux/freedom.desktop "$root/usr/share/applications/freedom.desktop"
for n in $sizes; do
    mkdir -p "$root/usr/share/icons/hicolor/${n}x${n}/apps"
    install -m 644 "packaging/icons/png/freedom-$n.png" \
        "$root/usr/share/icons/hicolor/${n}x${n}/apps/freedom.png"
done
cat >"$root/usr/share/doc/freedom/copyright" <<EOF
Freedom — https://github.com/ramirezjhulian7/browser-freedom
License: MIT OR Apache-2.0
EOF
size=$(du -sk "$root/usr" | cut -f1)
# winit, softbuffer y arboard cargan X11 o Wayland al arrancar (dlopen): van como Recommends
# porque cualquier escritorio ya los tiene.
cat >"$root/DEBIAN/control" <<EOF
Package: freedom
Version: $version
Section: web
Priority: optional
Architecture: $deb_arch
Installed-Size: $size
Depends: libc6 (>= 2.35)
Recommends: libxkbcommon0, libxkbcommon-x11-0, libx11-6, libxcursor1, libxrandr2, libxi6, libwayland-client0, libwayland-cursor0, fontconfig, fonts-dejavu-core, xdg-desktop-portal
Maintainer: ${DEB_MAINTAINER:-Freedom <https://github.com/ramirezjhulian7/browser-freedom>}
Homepage: https://github.com/ramirezjhulian7/browser-freedom
Description: Navega libre: un navegador web independiente
 Freedom es un navegador web con un motor propio, hecho desde cero: no está
 construido sobre Chrome, Firefox ni Safari. Todavía no ejecuta JavaScript.
EOF
cat >"$root/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t /usr/share/icons/hicolor || true
EOF
cp "$root/DEBIAN/postinst" "$root/DEBIAN/postrm"
chmod 755 "$root/DEBIAN/postinst" "$root/DEBIAN/postrm"
dpkg-deb --root-owner-group --build "$root" "$dist/freedom_${version}_${deb_arch}.deb"
rm -rf "$root"

ls -l "$dist"
