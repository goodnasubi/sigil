//! `scripts/sync-issues.sh`（バックログの同期）の試験。
//!
//! 本物の GitHub へは書かない。`gh` を、呼び出しと標準入力を記録して、決まった JSON を返すスクリプトに差し替え、
//! 次を確かめる:
//! - REST（`gh api`）だけを使い、GraphQL（`gh issue ...`）を使わない（クラウドのセッションは GraphQL が使えない）。
//! - 前提がすべて閉じた Issue の `blocked` を、ほかのラベルを残して外し、コメントする。
//! - 前提がすべて閉じていて、まだ起票していない規則だけを起票し、本文の末尾に `<!-- backlog-id: <id> -->` を付ける。
//! - 起票済み（目印あり）・前提が未完了（開いている Issue・未起票の規則）のものは、何もしない。
//! - PR（`pull_request` を持つ項目）は Issue として数えない。
//! - ページ送りは `page=` を自分で指定する。`--dry-run` は書き込まない。
//!
//! bash と python3 が要るので、Unix のみで走らせる。

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 呼び出しを `LOG` に 1 行ずつ記録する偽の `gh`。`api -X GET .../issues` には `STUB_ISSUES`（ページごと
/// `STUB_ISSUES.<page>`）の JSON を返し、ほかには `{"html_url":"https://example.test/issues/999"}` を返す。
const STUB_GH: &str = r#"#!/usr/bin/env bash
echo "CALL $*" >> "$STUB_LOG"
case " $* " in
  *" --input -"*) echo "STDIN $(cat)" >> "$STUB_LOG" ;;
esac
if [ "$1" = "api" ] && [ "$2" = "-X" ] && [ "$3" = "GET" ]; then
  page=1
  prev=""
  for a in "$@"; do
    if [ "$prev" = "-f" ]; then
      case "$a" in page=*) page="${a#page=}" ;; esac
    fi
    prev="$a"
  done
  f="$STUB_ISSUES.$page"
  if [ -f "$f" ]; then cat "$f"; else echo "[]"; fi
  exit 0
fi
echo '{"html_url":"https://example.test/issues/999"}'
"#;

struct Env {
    dir: PathBuf,
}

