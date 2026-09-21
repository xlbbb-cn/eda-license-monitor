//! FlexLM License 监控采集器（Rust 版）
//!
//! 与 `collector/python` 功能等价的 Rust 实现，对外暴露的 Prometheus 指标
//! 名称、标签与 Python 版完全一致，可直接复用现有的 Prometheus / Grafana 配置。
//!
//! - [`lmutil`]：解析 `lmutil lmstat -a` 输出、执行采集
//! - [`metrics`]：Prometheus 指标定义
//! - [`collector`]：定时采集循环
//! - [`config`]：YAML 配置加载

pub mod collector;
pub mod config;
pub mod lmutil;
pub mod metrics;

pub use collector::LicenseCollector;
pub use config::Config;
pub use lmutil::{collect_server, parse_lmstat_output, FeatureUsage, ServerStatus, UserUsage};
pub use metrics::Metrics;
