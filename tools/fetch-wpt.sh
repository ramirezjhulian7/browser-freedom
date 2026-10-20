#!/usr/bin/env sh
# Descarga (clon parcial y sparse) la parte de web-platform-tests que usan los harness:
# html/syntax/parsing y los directorios de reftests de CSS.
# El commit pineado está en tools/wpt-commit.txt.
# Uso (desde cualquier directorio): sh tools/fetch-wpt.sh
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WPT="$ROOT/tests/wpt"
DIRS="html/syntax/parsing fonts css/support css/CSS2/box-display css/CSS2/margin-padding-clear css/CSS2/normal-flow css/CSS2/floats css/CSS2/floats-clear css/CSS2/positioning css/CSS2/linebox css/CSS2/text css/CSS2/colors css/CSS2/borders css/CSS2/support css/CSS2/reference css/css-backgrounds css/selectors css/reference url/resources css/css-flexbox css/CSS2/tables css/css-tables css/css-grid css/css-ui"
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
