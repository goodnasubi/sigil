//! ADR 0015 追補: 起動引数と明示終了を、両方の実行入口で確かめる。
const SRC: &str = r#"
module m.argv_exit
  targets server
  trust none

fn read_argv() -> List<String>
  intent "起動引数を独立した一覧として読み直す"
  requires none
  ensures none
  effects env
{
  let mut args = env.args();
  args.push("変更");
  return env.args();
}

fn typed(n: Int) -> (Int, List<String>)
  intent "型付き引数の値と元のトークンを返す"
  requires none
  ensures none
  effects env
{
  return (n, env.args());
}

fn stop(code: Int)
  intent "指定したコードで終わる"
  requires none
  ensures false
  effects process
{
  let live = "終了までの所有値";
  process.exit(code);
  let _ = live.char_count();
}

fn finish(code: Int) -> Int
  intent "出力後に、呼び出し先で実行全体を終了する"
  requires none
  ensures none
  effects console, process
{
  console.log(&123);
  stop(code);
  console.log(&456);
  return 789;
}

fn finish_with_output(message: String)
  intent "パイプに長い文字列を出力して終了する"
  requires none
  ensures none
  effects console, process
{
  console.log(&message);
  stop(2);
}

fn ordinary() -> Int
  intent "通常の戻り値を返す"
  requires none
  ensures none
  effects none
{
  return 39;
}

fn divide(n: Int) -> Int
  intent "ゼロなら異常停止する"
  requires none
  ensures none
  effects none
{
  return 1 / n;
}

fn worker() -> Int
  intent "タスクから実行全体を終了する"
  requires none
  ensures none
  effects process
{
  stop(43);
  return 9;
}

fn concurrent() -> Int
  intent "子タスクを起動し、終了コードを観測する"
  requires none
  ensures none
  effects process, task.spawn, task.suspend
{
  return task.scope(|s| {
    let h = s.spawn(move || worker());
    return h.join();
  });
}
"#;

#[test]
fn evaluator_reports_exit_to_the_host() {
    use sigil::exec::{Arg, Options, RunError};
    use sigil::ir_eval::run_with;

    let opts = Options::captured();
    let exited = run_with(SRC, "finish", &[Arg::Int(70)], &opts).unwrap_err();
    assert!(matches!(exited.error, RunError::Exit(70)));
    assert_eq!(exited.output, "123\n");
    assert_eq!(run_with(SRC, "ordinary", &[], &opts).unwrap().value, "39");
    assert_eq!(run_with(SRC, "read_argv", &[], &opts).unwrap().value, "[]");
}

