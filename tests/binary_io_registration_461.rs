//! ADR 0036: 公開型・効果・SIR の結線。実ファイルには触れない。
use sigil::ir::{Builtin, Callee, Term};
use sigil::ErrorKind;

const HEAD: &str = "module m.registration\n  targets server\n  trust none\n\n";
const MATCH: &str = r#"
fn classify(e: BinaryIoError) -> Int
  intent "通常列挙型の全ての腕を識別する"
  requires none
  ensures none
  effects none
{
  return match e {
    BinaryIoError.InvalidLimit(n) => n,
    BinaryIoError.InvalidPath => 1,
    BinaryIoError.TooLarge(n) => n,
    BinaryIoError.UnsupportedFile => 3,
    BinaryIoError.Io(inner) => 4,
  };
}
"#;

fn source(body: &str) -> String {
    format!("{HEAD}{body}")
}

fn function(sig: &str, effects: &str, body: &str) -> String {
    format!("fn {sig}\n  intent \"登録の確認\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n")
}

#[test]
fn binary_error_has_five_exhaustive_arms() {
    sigil::check::check(&source(MATCH)).unwrap();
    for arm in [
        "    BinaryIoError.InvalidLimit(n) => n,\n",
        "    BinaryIoError.InvalidPath => 1,\n",
        "    BinaryIoError.TooLarge(n) => n,\n",
        "    BinaryIoError.UnsupportedFile => 3,\n",
        "    BinaryIoError.Io(inner) => 4,\n",
    ] {
        let err = sigil::check::check(&source(&MATCH.replace(arm, ""))).unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotExhaustive, "{arm}");
    }
}

#[test]
fn limit_error_payloads_are_ints_and_survive_matching() {
    for variant in ["InvalidLimit", "TooLarge"] {
        let entry = function(
            "main(n: Int) -> Int",
            "none",
            &format!("  return classify(BinaryIoError.{variant}(n));"),
        );
        let src = source(&format!("{MATCH}{entry}"));
        for n in [i64::MIN, -1, 0, 134217728, i64::MAX] {
            let out = sigil::ir_eval::run_sources_with(
                &[&src],
                "main",
                &[sigil::exec::Arg::Int(n)],
                &sigil::exec::Options::captured(),
            )
            .unwrap();
            assert_eq!(out.value, n.to_string());
        }
        let wrong = src.replace(
            &format!("classify(BinaryIoError.{variant}(n))"),
            &format!("classify(BinaryIoError.{variant}(true))"),
        );
        assert_eq!(
            sigil::check::check(&wrong).unwrap_err().kind,
            ErrorKind::TypeMismatch
        );
    }
}

#[test]
fn binary_api_and_error_are_os_layer_items() {
    let bodies = [
        function(
            "f(e: BinaryIoError) -> BinaryIoError",
            "none",
            "  return e;",
        ),
        function(
            "f() -> Result<Bytes, BinaryIoError>",
            "fs.read",
            "  return fs.read_bytes(&Path.new(&\"p\"), 0);",
        ),
        function(
            "f() -> Result<Unit, BinaryIoError>",
            "fs.write",
            "  return fs.write_bytes(&Path.new(&\"p\"), &Bytes.new(), 0);",
        ),
    ];
    for body in bodies {
        let src = source(&body).replace("targets server", "targets bare");
        assert_eq!(
            sigil::check::check(&src).unwrap_err().kind,
            ErrorKind::OsItemInCore
        );
    }
}

#[test]
fn binary_error_name_is_reserved() {
    for declaration in [
        "struct BinaryIoError {}\n",
        "enum BinaryIoError { Mine, }\n",
    ] {
        assert!(sigil::parser::parse(&source(declaration)).is_err());
    }
}

#[test]
fn binary_apis_require_exact_effects() {
    for (effect, call) in [
        ("fs.read", "fs.read_bytes(&p, 0)"),
        ("fs.write", "fs.write_bytes(&p, &Bytes.new(), 0)"),
    ] {
        let body = format!("  let p = Path.new(&\"p\");\n  let answer = {call};");
        let correct = source(&function("f()", effect, &body));
        sigil::check::check(&correct).unwrap();
        let missing = source(&function("f()", "none", &body));
        assert_eq!(
            sigil::check::check(&missing).unwrap_err().kind,
            ErrorKind::EffectMissing
        );
        let extra = source(&function("f()", "fs.read, fs.write", &body));
        assert_eq!(
            sigil::check::check(&extra).unwrap_err().kind,
            ErrorKind::EffectExtra
        );
    }
}

#[test]
fn binary_apis_lower_to_builtins_and_verify() {
    let src = source(&function(
        "main()",
        "fs.read, fs.write",
        "  let p = Path.new(&\"p\");\n  let read = fs.read_bytes(&p, 0);\n  let data = Bytes.new();\n  let write = fs.write_bytes(&p, &data, 0);",
    ));
    let program = sigil::ir_eval::lower_sources(&[&src], "main").unwrap();
    sigil::verify::verify(&program).unwrap();
    for want in [Builtin::FsReadBytes, Builtin::FsWriteBytes] {
        assert!(program.funcs.iter().flat_map(|f| &f.blocks).any(|b| {
            matches!(&b.term, Term::Call { func: Callee::Builtin(got), .. } if *got == want)
        }));
    }
}
