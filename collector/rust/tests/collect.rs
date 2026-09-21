//! `collect_server()` 进程级行为测试（不依赖真实 FlexLM 环境）。

use std::time::Duration;

use eda_license_collector::config::LicenseServerConfig;
use eda_license_collector::lmutil::collect_server;

fn server_with(lmutil_path: &str) -> LicenseServerConfig {
    LicenseServerConfig {
        name: "demo-server".to_owned(),
        host: "127.0.0.1".to_owned(),
        port: 27000,
        lmutil_path: lmutil_path.to_owned(),
        vendor_port: None,
    }
}

#[tokio::test]
async fn missing_lmutil_binary_returns_actionable_error() {
    let binary = if cfg!(windows) {
        "definitely-not-a-real-lmutil.exe"
    } else {
        "definitely-not-a-real-lmutil"
    };

    let status = collect_server(&server_with(binary), Duration::from_secs(5), 3).await;

    assert_eq!(status.server_name, "demo-server");
    assert_eq!(status.server_host, "127.0.0.1");
    assert_eq!(status.lmgrd_port, 27000);
    assert_eq!(status.status, "DOWN");
    assert!(!status.is_up());
    assert!(
        status.parse_error.contains("找不到 lmutil"),
        "parse_error 应提示 lmutil 缺失，实际: {}",
        status.parse_error
    );
    assert!(status.features.is_empty());
    // 状态对象即使采集失败也应可直接用于指标上报
    assert_eq!(status.total_licenses(), 0);
}
