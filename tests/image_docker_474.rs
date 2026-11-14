//! ADR 0035 / 6-2e: `tools/image/main.sigil` が生成した OCI archive の Docker 互換性テスト。
//!
//! `cargo test --test image_docker_474 -- --ignored`
mod common;

use common::native::{exec, scratch, try_build};
use sigil::exec::Arg;
use std::process::{Command, Output};

const MAIN: &str = include_str!("../tools/image/main.sigil");
const OWNERSHIP: &str = include_str!("../examples/03-ownership/ownership.sigil");

fn sources() -> Vec<&'static str> {
    vec![
        MAIN,
        include_str!("../tools/image/binary.sigil"),
        include_str!("../tools/image/elf.sigil"),
        include_str!("../tools/image/ustar.sigil"),
        include_str!("../tools/image/oci.sigil"),
    ]
}

fn docker(args: &[&str]) -> Output {
    Command::new("docker")
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("docker {:?} の起動に失敗: {e}", args))
}

fn docker_ok(args: &[&str]) -> String {
    let out = docker(args);
    assert!(
        out.status.success(),
        "docker {:?} failed (code {:?}):\nstdout: {}\nstderr: {}",
        args,
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn report_digest(report: &str, name: &str) -> String {
    let prefix = format!("{name} ");
    report
        .lines()
        .find_map(|l| l.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("{name} digest が報告にない:\n{report}"))
        .trim()
        .to_string()
}

/// 生成器で `examples/03-ownership/ownership.sigil` の静的 ELF から OCI archive を作り、
/// `docker load`・`docker inspect`・`docker run`（非 root 65532:65532）を検証する。
/// `cargo test --test image_docker_474 -- --ignored oci_archive_loads_inspects_and_runs_non_root_in_docker`
#[test]
#[ignore]
fn oci_archive_loads_inspects_and_runs_non_root_in_docker() {
    // 4. Docker の版・store の記録（docker が使えない環境では失敗とする）
    let version = docker_ok(&["--version"]);
    let info = docker_ok(&[
        "info",
        "--format",
        "ServerVersion={{.ServerVersion}} Driver={{.Driver}} DriverStatus={{json .DriverStatus}}",
    ]);
    println!("docker --version: {}", version.trim());
    println!("docker info: {}", info.trim());

    let dir = scratch("image-docker-474");
    let gen_exe = dir.join("image-gen");
    let app_elf = dir.join("ownership.elf");
    let archive_path = dir.join("ownership-oci.tar");

    // 生成器と入力 ELF（examples/03-ownership/ownership.sigil）をネイティブにビルドする
    try_build(&sources(), "main", &gen_exe, false).unwrap();
    try_build(&[OWNERSHIP], "main", &app_elf, false).unwrap();

    let gen_args = [
        Arg::Str(app_elf.to_string_lossy().into_owned()),
        Arg::Str(archive_path.to_string_lossy().into_owned()),
    ];
    let gen_out = exec(&gen_exe, &gen_args);
    assert_eq!(gen_out.code, Some(0), "{}", gen_out.stderr);
    assert!(
        gen_out.stdout.starts_with("image complete\nlayer sha256:"),
        "{}",
        gen_out.stdout
    );
    assert!(gen_out.stdout.ends_with("Ok(0)\n"), "{}", gen_out.stdout);

    let layer_digest = report_digest(&gen_out.stdout, "layer");
    let config_digest = report_digest(&gen_out.stdout, "config");

    // 1. 生成した OCI archive を docker load する
    let archive_arg = archive_path.to_string_lossy().into_owned();
    let load_out = docker_ok(&["load", "-i", &archive_arg]);
    println!("docker load: {}", load_out.trim());

    // index.json の org.opencontainers.image.ref.name は "sigil-app:latest"、
    // Docker の構成によってはタグ名または config digest（image ID）で参照される
    let image_ref = if docker(&["image", "inspect", "sigil-app:latest"])
        .status
        .success()
    {
        "sigil-app:latest".to_string()
    } else {
        config_digest.clone()
    };

    // 2 & 3. docker inspect で image config と非 root User 設定を検証する
    let inspect_fmt = concat!(
        "Os={{.Os}}\n",
        "Arch={{.Architecture}}\n",
        "User={{.Config.User}}\n",
        "Entrypoint={{json .Config.Entrypoint}}\n",
        "WorkingDir={{.Config.WorkingDir}}\n",
        "RootFS={{.RootFS.Type}}:{{json .RootFS.Layers}}"
    );
    let inspected = docker_ok(&["image", "inspect", "--format", inspect_fmt, &image_ref]);
    println!("docker inspect:\n{}", inspected.trim());
    let lines: Vec<&str> = inspected.lines().collect();
    assert_eq!(lines.len(), 6, "unexpected inspect output:\n{inspected}");
    assert_eq!(lines[0], "Os=linux");
    assert_eq!(lines[1], "Arch=amd64");
    let user = lines[2].strip_prefix("User=").unwrap();
    assert_eq!(
        user, "65532:65532",
        "生成 config の User は非 root 固定値であること"
    );
    assert!(
        !user.is_empty()
            && user != "0"
            && user != "root"
            && !user.starts_with("0:")
            && !user.starts_with("root:"),
        "User が root であってはならない: {user}"
    );
    assert_eq!(lines[3], "Entrypoint=[\"/app\"]");
    assert_eq!(lines[4], "WorkingDir=/");
    assert_eq!(lines[5], format!("RootFS=layers:[\"{layer_digest}\"]"));

    // 1 & 3. 生成 config の User ("65532:65532") のまま docker run し、終了値 0 と期待 stdout を検証する
    let run_out = docker(&["run", "--rm", &image_ref]);
    let _ = docker(&["rmi", "-f", &image_ref]);
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(
        run_out.status.code(),
        Some(0),
        "docker run stderr: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run_out.stdout), "150\n150\n()\n");
}
