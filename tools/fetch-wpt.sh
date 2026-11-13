#!/usr/bin/env sh
# Descarga (clon parcial y sparse) la parte de web-platform-tests que usan los harness y las
# mediciones: css, html y svg enteros (reftests y parsing), y los directorios de testharness.js
# (resources, common, dom, url, encoding, fetch, xhr, streams, IndexedDB, workers,
# service-workers, websockets, domparsing, quirks, images, webstorage, y los de
# accesibilidad: html-aam, wai-aria, accname, core-aam; animaciones, CSP y mixed content…).
# El commit pineado está en tools/wpt-commit.txt.
# Uso (desde cualquier directorio): sh tools/fetch-wpt.sh
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WPT="$ROOT/tests/wpt"
DIRS="fonts resources common dom url encoding console interfaces shadow-dom custom-elements css html svg fetch xhr streams IndexedDB workers service-workers websockets domparsing quirks images webstorage html-aam wai-aria accname core-aam web-animations content-security-policy mixed-content"
COMMIT="$(tr -d ' \r\n' < "$ROOT/tools/wpt-commit.txt")"

if [ -d "$WPT/.git" ]; then
    git -C "$WPT" fetch --filter=blob:none origin "$COMMIT"
    # shellcheck disable=SC2086
    git -C "$WPT" sparse-checkout set $DIRS
else
    git -c core.autocrlf=false clone --filter=blob:none --no-checkout https://github.com/web-platform-tests/wpt "$WPT"
    git -C "$WPT" config core.autocrlf false
    git -C "$WPT" sparse-checkout init --cone
    # shellcheck disable=SC2086
    git -C "$WPT" sparse-checkout set $DIRS
fi
git -C "$WPT" checkout --detach "$COMMIT"
echo "tests/wpt en $COMMIT"
