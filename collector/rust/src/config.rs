//! 配置加载（对应 Python 版 `prometheus_exporter.load_config()`）。
//!
//! local 覆盖文件的查找顺序：
//! 1. `config/config.yaml.local` —— Python 版 `Path.with_suffix(".yaml.local")` 的实际结果
//! 2. `config/config.local.yaml` —— README / quickstart 文档中记载的文件名
//!
//! 两者都不存在时使用 `-c/--config` 指定的文件本身。
//! 文件建议保存为 UTF-8；带 BOM（记事本 / PowerShell `Set-Content`）或包含个别
//! 非 UTF-8 字节的文件也能读取（PyYAML 同样容忍 BOM）。

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

fn default_lmgrd_port() -> u16 {
    27000
}

fn default_interval() -> u64 {
    30
}

fn default_timeout() -> u64 {
    15
}

fn default_retries() -> u32 {
    3
}

fn default_metrics_port() -> u16 {
    9091
}

/// 单台 License Server 配置
#[derive(Debug, Clone, Deserialize)]
pub struct LicenseServerConfig {
    /// 展示名，同时作为 Prometheus 的 `server` 标签
    pub name: String,
    /// License Server 主机名 / IP
    pub host: String,
    /// lmgrd 端口，默认 27000
    #[serde(default = "default_lmgrd_port")]
    pub port: u16,
    /// lmutil 可执行文件路径，为空时从 `PATH` 查找
    #[serde(default)]
    pub lmutil_path: String,
    /// 预留字段：vendor daemon 端口（部分 FlexLM 配置需要显式指定）
    #[serde(default)]
    pub vendor_port: Option<u16>,
}

/// `collector:` 段
#[derive(Debug, Clone, Deserialize)]
pub struct CollectorSection {
    /// 采集间隔（秒）
    #[serde(default = "default_interval")]
    pub interval: u64,
    /// lmutil 命令超时时间（秒）
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    /// 失败重试次数
    #[serde(default = "default_retries")]
    pub retries: u32,
}

impl Default for CollectorSection {
    fn default() -> Self {
        Self {
            interval: default_interval(),
            timeout: default_timeout(),
            retries: default_retries(),
        }
    }
}

/// `prometheus:` 段（只使用采集器需要的字段，其余字段如 `scrape_interval` 由 Prometheus 读取）
#[derive(Debug, Clone, Deserialize)]
pub struct PrometheusSection {
    /// 指标暴露端口
    #[serde(default = "default_metrics_port")]
    pub port: u16,
}

impl Default for PrometheusSection {
    fn default() -> Self {
        Self {
            port: default_metrics_port(),
        }
    }
}

/// 主配置（未识别的字段会被忽略，如 `alerts` 段由 AlertManager 使用）
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub license_servers: Vec<LicenseServerConfig>,
    #[serde(default)]
    pub collector: CollectorSection,
    #[serde(default)]
    pub prometheus: PrometheusSection,
}

impl Config {
    /// 采集间隔（至少 1 秒，避免配置为 0 时忙轮询）
    pub fn interval(&self) -> Duration {
        Duration::from_secs(self.collector.interval.max(1))
    }

    /// lmutil 执行超时（至少 1 秒）
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.collector.timeout.max(1))
    }

    /// 重试次数（至少 1 次）
    pub fn retries(&self) -> u32 {
        self.collector.retries.max(1)
    }
}

/// 计算实际加载的配置文件路径（支持 local 覆盖）。
pub fn resolve_config_path(requested: &Path) -> PathBuf {
    if let Some(file_name) = requested.file_name() {
        let appended = requested.with_file_name(format!("{}.local", file_name.to_string_lossy()));
        if appended.is_file() {
            return appended;
        }
    }

    if let (Some(stem), Some(extension)) = (requested.file_stem(), requested.extension()) {
        let dotted = requested.with_file_name(format!(
            "{}.local.{}",
            stem.to_string_lossy(),
            extension.to_string_lossy()
        ));
        if dotted.is_file() {
            return dotted;
        }
    }

    requested.to_path_buf()
}

