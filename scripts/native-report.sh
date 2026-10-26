#!/bin/sh
# SIGIL_NATIVE=1 cargo test の後に、ネイティブとの突き合わせの件数と飛ばした理由ごとの件数を出す。
# 使い方: SIGIL_NATIVE=1 cargo test && scripts/native-report.sh
dir="${CARGO_TARGET_DIR:-target}/tmp"
set -- "$dir"/native-report-*.tsv
if [ ! -e "$1" ]; then
  echo "記録がない: $dir（SIGIL_NATIVE=1 cargo test を先に走らせる）" >&2
  exit 1
fi
echo "== 突き合わせ / 飛ばし（理由別） =="
cat "$@" | awk -F'\t' '$1=="checked"{c++} $1=="skip"{s[$2]++; n++} END{printf "突き合わせ %d 件\n飛ばし %d 件\n", c, n; for(k in s) printf "  %5d  %s\n", s[k], k}'
echo "== ビルドできなかった事例（飛ばしの内訳） =="
cat "$@" | awk -F'\t' '$1=="detail"{print "  " $3 ": " $2}'
