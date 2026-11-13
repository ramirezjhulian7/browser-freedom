#!/bin/sh
# Arma Freedom.app (y el .dmg) en dist/, en macOS y desde cualquier carpeta del repositorio:
#
#   sh packaging/macos/build.sh [--native] [--no-dmg] [versión]
#
# Sin opciones, el ejecutable es universal (Apple Silicon e Intel: dos compilaciones y
# `lipo`), como lo arma el workflow de instaladores. Con --native se compila solo para la
# arquitectura de este Mac (una compilación; para probar la app en local). Con --no-dmg no se
# arma el .dmg.
#
# La app va firmada ad hoc (`codesign -s -`), no con una cuenta de Apple Developer ni
# notarizada: arranca sin el aviso de «dañada», pero la primera vez que se baja de internet
# macOS la bloquea y se permite en Ajustes del Sistema → Privacidad y seguridad → «Abrir
# igualmente». La versión, si no se da, sale del Cargo.toml del workspace.
#
# Al terminar comprueba el paquete: `plutil -lint` del Info.plist, `codesign --verify` y que
# el ejecutable de dentro responde a `--version` (sin abrir ninguna ventana).
set -eu

cd "$(dirname "$0")/../.."

universal=1
dmg=1
version=""
for arg in "$@"; do
    case "$arg" in
        --native) universal=0 ;;
        --no-dmg) dmg=0 ;;
        -*)
            echo "opción desconocida: $arg" >&2
            exit 2
            ;;
        *) version="$arg" ;;
    esac
done
if [ -z "$version" ]; then
    version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
fi

dist=dist
app="$dist/Freedom.app"
export MACOSX_DEPLOYMENT_TARGET=11.0

if [ "$universal" = 1 ]; then
    rustup target add aarch64-apple-darwin x86_64-apple-darwin
    for target in aarch64-apple-darwin x86_64-apple-darwin; do
        cargo build --release -p bf-shell --target "$target"
    done
    arch=universal
else
    cargo build --release -p bf-shell
    arch="$(uname -m)"
fi

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
if [ "$universal" = 1 ]; then
    lipo -create -output "$app/Contents/MacOS/freedom" \
        target/aarch64-apple-darwin/release/freedom \
        target/x86_64-apple-darwin/release/freedom
else
    cp target/release/freedom "$app/Contents/MacOS/freedom"
fi
sed "s/@VERSION@/$version/g" packaging/macos/Info.plist >"$app/Contents/Info.plist"
printf 'APPL????' >"$app/Contents/PkgInfo"
cp packaging/icons/freedom.icns "$app/Contents/Resources/freedom.icns"

# Comprobaciones del paquete: el Info.plist, la firma y que el ejecutable arranca.
plutil -lint "$app/Contents/Info.plist"
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict --verbose=2 "$app"
"$app/Contents/MacOS/freedom" --version

if [ "$dmg" = 1 ]; then
    # El .dmg: la app y un acceso a /Applications para arrastrarla.
    stage="$dist/dmg"
    rm -rf "$stage"
    mkdir -p "$stage"
    cp -R "$app" "$stage/"
    ln -s /Applications "$stage/Applications"
    image="$dist/Freedom-$version-macos-$arch.dmg"
    rm -f "$image"
    hdiutil create -volname "Freedom" -srcfolder "$stage" -ov -format UDZO "$image"
    rm -rf "$stage"
fi

ls -l "$dist"