#[test]
fn sir_accepts_explicit_exit_with_a_live_owned_value() {
    use sigil::ir::{Block, Builtin, Callee, Term};

    let mut program = sigil::ir_eval::lower_sources(&[SRC], "stop").unwrap();
    sigil::verify::verify(&program).unwrap();

    let blocks = &mut program.funcs[program.entry].blocks;
    let (next, cleanup) = blocks
        .iter()
        .find_map(|block| match &block.term {
            Term::Call {
                func: Callee::Builtin(Builtin::ProcessExit),
                next,
                cleanup,
                ..
            } => Some((*next, *cleanup)),
            _ => None,
        })
        .unwrap();
    // この二つの辺を通るなら所有値の破棄が欠ける。明示終了からはどちらにも到達しない。
    blocks[next] = Block {
        stmts: vec![],
        term: Term::Return,
    };
    blocks[cleanup] = Block {
        stmts: vec![],
        term: Term::Resume,
    };
    sigil::verify::verify(&program).unwrap();
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod common;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod cli {
    use super::{common, SRC};
    use std::ffi::{OsStr, OsString};
    use std::path::PathBuf;
    use std::process::{Command, Output};

    struct Cli {
        root: PathBuf,
        src: PathBuf,
    }

    impl Cli {
        fn new() -> Self {
            let (root, src) = common::stage_src("argv_exit_711", SRC.trim_start());
            Self { root, src }
        }

        fn command(&self, mode: &str, entry: &str, raw: bool) -> Command {
            let mut command = Command::new(env!("CARGO_BIN_EXE_sigil"));
            command.arg(mode).arg("--root").arg(&self.root);
            if raw {
                command.arg("--raw-argv");
            }
            command.arg(&self.src).arg(entry);
            command
        }

        fn run(&self, entry: &str, raw: bool, args: &[impl AsRef<OsStr>]) -> Output {
            self.command("run", entry, raw).args(args).output().unwrap()
        }

        fn build(&self, entry: &str, raw: bool) -> (PathBuf, Output) {
            let exe = self.root.join(format!("{entry}-{raw}"));
            let out = self
                .command("build", entry, raw)
                .arg("-o")
                .arg(&exe)
                .output()
                .unwrap();
            (exe, out)
        }

        fn executable(&self, entry: &str, raw: bool) -> PathBuf {
            let (exe, out) = self.build(entry, raw);
            assert_eq!(
                out.status.code(),
                Some(0),
                "{entry}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            exe
        }
    }

    impl Drop for Cli {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn assert_output(out: Output, code: i32, stdout: &str) {
        assert_eq!(
            out.status.code(),
            Some(code),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8(out.stdout).unwrap(), stdout);
        assert_eq!(String::from_utf8(out.stderr).unwrap(), "");
    }

    #[test]
    fn raw_argv_preserves_os_tokens_and_returns_owned_lists() {
        let cli = Cli::new();
        let exe = cli.executable("read_argv", true);
        let args = [
            "--max-response-body",
            "67108864",
            "",
            "二 語",
            "[unclosed",
            "[1,true]",
            "false",
            "--raw-argv",
        ];
        let want = "[\"--max-response-body\", \"67108864\", \"\", \"二 語\", \"[unclosed\", \"[1,true]\", \"false\", \"--raw-argv\"]\n";
        assert_output(cli.run("read_argv", true, &args), 0, want);
        assert_output(Command::new(&exe).args(args).output().unwrap(), 0, want);
        assert_output(cli.run("read_argv", true, &[] as &[&str]), 0, "[]\n");
        assert_output(Command::new(&exe).output().unwrap(), 0, "[]\n");

        {
            use std::os::unix::ffi::OsStringExt;

            let args = [OsString::from_vec(b"a\xff\xe3\x81b".to_vec())];
            let want = "[\"a��b\"]\n";
            assert_output(cli.run("read_argv", true, &args), 0, want);
            assert_output(Command::new(&exe).args(args).output().unwrap(), 0, want);
        }

        assert_eq!(cli.run("read_argv", false, &["1"]).status.code(), Some(1));
        let exe = cli.executable("read_argv", false);
        assert_eq!(
            Command::new(exe).arg("1").output().unwrap().status.code(),
            Some(1)
        );
    }

    #[test]
    fn typed_entry_keeps_its_signature_and_original_tokens() {
        let cli = Cli::new();
        let exe = cli.executable("typed", false);
        let want = "(42, [\"0042\"])\n";
        assert_output(cli.run("typed", false, &["0042"]), 0, want);
        assert_output(Command::new(exe).arg("0042").output().unwrap(), 0, want);
        assert!(!cli.run("typed", true, &["0042"]).status.success());
        let (exe, out) = cli.build("typed", true);
        assert!(!out.status.success());
        assert!(!exe.exists());
    }

    #[test]
    fn explicit_exit_keeps_prior_output_and_uses_the_low_byte() {
        let cli = Cli::new();
        let exe = cli.executable("finish", false);
        for (arg, code) in [("70", 70), ("1", 1), ("-1", 255), ("256", 0)] {
            assert_output(cli.run("finish", false, &[arg]), code, "123\n");
            assert_output(Command::new(&exe).arg(arg).output().unwrap(), code, "123\n");
        }
    }

    #[test]
    fn explicit_exit_keeps_large_piped_output() {
        let cli = Cli::new();
        let exe = cli.executable("finish_with_output", false);
        let message = "x".repeat(32 * 1024);
        let want = format!("{message}\n");
        assert_output(cli.run("finish_with_output", false, &[&message]), 2, &want);
        assert_output(Command::new(exe).arg(&message).output().unwrap(), 2, &want);
    }

    #[test]
    fn normal_return_and_abnormal_stop_keep_their_cli_behavior() {
        let cli = Cli::new();
        let exe = cli.executable("ordinary", false);
        assert_output(cli.run("ordinary", false, &[] as &[&str]), 0, "39\n");
        assert_output(Command::new(exe).output().unwrap(), 0, "39\n");

        let exe = cli.executable("divide", false);
        let evaluated = cli.run("divide", false, &["0"]);
        let native = Command::new(exe).arg("0").output().unwrap();
        for out in [evaluated, native] {
            assert_eq!(out.status.code(), Some(70));
            assert!(out.stdout.is_empty());
            assert!(String::from_utf8(out.stderr).unwrap().contains("異常停止"));
        }
    }

    #[test]
    fn exit_from_a_child_task_ends_the_program() {
        let cli = Cli::new();
        let exe = cli.executable("concurrent", false);
        assert_output(cli.run("concurrent", false, &[] as &[&str]), 43, "");
        assert_output(Command::new(exe).output().unwrap(), 43, "");
    }
}
