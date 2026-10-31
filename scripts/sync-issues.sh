#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
python3 - "$REPO_ROOT/docs/BACKLOG.json" <<'PY'
import json
import re
import subprocess
import sys
import tempfile

backlog_path = sys.argv[1]
with open(backlog_path, "r", encoding="utf-8") as f:
    backlog = json.load(f)

raw = subprocess.check_output(
    ["gh", "issue", "list", "--state", "all", "--limit", "500", "--json", "number,title,state,labels,body"],
    text=True,
)
all_issues = json.loads(raw)

by_num = {}
by_id = {}
for iss in all_issues:
    by_num[int(iss["number"])] = iss
    body = iss.get("body") or ""
    m = re.search(r"<!-- backlog-id: ([a-z0-9\-]+) -->", body)
    if m:
        by_id[m.group(1)] = iss

for rule in backlog.get("unblock_rules", []):
    tnum = int(rule["issue"])
    target = by_num.get(tnum)
    if not target or target.get("state") != "OPEN":
        continue
    labels = [l["name"] for l in target.get("labels", [])]
    if "blocked" not in labels:
        continue
    if all(by_num.get(int(d), {}).get("state") == "CLOSED" for d in rule.get("depends_on", [])):
        subprocess.check_call(["gh", "issue", "edit", str(tnum), "--remove-label", "blocked"])
        subprocess.check_call([
            "gh", "issue", "comment", str(tnum),
            "--body", "前提 Issue が完了したため、`blocked` ラベルを自動解除しました（着手可能）。"
        ])
        print(f"Unblocked Issue #{tnum}")

for rule in backlog.get("spawn_rules", []):
    sid = rule["id"]
    if sid in by_id:
        continue
    dep_issues = rule.get("depends_on_issues", [])
    dep_ids = rule.get("depends_on_ids", [])
    if not all(by_num.get(int(d), {}).get("state") == "CLOSED" for d in dep_issues):
        continue
    if not all(by_id.get(did, {}).get("state") == "CLOSED" for did in dep_ids):
        continue
    body_text = rule["body"] + f"\n<!-- backlog-id: {sid} -->\n"
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", delete=False) as tf:
        tf.write(body_text)
        tf_path = tf.name
    cmd = ["gh", "issue", "create", "--title", rule["title"], "--body-file", tf_path]
    for lbl in rule.get("labels", []):
        cmd.extend(["--label", lbl])
    out = subprocess.check_output(cmd, text=True).strip()
    print(f"Spawned Issue ({sid}): {out}")
PY
