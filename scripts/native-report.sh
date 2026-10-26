#!/bin/sh
# cargo test の後に、ネイティブとの突き合わせの件数と飛ばした理由ごとの件数を出し、
# 飛ばし件数が上限を超えたら（突き合わせ件数が下限を割ったら）失敗させる。
# 使い方: cargo test && scripts/native-report.sh
# 環境変数: SIGIL_NATIVE_MAX_SKIP（飛ばしの上限。既定 10）、SIGIL_NATIVE_MIN_CHECKED（突き合わせの下限。既定 400）
dir="${CARGO_TARGET_DIR:-target}/tmp"
max_skip="${SIGIL_NATIVE_MAX_SKIP:-10}"
min_checked="${SIGIL_NATIVE_MIN_CHECKED:-400}"
set -- "$dir"/native-report-*.tsv
if [ ! -e "$1" ]; then
  echo "記録がない: $dir（cargo test を先に走らせる）" >&2
  exit 1
fi
echo "== 突き合わせ / 飛ばし（理由別） =="
cat "$@" | awk -F'\t' -v max="$max_skip" -v min="$min_checked" '
  $1=="checked"{c++}
  $1=="skip"{s[$2]++; n++}
  END{
    printf "突き合わせ %d 件（下限 %d）\n飛ばし %d 件（上限 %d）\n", c, min, n, max
    for(k in s) printf "  %5d  %s\n", s[k], k
    bad = 0
    if (n > max) { printf "飛ばしが上限を超えた: %d > %d\n", n, max; bad = 1 }
    if (c < min) { printf "突き合わせが下限を割った: %d < %d\n", c, min; bad = 1 }
    exit bad
  }'