impl Env {
    fn new(tag: &str) -> Env {
        let dir = std::env::temp_dir().join(format!("sync_issues_{}_{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let gh = dir.join("bin/gh");
        std::fs::write(&gh, STUB_GH).unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        Env { dir }
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let p = self.dir.join(name);
        std::fs::write(&p, text).unwrap();
        p
    }

    /// スクリプトを走らせ、記録した `gh` の呼び出し（行ごと）と標準出力を返す。
    fn run(&self, args: &[&str]) -> (Vec<String>, String) {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/sync-issues.sh");
        let path = format!(
            "{}:{}",
            self.dir.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let log = self.dir.join("calls.log");
        let _ = std::fs::remove_file(&log);
        let out = Command::new("bash")
            .arg(&script)
            .args(args)
            .env("PATH", path)
            .env("GITHUB_REPOSITORY", "o/r")
            .env("BACKLOG_PATH", self.dir.join("backlog.json"))
            .env("STUB_LOG", &log)
            .env("STUB_ISSUES", self.dir.join("issues"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "同期が失敗した: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let calls = std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect();
        (calls, String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const BACKLOG: &str = r#"{
  "unblock_rules": [
    {"issue": 10, "depends_on": [9]},
    {"issue": 20, "depends_on": [10]}
  ],
  "spawn_rules": [
    {"id": "ready-one", "depends_on_issues": [9], "title": "起票される", "labels": ["lane:E", "x"], "body": "本文です"},
    {"id": "already-filed", "depends_on_issues": [9], "title": "起票済み", "labels": [], "body": "b"},
    {"id": "waits-on-open", "depends_on_issues": [10], "title": "前提が開いている", "labels": [], "body": "b"},
    {"id": "waits-on-spawn", "depends_on_ids": ["ready-one"], "title": "前提が未起票", "labels": [], "body": "b"},
    {"id": "after-filed", "depends_on_ids": ["already-filed"], "title": "起票済みの後続", "labels": ["lane:E"], "body": "後続"}
  ]
}"#;

/// #9 は閉じ、#10 は開いて `blocked` と `lane:A1`、#13 は起票済みの目印を持つ（閉じている）、#12 は PR。
const ISSUES: &str = r##"[
  {"number": 9, "state": "closed", "labels": [], "body": ""},
  {"number": 10, "state": "open", "labels": [{"name": "blocked"}, {"name": "lane:A1"}], "body": ""},
  {"number": 12, "state": "closed", "labels": [], "body": "", "pull_request": {"url": "x"}},
  {"number": 13, "state": "closed", "labels": [], "body": "x\n<!-- backlog-id: already-filed -->\n"},
  {"number": 20, "state": "open", "labels": [{"name": "blocked"}], "body": ""}
]"##;

fn setup(tag: &str) -> Env {
    let e = Env::new(tag);
    e.write("backlog.json", BACKLOG);
    e.write("issues.1", ISSUES);
    e
}

fn writes(calls: &[String]) -> Vec<&String> {
    calls
        .iter()
        .filter(|c| c.starts_with("CALL api -X PATCH") || c.starts_with("CALL api -X POST"))
        .collect()
}

#[test]
fn uses_rest_only_and_never_graphql_issue_commands() {
    let e = setup("rest");
    let (calls, _) = e.run(&[]);
    assert!(!calls.is_empty());
    for c in calls.iter().filter(|c| c.starts_with("CALL ")) {
        assert!(c.starts_with("CALL api "), "gh api 以外を呼んだ: {c}");
    }
}

#[test]
fn unblocks_when_all_dependencies_are_closed_and_keeps_other_labels() {
    let e = setup("unblock");
    let (calls, out) = e.run(&[]);
    // #10 は前提 #9 が閉じている → blocked だけを外す（lane:A1 は残る）。#20 は前提 #10 が開いているので動かさない。
    assert!(
        calls
            .iter()
            .any(|c| c == "CALL api -X PATCH repos/o/r/issues/10 --input -"),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|c| c == r#"STDIN {"labels": ["lane:A1"]}"#),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|c| c == "CALL api -X POST repos/o/r/issues/10/comments --input -"),
        "{calls:?}"
    );
    assert!(out.contains("Unblocked Issue #10"), "{out}");
    assert!(
        !calls.iter().any(|c| c.contains("issues/20")),
        "前提が開いている #20 を動かした: {calls:?}"
    );
}

#[test]
fn spawns_only_ready_rules_with_the_backlog_id_marker() {
    let e = setup("spawn");
    let (calls, out) = e.run(&[]);
    let creates: Vec<usize> = calls
        .iter()
        .enumerate()
        .filter(|(_, c)| *c == "CALL api -X POST repos/o/r/issues --input -")
        .map(|(i, _)| i)
        .collect();
    // 起票されるのは ready-one（前提 #9 が閉じ、未起票）と after-filed（前提の起票済みの Issue #13 が閉じている）の 2 件。
    assert_eq!(creates.len(), 2, "{calls:?}");
    let bodies: Vec<&String> = creates.iter().map(|i| &calls[i + 1]).collect();
    let first = bodies
        .iter()
        .find(|b| b.contains("起票される"))
        .expect("ready-one が起票されない");
    assert!(first.contains(r#""labels": ["lane:E", "x"]"#), "{first}");
    assert!(
        first.contains(r"\n<!-- backlog-id: ready-one -->\n"),
        "目印がない: {first}"
    );
    assert!(
        bodies.iter().any(|b| b.contains("起票済みの後続")),
        "after-filed が起票されない: {bodies:?}"
    );
    // 起票済み・前提が開いている・前提が未起票のものは起票しない。
    for skipped in ["起票済み\"", "前提が開いている", "前提が未起票"] {
        assert!(
            !bodies.iter().any(|b| b.contains(skipped)),
            "{skipped} を起票した: {bodies:?}"
        );
    }
    assert!(
        out.contains("Spawned Issue (ready-one): https://example.test/issues/999"),
        "{out}"
    );
}

#[test]
fn a_pull_request_is_not_counted_as_an_issue() {
    // #12 は閉じた PR。前提に #12 を持つ規則は、PR が Issue として数えられないので、起票されない。
    let e = Env::new("pr");
    e.write(
        "backlog.json",
        r#"{"unblock_rules": [], "spawn_rules": [
          {"id": "needs-pr", "depends_on_issues": [12], "title": "PR 前提", "labels": [], "body": "b"}]}"#,
    );
    e.write("issues.1", ISSUES);
    let (calls, _) = e.run(&[]);
    assert!(writes(&calls).is_empty(), "{calls:?}");
}

#[test]
fn dry_run_writes_nothing_and_says_what_it_would_do() {
    let e = setup("dry");
    let (calls, out) = e.run(&["--dry-run"]);
    assert!(writes(&calls).is_empty(), "書き込んだ: {calls:?}");
    assert!(out.contains("[dry-run] Unblock Issue #10"), "{out}");
    assert!(out.contains("[dry-run] Spawn Issue (ready-one)"), "{out}");
}

#[test]
fn reads_every_page_by_explicit_page_numbers() {
    // 1 ページ目が 100 件で埋まっていれば、2 ページ目を読む（`--paginate` のリンクは使わない）。
    let e = Env::new("pages");
    e.write(
        "backlog.json",
        r#"{"unblock_rules": [{"issue": 150, "depends_on": [9]}], "spawn_rules": []}"#,
    );
    let mut page1: Vec<String> =
        vec![r#"{"number": 9, "state": "closed", "labels": [], "body": ""}"#.into()];
    for n in 100..199 {
        page1.push(format!(
            r#"{{"number": {n}, "state": "closed", "labels": [], "body": ""}}"#
        ));
    }
    assert_eq!(page1.len(), 100);
    e.write("issues.1", &format!("[{}]", page1.join(",")));
    // #150 は 2 ページ目にだけいる。
    e.write(
        "issues.2",
        r#"[{"number": 150, "state": "open", "labels": [{"name": "blocked"}], "body": ""}]"#,
    );
    let (calls, out) = e.run(&[]);
    assert!(
        calls
            .iter()
            .any(|c| c.contains("repos/o/r/issues") && c.contains("page=2")),
        "2 ページ目を読んでいない: {calls:?}"
    );
    assert!(out.contains("Unblocked Issue #150"), "{out}");
}
