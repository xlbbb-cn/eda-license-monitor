//! FlexLM `lmutil lmstat -a` 输出解析与采集执行
//! （Python 版 `lmutil_parser.py` 的 Rust 移植）。
//!
//! 兼容 FlexLM 9.x / 11.x / 11.16.x 等主流版本输出格式。
//! 正则表达式与 Python 版逐条对应，保证解析结果一致；
//! 额外补充了两处 Python 版未覆盖的现代输出格式，见 [`parse_lmstat_output`] 的说明。

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::Duration;

use chrono::Local;
use regex::Regex;
use tokio::process::Command;

use crate::config::LicenseServerConfig;

// ---------------------------------------------------------------------------
// 数据结构
// ---------------------------------------------------------------------------

/// 单个用户的占用情况
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserUsage {
    pub username: String,
    pub host: String,
    pub display: String,
    pub start_time: String,
    pub feature: String,
}

/// 单个 Feature 的使用情况
#[derive(Debug, Clone, Default)]
pub struct FeatureUsage {
    pub name: String,
    pub vendor: String,
    pub total: i64,
    pub used: i64,
    pub users: Vec<UserUsage>,
}

impl FeatureUsage {
    /// 空闲 License 数（total - used）
    pub fn idle(&self) -> i64 {
        self.total - self.used
    }

    /// 使用率（0.0 - 1.0，total 为 0 时返回 0.0）
    pub fn usage_ratio(&self) -> f64 {
        if self.total > 0 {
            self.used as f64 / self.total as f64
        } else {
            0.0
        }
    }
}

/// License Server 整体状态
#[derive(Debug, Clone, Default)]
pub struct ServerStatus {
    /// 配置中的 Server 名称（采集时填充）
    pub server_name: String,
    /// License Server 主机（采集时使用配置值）
    pub server_host: String,
    /// lmgrd 端口（采集时使用配置值）
    pub lmgrd_port: u16,
    /// vendor daemon 名称，如 `snpslmd`
    pub vendor_daemon: String,
    /// `UP` / `DOWN` / `UNSUPPORTED`，解析不到时为 `""`
    pub status: String,
    pub features: Vec<FeatureUsage>,
    pub users: Vec<UserUsage>,
    /// 采集时间（ISO-8601）
    pub parse_time: String,
    /// 非空表示本次采集无效（Server 不可达 / 输出无法解析）
    pub parse_error: String,
    /// lmutil 明确报告 Server 不可达（如 `Cannot connect to license server`），
    /// 此时重试没有意义
    pub unreachable: bool,
}

impl ServerStatus {
    /// 所有 Feature 的 License 总数
    pub fn total_licenses(&self) -> i64 {
        self.features.iter().map(|feature| feature.total).sum()
    }

    /// 所有 Feature 的已用 License 数
    pub fn total_used(&self) -> i64 {
        self.features.iter().map(|feature| feature.used).sum()
    }

    /// 整体使用率（0.0 - 1.0）
    pub fn overall_usage_ratio(&self) -> f64 {
        let total = self.total_licenses();
        if total > 0 {
            self.total_used() as f64 / total as f64
        } else {
            0.0
        }
    }

    /// 是否可用：`UP`，或未解析到 vendor daemon 状态（与 Python 版一致，
    /// 避免老版本输出格式因缺少 vendor daemon 行而误判为 DOWN）
    pub fn is_up(&self) -> bool {
        self.status.is_empty() || self.status.eq_ignore_ascii_case("UP")
    }
}

// ---------------------------------------------------------------------------
// 正则表达式（顺序需与 Python 版一致，匹配优先级有意义）
// ---------------------------------------------------------------------------

/// `License server status: 27000@host` / `License Server status: 27000 on host`
static RE_SERVER_STATUS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)license\s+server\s+status:\s*(\d+)\s*(?:@|on)\s*(\S+)")
        .expect("RE_SERVER_STATUS 正则非法")
});

