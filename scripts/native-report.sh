#!/bin/sh
# cargo test の後に、ネイティブとの突き合わせの件数と飛ばした理由ごとの件数を出し、
# 飛ばし件数（仕様上ビルドで止める参照・Lock の入口引数を除く）が上限を超えたら（突き合わせ件数が下限を割ったら）失敗させる。
# 使い方: cargo test && scripts/native-report.sh
# 環境変数: SIGIL_NATIVE_MAX_SKIP（飛ばしの上限。既定 10）、SIGIL_NATIVE_MIN_CHECKED（突き合わせの下限。既定 400）
# CI は .github/workflows/ci.yml で上限 20・下限 400 を渡す（上限 20 は 2026-11-10 の所有者の決定、Issue #379 で削減を追跡）。
# 既定の 10 は引き上げ前の値（Issue #379 は上限を再び下げることを目指す）。今の実測（飛ばし 20）では、既定のままだと失敗する。
# 実測の記録は ADR 0030 補足 D-5。
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
  $1=="skip"{s[$2]++; if ($2 ~ /^仕様上ビルドで止める/) d++; else n++}
  END{
    printf "突き合わせ %d 件（下限 %d）\n飛ばし %d 件（上限 %d。仕様上ビルドで止める入口引数 %d 件は上限の対象外）\n", c, min, n, max, d
    for(k in s) printf "  %5d  %s\n", s[k], k
    bad = 0
    if (n > max) { printf "飛ばしが上限を超えた: %d > %d\n", n, max; bad = 1 }
    if (c < min) { printf "突き合わせが下限を割った: %d < %d\n", c, min; bad = 1 }
    exit bad
  }'
