//! ADR 0035 / 6-2d: 製品の入口をそのまま評価器・ネイティブで駆動する。
mod common;

use common::native::{exec, scratch, try_build};
use sigil::exec::{Arg, Options};
use std::path::Path;

const MAIN: &str = include_str!("../tools/image/main.sigil");
fn sources(main: &str) -> Vec<&str> {
    vec![
        main,
        include_str!("../tools/image/binary.sigil"),
        include_str!("../tools/image/elf.sigil"),
        include_str!("../tools/image/ustar.sigil"),
        include_str!("../tools/image/oci.sigil"),
    ]
}
fn fixture() -> Vec<u8> {
    let mut b = vec![0; 121];
    b[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    b[16..18].copy_from_slice(&2u16.to_le_bytes());
    b[18..20].copy_from_slice(&62u16.to_le_bytes());
    b[20..24].copy_from_slice(&1u32.to_le_bytes());
    b[32..40].copy_from_slice(&64u64.to_le_bytes());
    b[52..54].copy_from_slice(&64u16.to_le_bytes());
    b[54..56].copy_from_slice(&56u16.to_le_bytes());
    b[56..58].copy_from_slice(&1u16.to_le_bytes());
    b[64..68].copy_from_slice(&1u32.to_le_bytes());
    b[96..104].copy_from_slice(&121u64.to_le_bytes());
    b[104..112].copy_from_slice(&121u64.to_le_bytes());
    b
}
fn args(input: &Path, output: &Path) -> [Arg; 2] {
    [input, output].map(|p| Arg::Str(p.to_string_lossy().into_owned()))
}
fn run(main: &str, exe: &Path, input: &Path, output: &Path, native: bool) -> String {
    let args = args(input, output);
    if native {
        let got = exec(exe, &args);
        assert_eq!(got.code, Some(0), "{}", got.stderr);
        got.stdout
    } else {
        let got =
            sigil::ir_eval::run_sources_with(&sources(main), "main", &args, &Options::captured())
                .unwrap_or_else(|e| panic!("{e}"));
        format!("{}{}\n", got.output, got.value)
    }
}

#[test]
fn exact_effects_and_trust_none_are_checked() {
    let program = sigil::ir_eval::lower_sources(&sources(MAIN), "main").unwrap();
    sigil::verify::verify(&program).unwrap();
    for effects in [
        "fs.read, fs.write",
        "fs.read, console",
        "fs.write, console",
        "fs.read, fs.write, console, env",
    ] {
        let changed = MAIN.replace("effects console, fs", &format!("effects {effects}"));
        let error = sigil::ir_eval::lower_sources(&sources(&changed), "main").unwrap_err();
        assert!(error.to_string().contains("効果"), "{error}");
    }
    assert!(MAIN.contains("trust none"));
    assert!(!MAIN.contains("unchecked"));
    assert!(!MAIN.contains("raw."));
}

#[test]
fn entry_reproducibility_references_digests_and_failures() {
    let dir = scratch("image-generator");
    let exe = dir.join("image");
    try_build(&sources(MAIN), "main", &exe, false).unwrap();
    let elf = fixture();
    let mut reference = None;
    let mut reports = None;
    for n in 0..2 {
        let cwd = dir.join(format!("different-directory-{n}"));
        std::fs::create_dir(&cwd).unwrap();
        let input = cwd.join(if n == 0 { "original.elf" } else { "renamed" });
        std::fs::write(&input, &elf).unwrap();
        // 入力メタデータも変える。生成器はホスト時刻を読む効果を持たない。
        let status = std::process::Command::new("touch")
            .args(["-d", if n == 0 { "2000-01-01" } else { "2030-01-01" }])
            .arg(&input)
            .status()
            .unwrap();
        assert!(status.success());
        for native in [false, true] {
            let output = cwd.join(if native { "native.tar" } else { "eval.tar" });
            let report = run(MAIN, &exe, &input, &output, native);
            assert!(
                report.starts_with("image complete\nlayer sha256:"),
                "{report}"
            );
            assert!(report.ends_with("Ok(0)\n"));
            let archive = std::fs::read(&output).unwrap();
            if let Some(ref expected) = reference {
                assert_eq!(&archive, expected);
            }
            if let Some(ref expected) = reports {
                assert_eq!(&report, expected);
            }
            reference = Some(archive);
            reports = Some(report);
        }
        // 別の実ホスト時刻でも生成する。時刻を差し替える効果は追加しない。
        if n == 0 {
            std::thread::sleep(std::time::Duration::from_millis(1100));
        }
    }
    let original = dir.join("different-directory-0/eval.tar");
    let report_path = dir.join("report.txt");
    std::fs::write(&report_path, reports.as_ref().unwrap()).unwrap();
    let changed_input = dir.join("changed.elf");
    let mut changed = elf.clone();
    changed[120] = 1;
    std::fs::write(&changed_input, changed).unwrap();
    let changed_output = dir.join("changed.tar");
    let changed_report = run(MAIN, &exe, &changed_input, &changed_output, false);
    assert_ne!(std::fs::read(&changed_output).unwrap(), reference.unwrap());
    for (a, b) in reports
        .unwrap()
        .lines()
        .skip(1)
        .take(4)
        .zip(changed_report.lines().skip(1).take(4))
    {
        assert_ne!(a, b);
    }
    assert_eq!(
        run(MAIN, &exe, &changed_input, &changed_output, true),
        changed_report
    );
    let python = std::process::Command::new("python3").arg("-c").arg(r#"
import hashlib, io, json, sys, tarfile
data = open(sys.argv[1], 'rb').read()
reported = dict(line.split() for line in open(sys.argv[2]).read().splitlines()[1:5])
assert reported['archive'] == 'sha256:' + hashlib.sha256(data).hexdigest()
with tarfile.open(fileobj=io.BytesIO(data), mode='r:') as tar:
    members = tar.getmembers()
    assert [m.name for m in members[:2]] == ['index.json', 'oci-layout'] and len(members) == 5
    assert all(m.isfile() and m.mode == 0o644 and m.uid == m.gid == m.mtime == 0 and m.uname == m.gname == '' for m in members)
    files = {m.name: tar.extractfile(m).read() for m in members}
def ref(d):
    b = files['blobs/sha256/' + d['digest'][7:]]
    assert d['size'] == len(b) and d['digest'] == 'sha256:' + hashlib.sha256(b).hexdigest()
    return b
assert json.loads(files['oci-layout']) == {'imageLayoutVersion': '1.0.0'}
md = json.loads(files['index.json'])['manifests'][0]
assert md['digest'] == reported['manifest']
assert md['platform'] == {'architecture':'amd64','os':'linux'}
assert md['annotations'] == {'org.opencontainers.image.ref.name':'sigil-app:latest'}
manifest = json.loads(ref(md))
assert manifest['config']['digest'] == reported['config']
assert manifest['layers'][0]['digest'] == reported['layer']
cfg = json.loads(ref(manifest['config']))
assert cfg['config'] == {'User':'65532:65532','Entrypoint':['/app'],'WorkingDir':'/'}
assert cfg['rootfs'] == {'type':'layers','diff_ids':[reported['layer']]}
assert 'created' not in cfg and 'history' not in cfg
with tarfile.open(fileobj=io.BytesIO(ref(manifest['layers'][0])), mode='r:') as layer:
    app, = layer.getmembers()
    assert app.name == 'app' and app.mode == 0o555 and app.uid == app.gid == app.mtime == 0
    assert layer.extractfile(app).read() == open(sys.argv[3], 'rb').read()
"#).arg(&original).arg(&report_path).arg(dir.join("different-directory-0/original.elf")).output().unwrap();
    assert!(
        python.status.success(),
        "{}",
        String::from_utf8_lossy(&python.stderr)
    );
    let valid = dir.join("different-directory-0/original.elf");
    let preserved = dir.join("preserved.tar");
    let bad = dir.join("invalid.elf");
    std::fs::write(&bad, b"not ELF").unwrap();
    for native in [false, true] {
        std::fs::write(&preserved, b"preserve").unwrap();
        assert_eq!(run(MAIN, &exe, &bad, &preserved, native), "Err(400)\n");
        assert_eq!(std::fs::read(&preserved).unwrap(), b"preserve");
        assert_eq!(
            run(MAIN, &exe, &dir.join("missing"), &preserved, native),
            "Err(1)\n"
        );
        assert_eq!(std::fs::read(&preserved).unwrap(), b"preserve");
        let absent = dir.join("missing-parent/out.tar");
        assert_eq!(run(MAIN, &exe, &valid, &absent, native), "Err(2)\n");
        assert!(!absent.exists());
        assert_eq!(run(MAIN, &exe, &valid, &dir, native), "Err(2)\n");
    }
    // 書込み済みでErrとなる境界を注入（close失敗等）。実際のI/Oは既存APIを通す。
    let injected = format!("{}\nfn failed_write(p: &Path, b: &Bytes, max: Int) -> Result<Int, Int>\n  intent \"全量出力後の失敗を注入\"\n  requires none\n  ensures none\n  effects fs.write\n{{ fs.write_bytes(&p, &b, max).map_err(|e| 9)?; return Err(9); }}\n", MAIN.replace("fs.write_bytes(", "failed_write("));
    let failed_exe = dir.join("failed-image");
    try_build(&sources(&injected), "main", &failed_exe, false).unwrap();
    for native in [false, true] {
        assert_eq!(
            run(&injected, &failed_exe, &valid, &preserved, native),
            "Err(2)\n"
        );
        assert_eq!(
            std::fs::read(&preserved).unwrap(),
            std::fs::read(&original).unwrap()
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}