/// `vendor daemon status: 27001 host: UP` / `Vendor daemon status: 27001@host: UP`
static RE_VENDOR_DAEMON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)vendor\s+daemon\s+status:\s*(\d+)\s+(\S+):\s*(\S+)")
        .expect("RE_VENDOR_DAEMON 正则非法")
});

/// `   snpslmd: UP v11.19.6`（现代 `lmstat -a` 的 vendor daemon 行）
static RE_VENDOR_DAEMON_SIMPLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s+(\S+):\s+(UP|DOWN|UNSUPPORTED)\b")
        .expect("RE_VENDOR_DAEMON_SIMPLE 正则非法")
});

/// `  vcs    2023.06 synopsys 01-jan-00 20     12    `
static RE_FEATURE_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s+(\S+)\s+(?:v?[\d.]+\s+)?(\S+)\s+(?:\d+-[\w]+-\d+|lifetime)\s+(\d+)\s+(\d+)(?:\s+\d+)?",
    )
    .expect("RE_FEATURE_LINE 正则非法")
});

/// `  username hostname display start_time [feature]`
static RE_USER_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s+(\S+)\s+(\S+:\d+|\S+)(?:\s+(\S+))?\s+(\d{2}/\d{2}/\d{4}\s+\d{2}:\d{2})(?:\s+\((\S+)\))?",
    )
    .expect("RE_USER_LINE 正则非法")
});

/// `Users of FEATURE:  (Total of 20 licenses issued;  Total of 12 licenses in use)`
static RE_FEATURE_USERS_OF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"Users of (\S+?):\s+\(Total of (\d+) licenses issued;\s+Total of (\d+) licenses in use\)",
    )
    .expect("RE_FEATURE_USERS_OF 正则非法")
});

/// 备用简写格式：`FEATURE: 20 licenses, 12 in use`
static RE_FEATURE_BRIEF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\S+?):\s+(\d+)\s+licen[cs]es?,\s+(\d+)\s+(?:in use|used)")
        .expect("RE_FEATURE_BRIEF 正则非法")
});

/// lmutil 明显的失败信息（Server 不可达 / 授权文件配置错误）
const FAILURE_MARKERS: [&str; 6] = [
    "Cannot connect to license server",
    "License server machine is down",
    "Error getting status",
    "lmgrd is not running",
    "Invalid license file syntax",
    "Failed to open the TCP port",
];

// ---------------------------------------------------------------------------
// 解析
// ---------------------------------------------------------------------------

/// Feature 表：同时维护插入顺序（与 Python 有序 dict 一致）和名称索引
#[derive(Default)]
struct FeatureTable {
    features: Vec<FeatureUsage>,
    index: HashMap<String, usize>,
}

impl FeatureTable {
    /// 取得（必要时创建）指定 Feature 的条目
    fn entry(&mut self, name: &str) -> &mut FeatureUsage {
        let position = match self.index.get(name).copied() {
            Some(position) => position,
            None => {
                let position = self.features.len();
                self.features.push(FeatureUsage {
                    name: name.to_owned(),
                    ..Default::default()
                });
                self.index.insert(name.to_owned(), position);
                position
            }
        };
        &mut self.features[position]
    }

    fn into_vec(self) -> Vec<FeatureUsage> {
        self.features
    }
}

