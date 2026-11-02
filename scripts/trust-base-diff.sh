#!/usr/bin/env bash
# PR の基準（base）との差分から、信頼基盤に関わる変更を検出する（ADR 0018 判断 1 の T3・判断 2 の S2）。
#   (1) 原始操作の表（src/stdlib.rs の RAW_OPS）
#   (2) 許可の一覧（trust-base.list。予定のパス。無ければ対象から外す）
#   (3) `unchecked` の関数の追加・削除・本体の変更・前提の文の変更
# 変わった前提の文は、標準出力と $GITHUB_STEP_SUMMARY（あれば）に出す。
#
# 使い方: scripts/trust-base-diff.sh <base の版> [<head の版（既定 HEAD）>]
# 環境変数:
#   SIGIL_TRUST_BASE_MODE  mark（既定。印のみ。失敗にしない）か fail（検出したら失敗にする）。
#                          切替えはこの 1 つだけ（CI では ci.yml の該当ステップの env）。
# 走査の対象: tests/ と target/ を除く .rs・.sigil（標準ライブラリの Sigil ソースは Rust の文字列にある）。
set -uo pipefail

base="${1:?使い方: $0 <base の版> [<head の版>]}"
head="${2:-HEAD}"
mode="${SIGIL_TRUST_BASE_MODE:-mark}"
case "$mode" in
  mark | fail) ;;
  *)
    echo "SIGIL_TRUST_BASE_MODE は mark か fail: $mode" >&2
    exit 2
    ;;
esac

