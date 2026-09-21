//! 解析器集成测试。
//!
//! 期望值与 Python 版 `lmutil_parser.parse_lmstat_output()` 的输出对齐
//! （可用 `python collector/rust/tests/python_parity_check.py` 重新生成对照结果），
//! 并额外覆盖 Rust 版新增的容错行为。

use eda_license_collector::lmutil::parse_lmstat_output;

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("读取 fixture {} 失败: {err}", path.display()))
}

#[test]
fn parses_realistic_lmstat_output() {
    let status = parse_lmstat_output(&fixture("lmstat_full.txt"));

    // Python 版因正则大小写/格式差异会漏掉这两项，Rust 版补齐
    assert_eq!(status.server_host, "licsrv01");
    assert_eq!(status.lmgrd_port, 27000);
    assert_eq!(status.vendor_daemon, "snpslmd");
    assert_eq!(status.status, "UP");
    assert!(status.is_up());
    assert!(status.parse_error.is_empty());

    assert_eq!(status.features.len(), 2);
    assert_eq!(status.total_licenses(), 30);
    assert_eq!(status.total_used(), 15);
    assert!((status.overall_usage_ratio() - 0.5).abs() < f64::EPSILON);

    let vcs = &status.features[0];
    assert_eq!(vcs.name, "vcs");
    assert_eq!(vcs.total, 20);
    assert_eq!(vcs.used, 12);
    assert_eq!(vcs.idle(), 8);
    assert_eq!(vcs.users.len(), 3);
    assert_eq!(vcs.users[0].username, "jsmith");
    assert_eq!(vcs.users[0].host, "desktop01:0");
    assert_eq!(vcs.users[0].display, "/dev/pts/3");
    assert_eq!(vcs.users[0].start_time, "06/02/2026 10:01");
    assert_eq!(vcs.users[0].feature, "vcs");
    assert_eq!(vcs.users[1].username, "lzhang");
    assert_eq!(vcs.users[1].display, "");

    let verdi = &status.features[1];
    assert_eq!(verdi.name, "verdi");
    assert_eq!(verdi.total, 10);
    assert_eq!(verdi.used, 3);
    assert_eq!(verdi.users.len(), 2);
    assert_eq!(verdi.users[1].username, "fchen");

    assert_eq!(
        status
            .users
            .iter()
            .map(|user| user.username.as_str())
            .collect::<Vec<_>>(),
        vec!["jsmith", "lzhang", "qzhou", "awu", "fchen"]
    );
}

#[test]
fn parses_legacy_table_and_brief_layouts() {
    let status = parse_lmstat_output(&fixture("lmstat_table.txt"));

    // 与 Python 版一致：`verdi` 行的第二列不是日期，无法匹配 Feature 明细正则，因此被忽略
    assert_eq!(status.features.len(), 2);

    let vcs = &status.features[0];
    assert_eq!(vcs.name, "vcs");
    assert_eq!(vcs.vendor, "synopsys");
    assert_eq!(vcs.total, 20);
    assert_eq!(vcs.used, 12);

    let xcelium = &status.features[1];
    assert_eq!(xcelium.name, "xcelium");
    assert_eq!(xcelium.total, 40);
    assert_eq!(xcelium.used, 25);

    assert_eq!(status.total_licenses(), 60);
    assert_eq!(status.total_used(), 37);
    assert!(status.users.is_empty());
}

#[test]
fn unreachable_server_is_reported_as_down() {
    let status = parse_lmstat_output(&fixture("lmstat_connect_error.txt"));

    assert_eq!(status.status, "DOWN");
    assert!(!status.is_up());
    assert!(status.unreachable);
    assert!(status.features.is_empty());
    assert!(
        status
            .parse_error
            .contains("Cannot connect to license server"),
        "parse_error 应包含失败原因，实际: {}",
        status.parse_error
    );
}

#[test]
fn keeps_successful_data_even_if_output_has_error_lines() {
    // 多 license 文件场景：部分 server 不可达，但有效数据仍然存在时不应判为 DOWN
    let output = "\
Flexible License Manager status on Thu 6/2/2026 10:23

Users of vcs:  (Total of 20 licenses issued;  Total of 12 licenses in use)

Error getting status: Cannot connect to license server system. (-15,10:10061)
";
    let status = parse_lmstat_output(output);

    assert_eq!(status.status, "");
    assert!(status.is_up());
    assert!(status.parse_error.is_empty());
    assert!(!status.unreachable);
    assert_eq!(status.features.len(), 1);
    assert_eq!(status.features[0].used, 12);
}

#[test]
fn repeated_feature_sections_are_merged_in_order() {
    let output = "\
Users of vcs:  (Total of 20 licenses issued;  Total of 12 licenses in use)
Users of verdi:  (Total of 10 licenses issued;  Total of 3 licenses in use)
Users of vcs:  (Total of 22 licenses issued;  Total of 14 licenses in use)
";
    let status = parse_lmstat_output(output);

    assert_eq!(status.features.len(), 2);
    assert_eq!(status.features[0].name, "vcs");
    assert_eq!(status.features[0].total, 22);
    assert_eq!(status.features[0].used, 14);
    assert_eq!(status.features[1].name, "verdi");
}

#[test]
fn empty_output_produces_empty_status() {
    let status = parse_lmstat_output("");

    assert!(status.features.is_empty());
    assert!(status.users.is_empty());
    assert_eq!(status.status, "");
    assert!(status.parse_error.is_empty());
    assert_eq!(status.total_licenses(), 0);
    assert_eq!(status.overall_usage_ratio(), 0.0);
}
