#!/bin/sh
# Arma Freedom.app y el .dmg en dist/, en macOS y desde la raíz del repositorio:
#
#   sh packaging/macos/build.sh [versión]
#
# El ejecutable es universal (Apple Silicon e Intel). La app va firmada ad hoc, no con una
# cuenta de Apple Developer ni notarizada: la primera vez, macOS la bloquea y se permite en
# Ajustes del Sistema → Privacidad y seguridad → «Abrir igualmente» (desde macOS 15 el clic
# derecho → Abrir ya no basta). La versión, si no se da, sale del Cargo.toml del workspace.
set -eu

cd "$(dirname "$0")/../.."
version="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)}"
dist=dist
app="$dist/Freedom.app"

rustup target add aarch64-apple-darwin x86_64-apple-darwin
export MACOSX_DEPLOYMENT_TARGET=11.0
for target in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --release -p bf-shell --target "$target"
done

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create -output "$app/Contents/MacOS/freedom" \
    target/aarch64-apple-darwin/release/freedom \
    target/x86_64-apple-darwin/release/freedom
sed "s/@VERSION@/$version/g" packaging/macos/Info.plist >"$app/Contents/Info.plist"
cp packaging/icons/freedom.icns "$app/Contents/Resources/freedom.icns"
codesign --force --deep --sign - "$app"

# El .dmg: la app y un acceso a /Applications para arrastrarla.
stage="$dist/dmg"
rm -rf "$stage"
mkdir -p "$stage"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
dmg="$dist/Freedom-$version-macos-universal.dmg"
rm -f "$dmg"
hdiutil create -volname "Freedom" -srcfolder "$stage" -ov -format UDZO "$dmg"
rm -rf "$stage"

ls -l "$dist"