/// 解析 `lmutil lmstat -a` 的完整输出文本。
///
/// 与 Python 版相比，额外识别两类现代输出（均只在 Python 版会漏判时生效）：
/// - `License server status: 27000@host`（Python 版正则只认 `... on host`，且大小写敏感）
/// - `   snpslmd: UP v11.19.6`（Python 版只认带端口号的 vendor daemon 行）
///
/// 另外当输出中没有任何 Feature/用户数据、且出现明确的失败信息
/// （如 `Error getting status: Cannot connect to license server system`）时，
/// 会置 `status = DOWN` 并填写 `parse_error`，使 `flexlm_server_status` 指标正确反映
/// Server 不可达（Python 版此时会错误地报告为 UP）。
pub fn parse_lmstat_output(output: &str) -> ServerStatus {
    let mut status = ServerStatus {
        parse_time: now_iso8601(),
        ..Default::default()
    };
    let mut table = FeatureTable::default();
    let mut current_feature: Option<String> = None;

    for line in output.trim().lines() {
        // --- License Server status ---
        if let Some(captures) = RE_SERVER_STATUS.captures(line) {
            if let Ok(port) = captures[1].parse::<u16>() {
                status.lmgrd_port = port;
            }
            status.server_host = captures[2].to_owned();
            continue;
        }

        // --- Vendor daemon（带端口号的格式） ---
        if let Some(captures) = RE_VENDOR_DAEMON.captures(line) {
            status.vendor_daemon = captures[2].to_owned();
            status.status = captures[3].to_owned();
            continue;
        }

        // --- Vendor daemon（现代格式：`snpslmd: UP v11.19.6`） ---
        if status.vendor_daemon.is_empty() {
            if let Some(captures) = RE_VENDOR_DAEMON_SIMPLE.captures(line) {
                status.vendor_daemon = captures[1].to_owned();
                status.status = captures[2].to_uppercase();
                continue;
            }
        }

        // --- Feature 汇总行：`Users of FEATURE: (Total of N ...)` ---
        if let Some(captures) = RE_FEATURE_USERS_OF.captures(line) {
            let name = captures[1].to_owned();
            let total = parse_int(&captures[2]);
            let used = parse_int(&captures[3]);
            let feature = table.entry(&name);
            feature.total = total;
            feature.used = used;
            current_feature = Some(name);
            continue;
        }

        // --- Feature 简写行 ---
        if let Some(captures) = RE_FEATURE_BRIEF.captures(line) {
            let name = captures[1].to_owned();
            let total = parse_int(&captures[2]);
            let used = parse_int(&captures[3]);
            let feature = table.entry(&name);
            feature.total = total;
            feature.used = used;
            continue;
        }

        // --- Feature 明细行 ---
        if let Some(captures) = RE_FEATURE_LINE.captures(line) {
            let name = captures[1].to_owned();
            let vendor = captures[2].to_owned();
            let total = parse_int(&captures[3]);
            let used = parse_int(&captures[4]);
            let feature = table.entry(&name);
            feature.vendor = vendor;
            feature.total = total;
            feature.used = used;
            current_feature = Some(name);
            continue;
        }

        // --- 用户行 ---
        if let Some(captures) = RE_USER_LINE.captures(line) {
            let user = UserUsage {
                username: captures[1].to_owned(),
                host: captures[2].to_owned(),
                display: captures
                    .get(3)
                    .map(|matched| matched.as_str().to_owned())
                    .unwrap_or_default(),
                start_time: captures
                    .get(4)
                    .map(|matched| matched.as_str().to_owned())
                    .unwrap_or_default(),
                feature: captures
                    .get(5)
                    .map(|matched| matched.as_str().to_owned())
                    .or_else(|| current_feature.clone())
                    .unwrap_or_default(),
            };

            if let Some(name) = &current_feature {
                table.entry(name).users.push(user.clone());
            }
            status.users.push(user);
        }
    }

    status.features = table.into_vec();
    detect_failure(&mut status, output);
    status
}

/// 输出中没有任何有效数据时，用明确的失败信息判定 Server 不可达
fn detect_failure(status: &mut ServerStatus, output: &str) {
    if !status.status.is_empty() || !status.features.is_empty() || !status.users.is_empty() {
        return;
    }

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if FAILURE_MARKERS
            .iter()
            .any(|marker| trimmed.contains(marker))
        {
            status.status = "DOWN".to_owned();
            status.parse_error = format!("lmutil 采集失败: {trimmed}");
            status.unreachable = true;
            return;
        }
    }
}

fn parse_int(value: &str) -> i64 {
    value.parse().unwrap_or(0)
}

