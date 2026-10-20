#!/usr/bin/env sh
# Descarga (clon parcial y sparse) test262 en tests/test262: harness, test/language,
# test/built-ins, test/annexB y test/intl402. El commit pineado está en tools/test262-commit.txt.
# Uso (desde cualquier directorio): sh tools/fetch-test262.sh
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/tests/test262"
DIRS="harness test/language test/built-ins test/annexB test/intl402"
COMMIT="$(tr -d ' \r\n' < "$ROOT/tools/test262-commit.txt")"

if [ -d "$DEST/.git" ]; then
    git -C "$DEST" fetch --filter=blob:none origin "$COMMIT"
    # shellcheck disable=SC2086
    git -C "$DEST" sparse-checkout set $DIRS
else
    git -c core.autocrlf=false clone --filter=blob:none --no-checkout https://github.com/tc39/test262 "$DEST"
    git -C "$DEST" config core.autocrlf false
    git -C "$DEST" sparse-checkout init --cone
    # shellcheck disable=SC2086
    git -C "$DEST" sparse-checkout set $DIRS
fi
git -C "$DEST" checkout --detach "$COMMIT"
echo "tests/test262 en $COMMIT"
