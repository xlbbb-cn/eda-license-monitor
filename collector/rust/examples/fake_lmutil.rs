//! 测试用假 `lmutil`：打印内置的 `lmstat -a` 样例输出。
//!
//! 用于在没有 FlexLM 环境时验证采集器全链路
//! （子进程调用 → 输出解析 → Prometheus 指标 → HTTP 端点）。
//!
//! ```text
//! cargo build --examples
//! cargo run --example fake_lmutil -- full    # 正常输出（默认）
//! cargo run --example fake_lmutil -- table   # 旧版表格 + 简写格式
//! cargo run --example fake_lmutil -- error   # Server 不可达
//! ```
//!
//! 采集器调用时参数固定为 `lmstat -a -c <port>@<host>`，因此也可以按 host 名切换输出：
//!
//! ```yaml
//! license_servers:
//!   - name: demo-server           # host 名含 "down" / "error" → 输出无法连接
//!     host: down-host             # host 名含 "table"        → 输出旧版表格格式
//!     lmutil_path: "collector/rust/target/debug/examples/fake_lmutil"
//! ```

use std::process::ExitCode;

const FULL: &str = include_str!("../tests/fixtures/lmstat_full.txt");
const TABLE: &str = include_str!("../tests/fixtures/lmstat_table.txt");
const ERROR: &str = include_str!("../tests/fixtures/lmstat_connect_error.txt");

fn main() -> ExitCode {
    let selector = std::env::args().skip(1).collect::<Vec<_>>().join(" ");

    let variant = if selector.contains("down") || selector.contains("error") {
        "error"
    } else if selector.contains("table") {
        "table"
    } else {
        "full"
    };

    let (output, exit_code) = match variant {
        "error" => (ERROR, 1),
        "table" => (TABLE, 0),
        _ => (FULL, 0),
    };

    print!("{output}");
    ExitCode::from(exit_code)
}