fn now_iso8601() -> String {
    Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false)
}

// ---------------------------------------------------------------------------
// lmutil 执行
// ---------------------------------------------------------------------------

/// lmutil 执行失败原因
#[derive(Debug)]
pub enum LmutilError {
    /// 找不到可执行文件
    NotFound,
    /// 执行超时
    Timeout,
    /// 进程已执行但没有任何有效输出（退出码非 0 或 stdout 为空），附带 stderr 信息
    Failed {
        exit_code: Option<i32>,
        stderr: String,
    },
    /// 其他 IO 错误
    Io(std::io::Error),
}

impl std::fmt::Display for LmutilError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(formatter, "找不到 lmutil 可执行文件"),
            Self::Timeout => write!(formatter, "lmutil 执行超时"),
            Self::Failed { exit_code, stderr } => {
                if stderr.is_empty() {
                    write!(formatter, "lmutil 无输出，退出码 {exit_code:?}")
                } else {
                    write!(
                        formatter,
                        "lmutil 无输出，退出码 {exit_code:?}，stderr: {stderr}"
                    )
                }
            }
            Self::Io(err) => write!(formatter, "{err}"),
        }
    }
}

impl std::error::Error for LmutilError {}

/// 执行 `lmutil lmstat -a -c <port>@<host>` 并返回标准输出文本。
///
/// 与 Python 版一致：lmutil 有时返回非零退出码但 stdout 仍有有效内容，
/// 因此只检查 stdout 是否为空，不检查退出码。
pub async fn run_lmutil(
    host: &str,
    port: u16,
    lmutil_path: &str,
    timeout: Duration,
) -> Result<String, LmutilError> {
    let program = if lmutil_path.is_empty() {
        "lmutil"
    } else {
        lmutil_path
    };
    let target = format!("{port}@{host}");

    tracing::info!("执行: {program} lmstat -a -c {target}");

    let mut command = Command::new(program);
    command
        .args(["lmstat", "-a", "-c", &target])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // 超时后 future 被 drop，配合 kill_on_drop 直接终止卡死的 lmutil 进程
        .kill_on_drop(true);

    let child = command.spawn().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            LmutilError::NotFound
        } else {
            LmutilError::Io(err)
        }
    })?;

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => return Err(LmutilError::Io(err)),
        Err(_) => return Err(LmutilError::Timeout),
    };

    // lmutil 输出为本地编码（Windows 上常见 GBK），按 UTF-8 宽松解码避免采集直接失败
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !stdout.trim().is_empty() {
        return Ok(stdout);
    }

    // 没有 stdout：把退出码与 stderr 摘要作为失败原因返回，便于定位
    // （例如容器内未挂载 FlexLM 客户端、lmutil 缺少依赖库、参数不兼容）
    Err(LmutilError::Failed {
        exit_code: output.status.code(),
        stderr: truncate(String::from_utf8_lossy(&output.stderr).trim(), 500),
    })
}

