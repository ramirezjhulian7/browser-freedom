#!/usr/bin/env bash
# Comparación A/B en una máquina con carga: alterna el bf-bench de la línea base (BASE) y el
# actual (NEW) ROUNDS veces y compara el mejor tiempo de cada uno.
#   BASE=target/base-target/release/bf-bench NEW=target/release/bf-bench ROUNDS=5 bench/ab.sh [--filter x]
set -e
BASE=${BASE:-target/base-target/release/bf-bench}
NEW=${NEW:-target/release/bf-bench}
ROUNDS=${ROUNDS:-5}
OUT=${OUT:-target/ab}
# Los dos binarios corren los mismos programas (los de este directorio).
DIR=${DIR:-$(dirname "$0")}
mkdir -p "$OUT"
rm -f "$OUT/base.tsv" "$OUT/new.tsv"
for i in $(seq "$ROUNDS"); do
  "$BASE" --runs 1 --dir "$DIR" --save "$OUT/base.tsv" --merge-min "$@" > /dev/null
  "$NEW" --runs 1 --dir "$DIR" --save "$OUT/new.tsv" --merge-min "$@" > /dev/null
done
"$NEW" --runs 0 --dir "$DIR" --compare "$OUT/base.tsv" "$@" > /dev/null || true
# La tabla: el mínimo de cada uno.
join -t $'\t' <(sort "$OUT/base.tsv") <(sort "$OUT/new.tsv") | awk -F'\t' '
  BEGIN { print "| benchmark | base ms | nuevo ms | speedup |"; print "|---|---:|---:|---:|" }
  { s = $2 / $3; g += log(s); n++; printf "| %s | %.1f | %.1f | %.2f× |\n", $1, $2, $3, s }
  END { if (n) printf "\nmedia geométrica del speedup (%d): %.2f×\n", n, exp(g / n) }'
