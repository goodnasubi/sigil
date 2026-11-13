//! 古い記録を除いた native-report の下限と飛ばし上限を検査する（#429）。
#![cfg(unix)]

use std::process::Command;

#[test]
fn fresh_report_checks_both_boundaries_in_a_custom_target_dir() {
    let target = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-report-429-{}", std::process::id()));
    let tmp = target.join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let report = tmp.join("native-report-fixture-1.tsv");
    let run = |checked: usize, skipped: usize| {
        let mut rows = "checked\t\tf\n".repeat(checked);
        rows.push_str(&"skip\t停止でない失敗\tf\n".repeat(skipped));
        // 参照・Lock の入口は引き続き上限から除外する。
        rows.push_str(&"skip\t仕様上ビルドで止める入口引数（参照・Lock）\tf\n".repeat(123));
        std::fs::write(&report, rows).unwrap();
        Command::new("sh")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/scripts/native-report.sh"
            ))
            .env("CARGO_TARGET_DIR", &target)
            .env("SIGIL_NATIVE_MIN_CHECKED", "800")
            .env("SIGIL_NATIVE_MAX_SKIP", "17")
            .output()
            .unwrap()
    };
    for checked in [800, 803] {
        let output = run(checked, 17);
        assert!(output.status.success(), "{:?}", output);
        assert!(String::from_utf8_lossy(&output.stdout)
            .contains(&format!("突き合わせ {checked} 件（下限 800）")));
    }
    let output = run(799, 17);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("799 < 800"));
    let output = run(803, 18);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("18 > 17"));
    std::fs::remove_dir_all(target).unwrap();
}