# 検出の本体。失敗は return 1 で返す（mark では警告にとどめるため、set -e には頼らない）。
detect() {
  # base の版が手元に無ければ（浅い checkout）、その 1 コミットだけ取り寄せる。
  if ! git cat-file -e "${base}^{commit}" 2>/dev/null; then
    git fetch --no-tags --depth=1 origin "$base" || return 1
  fi
  # 比べる起点は merge-base（履歴が浅くて求まらないときは base そのもの）。
  local mb
  mb="$(git merge-base "$base" "$head" 2>/dev/null)" || mb="$base"
  python3 -I - "$mb" "$head" <<'PY' || return 1
import re
import subprocess
import sys

base, head = sys.argv[1], sys.argv[2]
RAW_OPS_FILE = "src/stdlib.rs"
ALLOW_LIST_FILE = "trust-base.list"


def strip_comment(line):
    """文字列の外の `//` 以降を落とす。"""
    in_str = False
    i = 0
    while i < len(line):
        c = line[i]
        if in_str:
            if c == "\\":
                i += 1
            elif c == '"':
                in_str = False
        elif c == '"':
            in_str = True
        elif line.startswith("//", i):
            return line[:i]
        i += 1
    return line


def git(*args):
    r = subprocess.run(["git", *args], capture_output=True, text=True, encoding="utf-8")
    if r.returncode != 0:
        raise SystemExit(f"git {' '.join(args)}: {r.stderr.strip()}")
    return r.stdout


def show(rev, path):
    r = subprocess.run(
        ["git", "show", f"{rev}:{path}"], capture_output=True, text=True, encoding="utf-8"
    )
    return r.stdout if r.returncode == 0 else None  # 無ければ None（予定のパスは静かに外す）


def scannable(p):
    if p.startswith(("tests/", "target/")) or "/target/" in p:
        return False
    return p.endswith((".rs", ".sigil"))


def norm(s):
    return " ".join(s.split())


def find_brace(line):
    """文字列と // の外の最初の `{` の位置。無ければ -1。"""
    in_str = False
    i = 0
    while i < len(line):
        c = line[i]
        if in_str:
            if c == "\\":
                i += 1
            elif c == '"':
                in_str = False
        elif c == '"':
            in_str = True
        elif line.startswith("//", i):
            return -1
        elif c == "{":
            return i
        i += 1
    return -1


def match_body(lines, j, col):
    """lines[j][col] の `{` から、対応する `}` までの本文を返す。"""
    depth = 0
    in_str = False
    buf = []
    for k in range(j, len(lines)):
        line = lines[k]
        start = col if k == j else 0
        i = start
        while i < len(line):
            c = line[i]
            if in_str:
                if c == "\\":
                    i += 1
                elif c == '"':
                    in_str = False
            elif c == '"':
                in_str = True
            elif line.startswith("//", i):
                break
            elif c == "{":
                depth += 1
            elif c == "}":
                depth -= 1
                if depth == 0:
                    buf.append(line[start : i + 1])
                    return "\n".join(buf)
            i += 1
        buf.append(line[start:])
    return "\n".join(buf)


FN_RE = re.compile(r"^\s*(?:pub\s+)?fn\s+(\w+)")
UNCHECKED_RE = re.compile(r'^\s*unchecked\s+"((?:[^"\\]|\\.)*)"')
MODULE_RE = re.compile(r"^(?:.*r#\")?\s*module\s+([\w.]+)\s*$")
IMPL_RE = re.compile(r"^(\s*)impl\b(.*?)\{\s*$")


def unchecked_fns(text, path):
    """`unchecked` の節を持つ関数を {名前: (前提の文, 宣言と本体)} で返す。"""
    lines = text.split("\n")
    module, impls, found = "", [], {}
    for i, line in enumerate(lines):
        m = MODULE_RE.match(line)
        if m:
            module = m.group(1)
            impls = []
            continue
        m = IMPL_RE.match(line)
        if m:
            impls.append((len(m.group(1)), norm(m.group(2))))
            continue
        m = re.match(r"^(\s*)\}\s*$", line)
        if m and impls and len(m.group(1)) == impls[-1][0]:
            impls.pop()
            continue
        m = FN_RE.match(line)
        if not m:
            continue
        header, premise, j, col = [], None, i, -1
        while j < min(i + 60, len(lines)):
            if j > i and FN_RE.match(lines[j]):
                break
            col = find_brace(lines[j])
            header.append(lines[j] if col < 0 else lines[j][:col])
            um = UNCHECKED_RE.match(lines[j])
            if um:
                premise = um.group(1)
                header.pop()  # 前提の文は別に比べる
            if col >= 0:
                break
            j += 1
        if premise is None or col < 0:
            continue
        body = match_body(lines, j, col)
        name = f"{path} :: {module} :: " + (impls[-1][1] + " :: " if impls else "") + m.group(1)
        n, key = 2, name
        while key in found:
            key, n = f"{name}#{n}", n + 1
        code = "\n".join(strip_comment(x) for x in (" ".join(header) + "\n" + body).split("\n"))
        found[key] = (premise, norm(code))
    return found


ROW_RE = re.compile(r"RawRow\s*\{(.*?)\n\s*\},", re.S)
FIELD_RE = re.compile(r'(\w+):\s*(?:Layer::(\w+)|"((?:[^"\\]|\\.)*)"|([A-Z][A-Z0-9_]*)\b(?!::))')
CONST_RE = re.compile(r'const\s+([A-Z][A-Z0-9_]*)\s*:\s*&str\s*=\s*"((?:[^"\\]|\\.)*)"\s*;')


def raw_ops(text):
    """RAW_OPS の行を {操作名: {項目: 値}} で返す。"""
    if text is None:
        return {}
    consts = dict(CONST_RE.findall(text))
    s = text.find("pub const RAW_OPS")
    if s < 0:
        return {}
    e = text.find("\n];", s)
    block = text[s : e if e >= 0 else len(text)]
    rows = {}
    for rm in ROW_RE.finditer(block):
        # 識別子の値（例 `effects: DELEGATED`）は、同じファイルの `const` の文字列に解決する。
        f = {k: (a or b or consts.get(c, c)) for k, a, b, c in FIELD_RE.findall(rm.group(1))}
        sig = f.get("signature", "?")
        rows[sig.split("(")[0]] = f
    return rows


def allow_list(text):
    if text is None:
        return None
    return [l.strip() for l in text.splitlines() if l.strip() and not l.strip().startswith("#")]


changes = 0
out = ["## 信頼基盤の差分検出（ADR 0018 判断 1 T3・判断 2 S2）", ""]

# (1) 原始操作の表
b, h = raw_ops(show(base, RAW_OPS_FILE)), raw_ops(show(head, RAW_OPS_FILE))
sec, n_raw = [], 0
for k in sorted(set(b) | set(h)):
    n_raw += k not in b or k not in h or b[k] != h[k]
    if k not in b:
        sec.append(f"- 追加: `raw.{k}`（前提: {h[k].get('premise', '')}）")
    elif k not in h:
        sec.append(f"- 削除: `raw.{k}`（前提: {b[k].get('premise', '')}）")
    elif b[k] != h[k]:
        diffs = [x for x in sorted(set(b[k]) | set(h[k])) if b[k].get(x) != h[k].get(x)]
        sec.append(f"- 変更: `raw.{k}`（{', '.join(diffs)}）")
        if b[k].get("premise") != h[k].get("premise"):
            sec.append(f"  - 旧の前提: {b[k].get('premise', '')}")
            sec.append(f"  - 新の前提: {h[k].get('premise', '')}")
if sec:
    changes += n_raw
    out += ["### 原始操作の表（" + RAW_OPS_FILE + "）", *sec, ""]

# (2) 許可の一覧
bl, hl = allow_list(show(base, ALLOW_LIST_FILE)), allow_list(show(head, ALLOW_LIST_FILE))
if bl is not None or hl is not None:
    bs, hs = set(bl or []), set(hl or [])
    sec = [f"- 追加: `{x}`" for x in sorted(hs - bs)] + [f"- 削除: `{x}`" for x in sorted(bs - hs)]
    if sec:
        changes += len(sec)
        out += ["### 許可の一覧（" + ALLOW_LIST_FILE + "）", *sec, ""]

# (3) unchecked の関数
files = [
    p
    for p in git(
        "-c", "core.quotePath=false", "diff", "--name-only", "-z", "--no-renames", base, head
    ).split("\0")
    if p and scannable(p)
]
added, removed, premise, body = [], [], [], []
for p in files:
    bf = unchecked_fns(show(base, p) or "", p)
    hf = unchecked_fns(show(head, p) or "", p)
    for k in sorted(set(bf) | set(hf)):
        if k not in bf:
            added.append((k, hf[k][0]))
        elif k not in hf:
            removed.append((k, bf[k][0]))
        else:
            (bp, bb), (hp, hb) = bf[k], hf[k]
            if bp != hp:
                premise.append((k, bp, hp))
            if bb != hb:
                body.append(k)
if added or removed or premise or body:
    changes += len({k for k, _ in added + removed} | {k for k, _, _ in premise} | set(body))
    out.append("### `unchecked` の関数")
    for k, p in added:
        out.append(f"- 追加: `{k}`\n  - 前提の文: {p}")
    for k, p in removed:
        out.append(f"- 削除: `{k}`\n  - 前提の文: {p}")
    for k, o, n in premise:
        out.append(f"- 前提の文の変更: `{k}`\n  - 旧: {o}\n  - 新: {n}")
    for k in body:
        out.append(f"- 本体（宣言を含む）の変更: `{k}`（前提の文は上の変更の有無を見る）")
    out.append("")

if changes == 0:
    out.append("信頼基盤に関わる変更はない。")
else:
    out.append("所有者のレビューが要る変更を含む（上の前提の文を読む）。")
print(f"CHANGES={changes}")
print("\n".join(out))
PY
}