/// 采集单台 License Server 的使用数据，带重试。
///
/// - 失败时返回 `status: DOWN` 的状态对象，不返回 `Err`
/// - `server_host` / `lmgrd_port` 统一使用配置文件中的值（与 Python 版一致）
pub async fn collect_server(
    server: &LicenseServerConfig,
    timeout: Duration,
    retries: u32,
) -> ServerStatus {
    let mut last_error = ServerStatus {
        server_name: server.name.clone(),
        server_host: server.host.clone(),
        lmgrd_port: server.port,
        status: "DOWN".to_owned(),
        parse_error: "采集失败".to_owned(),
        parse_time: now_iso8601(),
        ..Default::default()
    };

    let attempts = retries.max(1);
    for attempt in 1..=attempts {
        match run_lmutil(&server.host, server.port, &server.lmutil_path, timeout).await {
            Ok(output) if !output.trim().is_empty() => {
                let mut status = parse_lmstat_output(&output);
                status.server_name = server.name.clone();
                status.server_host = server.host.clone();
                status.lmgrd_port = server.port;
                if status.parse_error.is_empty() {
                    return status;
                }

                // Server 明确不可达时重试没有意义，直接返回避免无谓的进程调用
                let unreachable = status.unreachable;
                last_error = status;
                if unreachable {
                    tracing::warn!("[{}] 采集失败: {}", server.name, last_error.parse_error);
                    return last_error;
                }
            }
            Ok(_) => {}
            Err(LmutilError::NotFound) => {
                tracing::error!(
                    "找不到 lmutil，请确认已安装 FlexLM 或在 config.yaml 中配置 lmutil_path"
                );
                last_error.parse_error = format!(
                    "找不到 lmutil 可执行文件: {program}",
                    program = if server.lmutil_path.is_empty() {
                        "lmutil"
                    } else {
                        &server.lmutil_path
                    }
                );
                // 二进制缺失属于配置问题，重试没有意义，立即返回
                return last_error;
            }
            Err(LmutilError::Timeout) => {
                tracing::error!(
                    "lmutil 执行超时（{}s），License Server 可能不可达",
                    timeout.as_secs()
                );
                last_error.parse_error = format!("lmutil 执行超时（{}s）", timeout.as_secs());
            }
            Err(err @ LmutilError::Failed { .. }) => {
                tracing::error!("lmutil 执行异常: {err}");
                last_error.parse_error = format!("lmutil 执行异常: {err}");
            }
            Err(LmutilError::Io(err)) => {
                tracing::error!("lmutil 执行异常: {err}");
                last_error.parse_error = format!("lmutil 执行异常: {err}");
            }
        }

        tracing::warn!(
            "第 {attempt}/{attempts} 次采集失败 (host={}:{})",
            server.host,
            server.port
        );
    }

    last_error
}

/// 截断日志文本（按字符数，避免多字节字符被截断）
fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_usage_helpers() {
        let feature = FeatureUsage {
            total: 20,
            used: 12,
            ..Default::default()
        };
        assert_eq!(feature.idle(), 8);
        assert!((feature.usage_ratio() - 0.6).abs() < f64::EPSILON);

        let empty = FeatureUsage::default();
        assert_eq!(empty.idle(), 0);
        assert_eq!(empty.usage_ratio(), 0.0);
    }

    #[test]
    fn status_helpers() {
        let mut status = ServerStatus {
            status: "UP".to_owned(),
            features: vec![
                FeatureUsage {
                    total: 10,
                    used: 4,
                    ..Default::default()
                },
                FeatureUsage {
                    total: 6,
                    used: 2,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        assert!(status.is_up());
        assert_eq!(status.total_licenses(), 16);
        assert_eq!(status.total_used(), 6);
        assert!((status.overall_usage_ratio() - 0.375).abs() < f64::EPSILON);

        status.status = "DOWN".to_owned();
        assert!(!status.is_up());

        // 与 Python 版一致：未解析到 vendor daemon 状态时视为 UP
        status.status.clear();
        assert!(status.is_up());
    }

    #[test]
    fn lmutil_error_messages_are_actionable() {
        assert_eq!(
            LmutilError::NotFound.to_string(),
            "找不到 lmutil 可执行文件"
        );
        assert_eq!(LmutilError::Timeout.to_string(), "lmutil 执行超时");

        let failed = LmutilError::Failed {
            exit_code: Some(1),
            stderr: "ERROR: lmutil not found.".to_owned(),
        };
        let message = failed.to_string();
        assert!(message.contains("退出码 Some(1)"));
        assert!(message.contains("ERROR: lmutil not found."));

        let silent = LmutilError::Failed {
            exit_code: None,
            stderr: String::new(),
        };
        assert!(silent.to_string().contains("lmutil 无输出"));

        assert_eq!(
            LmutilError::Io(std::io::Error::other("boom")).to_string(),
            "boom"
        );
    }
}
