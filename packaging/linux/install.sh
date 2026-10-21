#!/bin/sh
# Instala Freedom desde el .tar.gz (este archivo va dentro del paquete).
#
#   ./install.sh               para tu usuario, en ~/.local (sin sudo)
#   sudo ./install.sh --system para todos, en /usr/local
#   ./install.sh --uninstall   lo quita (con --system, el de /usr/local)
set -eu

here=$(cd "$(dirname "$0")" && pwd)
prefix="${HOME}/.local"
action=install
for arg in "$@"; do
    case "$arg" in
        --system) prefix=/usr/local ;;
        --uninstall) action=uninstall ;;
        -h | --help)
            sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "opción desconocida: $arg" >&2
            exit 2
            ;;
    esac
done

bin="$prefix/bin"
apps="$prefix/share/applications"
icons="$prefix/share/icons/hicolor"
sizes="16 24 32 48 64 128 256 512"

refresh() {
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$apps" || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$icons" || true
}

if [ "$action" = uninstall ]; then
    rm -f "$bin/freedom" "$apps/freedom.desktop"
    for n in $sizes; do rm -f "$icons/${n}x${n}/apps/freedom.png"; done
    refresh
    echo "Freedom se quitó de $prefix."
    exit 0
fi

mkdir -p "$bin" "$apps"
install -m 755 "$here/freedom" "$bin/freedom"
for n in $sizes; do
    mkdir -p "$icons/${n}x${n}/apps"
    install -m 644 "$here/icons/freedom-$n.png" "$icons/${n}x${n}/apps/freedom.png"
done
# Con la ruta completa, el menú de aplicaciones lo encuentra aunque ~/.local/bin no esté en
# el PATH.
sed "s|^Exec=freedom |Exec=$bin/freedom |" "$here/freedom.desktop" >"$apps/freedom.desktop"
chmod 644 "$apps/freedom.desktop"
refresh

echo "Freedom quedó instalado en $prefix."
echo "Ábrelo desde el menú de aplicaciones o con: $bin/freedom"
case ":$PATH:" in
    *":$bin:"*) ;;
    *) echo "(Para escribir solo «freedom» en la terminal, agrega $bin a tu PATH.)" ;;
esac
