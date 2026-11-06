#!/usr/bin/env bash
# バックログの同期（アンロック・自動起票）。docs/BACKLOG.json の規則を、GitHub の Issue に反映する。
#
#   bash scripts/sync-issues.sh            # 実行する
#   bash scripts/sync-issues.sh --dry-run  # 何をするかだけを表示し、書き込まない（SYNC_DRY_RUN=1 でも同じ）
#
# GitHub へは `gh api`（REST）だけで書く。クラウドのセッションは GraphQL が使えず、`gh issue list`・`gh issue edit`・
# `gh issue create` が失敗するため。ページ送りも、`--paginate` が辿るリンクが数値 ID のパスで拒否されることがあるので、
# `page=` を自分で指定する。Windows 用の `sync-issues.ps1` は、従来どおり `gh issue ...` を使う。
#
# 環境変数（試験・特別な環境用）: GITHUB_REPOSITORY（`owner/repo`。なければ origin の URL から取る）、
# BACKLOG_PATH（既定は docs/BACKLOG.json）。
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BACKLOG_PATH="${BACKLOG_PATH:-$REPO_ROOT/docs/BACKLOG.json}"

DRY_RUN=0
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    *)
      echo "使い方: bash scripts/sync-issues.sh [--dry-run]" >&2
      exit 2
      ;;
  esac
done
if [ "${SYNC_DRY_RUN:-}" = "1" ]; then
  DRY_RUN=1
fi

REPO="${GITHUB_REPOSITORY:-}"
if [ -z "$REPO" ]; then
  origin_url="$(git -C "$REPO_ROOT" remote get-url origin)"
  REPO="$(printf '%s' "$origin_url" | sed -E 's#^(git@github\.com:|https?://([^/@]*@)?github\.com/)##; s#\.git$##')"
fi
if ! [[ "$REPO" =~ ^[^/]+/[^/]+$ ]]; then
  echo "リポジトリ名（owner/repo）を決められない: '$REPO'（GITHUB_REPOSITORY で指定する）" >&2
  exit 1
fi
export REPO DRY_RUN

python3 -I - "$BACKLOG_PATH" <<'PY'
import json
import os
import re
import subprocess
import sys

backlog_path = sys.argv[1]
repo = os.environ["REPO"]
dry = os.environ.get("DRY_RUN") == "1"

with open(backlog_path, "r", encoding="utf-8") as f:
    backlog = json.load(f)


def gh_api(method, path, payload=None, params=None):
    """`gh api` で REST を呼ぶ。本体は JSON（`--input -`）、問い合わせは `-f`（GET のみ）で渡す。"""
    cmd = ["gh", "api", "-X", method, path]
    for k, v in (params or {}).items():
        cmd += ["-f", f"{k}={v}"]
    data = None
    if payload is not None:
        cmd += ["--input", "-"]
        data = json.dumps(payload, ensure_ascii=False)
    r = subprocess.run(cmd, input=data, capture_output=True, text=True, encoding="utf-8")
    if r.returncode != 0:
        sys.exit(f"gh api が失敗した: {' '.join(cmd)}\n{r.stderr}")
    return json.loads(r.stdout) if r.stdout.strip() else None


# 全 Issue を読む（PR は除く。REST の issues には PR も含まれる）。ページごとに 100 件。
all_issues = []
page = 1
while True:
    chunk = gh_api(
        "GET",
        f"repos/{repo}/issues",
        params={"state": "all", "per_page": "100", "page": str(page)},
    )
    all_issues += [x for x in chunk if "pull_request" not in x]
    if len(chunk) < 100:
        break
    page += 1

by_num = {}
by_id = {}
for iss in all_issues:
    by_num[int(iss["number"])] = iss
    body = iss.get("body") or ""
    m = re.search(r"<!-- backlog-id: ([a-z0-9\-]+) -->", body)
    if m:
        by_id[m.group(1)] = iss


def is_closed(num):
    return by_num.get(int(num), {}).get("state") == "closed"


# 1. 前提がすべて CLOSED の Issue の `blocked` ラベルを外す。
for rule in backlog.get("unblock_rules", []):
    tnum = int(rule["issue"])
    target = by_num.get(tnum)
    if not target or target.get("state") != "open":
        continue
    labels = [l["name"] for l in target.get("labels", [])]
    if "blocked" not in labels:
        continue
    if all(is_closed(d) for d in rule.get("depends_on", [])):
        if dry:
            print(f"[dry-run] Unblock Issue #{tnum}")
            continue
        gh_api(
            "PATCH",
            f"repos/{repo}/issues/{tnum}",
            {"labels": [l for l in labels if l != "blocked"]},
        )
        gh_api(
            "POST",
            f"repos/{repo}/issues/{tnum}/comments",
            {"body": "前提 Issue が完了したため、`blocked` ラベルを自動解除しました（着手可能）。"},
        )
        print(f"Unblocked Issue #{tnum}")

# 2. 前提がすべて CLOSED で、まだ起票していない規則の Issue を起票する。
#    重複の判定は、本文の末尾の `<!-- backlog-id: <id> -->` で行う。
for rule in backlog.get("spawn_rules", []):
    sid = rule["id"]
    if sid in by_id:
        continue
    dep_issues = rule.get("depends_on_issues", [])
    dep_ids = rule.get("depends_on_ids", [])
    if not all(is_closed(d) for d in dep_issues):
        continue
    if not all(by_id.get(did, {}).get("state") == "closed" for did in dep_ids):
        continue
    if dry:
        print(f"[dry-run] Spawn Issue ({sid}): {rule['title']}")
        continue
    created = gh_api(
        "POST",
        f"repos/{repo}/issues",
        {
            "title": rule["title"],
            "body": rule["body"] + f"\n<!-- backlog-id: {sid} -->\n",
            "labels": rule.get("labels", []),
        },
    )
    print(f"Spawned Issue ({sid}): {created.get('html_url', '?')}")
PY