errf="$(mktemp)"
rc=0
out="$(detect 2>"$errf")" || rc=$?
if [ "$rc" -ne 0 ]; then
  msg="$(tr '\n' ' ' <"$errf")"
  rm -f "$errf"
  echo "信頼基盤の差分の検出に失敗した（終了コード $rc）: $msg" >&2
  if [ "$mode" = fail ]; then
    exit 1
  fi
  if [ -n "${GITHUB_ACTIONS:-}" ]; then
    echo "::warning title=信頼基盤の差分の検出に失敗::検出できなかった。ジョブのログを見る。"
  fi
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    printf '## 信頼基盤の差分検出\n\n検出に失敗した（印のみのため続行）: %s\n' "$msg" >>"$GITHUB_STEP_SUMMARY"
  fi
  exit 0
fi
rm -f "$errf"

changes="$(printf '%s\n' "$out" | sed -n '1s/^CHANGES=//p')"
report="$(printf '%s\n' "$out" | sed '1d')"
# 前提の文などに `::` で始まる行があっても、ワークフローコマンドとして解釈させない。
if [ -n "${GITHUB_ACTIONS:-}" ]; then
  token="tbd-$RANDOM$RANDOM$RANDOM"
  echo "::stop-commands::$token"
  printf '%s\n' "$report"
  echo "::$token::"
else
  printf '%s\n' "$report"
fi
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  printf '%s\n' "$report" >>"$GITHUB_STEP_SUMMARY"
fi

if [ "$changes" -gt 0 ]; then
  if [ "$mode" = fail ]; then
    echo "信頼基盤に関わる変更を $changes 件検出した（SIGIL_TRUST_BASE_MODE=fail）" >&2
    exit 1
  fi
  if [ -n "${GITHUB_ACTIONS:-}" ]; then
    echo "::warning title=信頼基盤の変更::信頼基盤に関わる変更を ${changes} 件検出した。ジョブの要約の前提の文を所有者が読む。"
  fi
fi