/// 读取并解析 YAML 配置
pub fn load(requested: &Path) -> Result<Config> {
    let path = resolve_config_path(requested);
    if path != requested {
        tracing::info!("加载本地覆盖配置: {}", path.display());
    }

    let bytes =
        std::fs::read(&path).with_context(|| format!("读取配置文件失败: {}", path.display()))?;

    // 去掉 UTF-8 BOM：serde_yaml 无法解析带 BOM 的文档
    let bytes = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        &bytes[..]
    };

    let text: Cow<'_, str> = match std::str::from_utf8(bytes) {
        Ok(text) => Cow::Borrowed(text),
        Err(err) => {
            tracing::warn!(
                "配置文件不是合法 UTF-8（{err}），已忽略非法字节，建议另存为 UTF-8 编码"
            );
            Cow::Owned(String::from_utf8_lossy(bytes).into_owned())
        }
    };

    let config: Config = serde_yaml::from_str(&text)
        .with_context(|| format!("解析 YAML 配置失败: {}", path.display()))?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_appended_local_override_first() {
        let dir = std::env::temp_dir().join("eda-license-config-test-appended");
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("config.yaml");
        let appended = dir.join("config.yaml.local");
        std::fs::write(&appended, "license_servers: []\n").unwrap();

        assert_eq!(resolve_config_path(&base), appended);
    }

    #[test]
    fn resolves_dotted_local_override() {
        let dir = std::env::temp_dir().join("eda-license-config-test-dotted");
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("config.yaml");
        let dotted = dir.join("config.local.yaml");
        std::fs::write(&dotted, "license_servers: []\n").unwrap();
        let appended = dir.join("config.yaml.local");
        let _ = std::fs::remove_file(&appended);

        assert_eq!(resolve_config_path(&base), dotted);
    }

    #[test]
    fn keeps_requested_path_without_override() {
        let dir = std::env::temp_dir().join("eda-license-config-test-plain");
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("config.yaml");
        let _ = std::fs::remove_file(dir.join("config.yaml.local"));
        let _ = std::fs::remove_file(dir.join("config.local.yaml"));

        assert_eq!(resolve_config_path(&base), base);
    }

    #[test]
    fn parses_project_config_shape() {
        let yaml = r#"
prometheus:
  port: 9091
  scrape_interval: 30s
license_servers:
  - name: "main-server"
    host: "192.168.1.100"
    port: 27000
    lmutil_path: ""
collector:
  interval: 30
  timeout: 15
  retries: 3
alerts:
  enabled: true
  high_usage_threshold: 0.90
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.prometheus.port, 9091);
        assert_eq!(config.license_servers.len(), 1);
        assert_eq!(config.license_servers[0].name, "main-server");
        assert_eq!(config.license_servers[0].port, 27000);
        assert_eq!(config.interval(), Duration::from_secs(30));
        assert_eq!(config.retries(), 3);
    }

    #[test]
    fn applies_defaults_when_sections_missing() {
        let yaml = "license_servers:\n  - name: srv\n    host: 10.0.0.1\n";
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.license_servers[0].port, 27000);
        assert_eq!(config.prometheus.port, 9091);
        assert_eq!(config.timeout(), Duration::from_secs(15));
    }

    #[test]
    fn loads_config_with_utf8_bom_and_chinese_comments() {
        let dir = std::env::temp_dir().join("eda-license-config-test-bom");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        let _ = std::fs::remove_file(dir.join("config.yaml.local"));
        let _ = std::fs::remove_file(dir.join("config.local.yaml"));

        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(
            "# 中文注释\nlicense_servers:\n  - name: \"主服务器\"\n    host: 10.0.0.1\n".as_bytes(),
        );
        std::fs::write(&path, bytes).unwrap();

        let config = load(&path).unwrap();
        assert_eq!(config.license_servers[0].name, "主服务器");
        assert_eq!(config.license_servers[0].port, 27000);
    }
}
