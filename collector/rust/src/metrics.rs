//! Prometheus 指标定义（对应 Python 版 `prometheus_exporter.py` 的 Gauge / Counter）。
//!
//! 指标名、HELP 文本与标签与 Python 版完全一致，现有 Grafana 仪表盘
//! 与告警规则无需任何改动：
//!
//! | 指标 | 类型 | 标签 |
//! |------|------|------|
//! | `flexlm_feature_total` | Gauge | server, feature, vendor |
//! | `flexlm_feature_used` | Gauge | server, feature, vendor |
//! | `flexlm_feature_idle` | Gauge | server, feature, vendor |
//! | `flexlm_feature_users` | Gauge | server, feature |
//! | `flexlm_server_status` | Gauge | server, host, port |
//! | `flexlm_server_features_total` | Gauge | server |
//! | `flexlm_scrape_duration_seconds` | Gauge | server |
//! | `flexlm_scrape_total` | Counter | server, status |

use anyhow::Result;
use prometheus::{Encoder, GaugeVec, IntCounterVec, IntGaugeVec, Opts, Registry, TextEncoder};

/// `/metrics` 的 Content-Type（与 prometheus_client 一致）
pub const METRICS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// 采集器暴露的全部 Prometheus 指标（使用独立 Registry，等价于 Python 版的
/// `CollectorRegistry(auto_describe=True)`，因此 /metrics 中只有 flexlm_* 指标）
pub struct Metrics {
    registry: Registry,

    // Feature 级指标
    pub(crate) feature_total: IntGaugeVec,
    pub(crate) feature_used: IntGaugeVec,
    pub(crate) feature_idle: IntGaugeVec,
    pub(crate) feature_users: IntGaugeVec,

    // Server 级指标
    pub(crate) server_status: IntGaugeVec,
    pub(crate) server_features_total: IntGaugeVec,

    // 采集过程指标
    pub(crate) scrape_duration: GaugeVec,
    pub(crate) scrape_total: IntCounterVec,
}

impl Metrics {
    /// 创建并注册所有指标
    pub fn new() -> Result<Self> {
        let metrics = Self {
            registry: Registry::new(),
            feature_total: IntGaugeVec::new(
                Opts::new(
                    "flexlm_feature_total",
                    "Total licenses issued for this feature",
                ),
                &["server", "feature", "vendor"],
            )?,
            feature_used: IntGaugeVec::new(
                Opts::new(
                    "flexlm_feature_used",
                    "Licenses currently in use for this feature",
                ),
                &["server", "feature", "vendor"],
            )?,
            feature_idle: IntGaugeVec::new(
                Opts::new(
                    "flexlm_feature_idle",
                    "Licenses idle (total - used) for this feature",
                ),
                &["server", "feature", "vendor"],
            )?,
            feature_users: IntGaugeVec::new(
                Opts::new(
                    "flexlm_feature_users",
                    "Number of distinct users holding this feature",
                ),
                &["server", "feature"],
            )?,
            server_status: IntGaugeVec::new(
                Opts::new(
                    "flexlm_server_status",
                    "License server reachability (1=UP, 0=DOWN/UNKNOWN)",
                ),
                &["server", "host", "port"],
            )?,
            server_features_total: IntGaugeVec::new(
                Opts::new(
                    "flexlm_server_features_total",
                    "Total number of monitored features on this server",
                ),
                &["server"],
            )?,
            scrape_duration: GaugeVec::new(
                Opts::new(
                    "flexlm_scrape_duration_seconds",
                    "Time spent collecting license data from lmutil",
                ),
                &["server"],
            )?,
            scrape_total: IntCounterVec::new(
                Opts::new("flexlm_scrape_total", "Total number of scrape attempts"),
                &["server", "status"],
            )?,
        };

        metrics.register_all()?;
        Ok(metrics)
    }

    fn register_all(&self) -> Result<()> {
        self.registry
            .register(Box::new(self.feature_total.clone()))?;
        self.registry
            .register(Box::new(self.feature_used.clone()))?;
        self.registry
            .register(Box::new(self.feature_idle.clone()))?;
        self.registry
            .register(Box::new(self.feature_users.clone()))?;
        self.registry
            .register(Box::new(self.server_status.clone()))?;
        self.registry
            .register(Box::new(self.server_features_total.clone()))?;
        self.registry
            .register(Box::new(self.scrape_duration.clone()))?;
        self.registry
            .register(Box::new(self.scrape_total.clone()))?;
        Ok(())
    }

    /// 渲染为 Prometheus 文本格式（等价于 prometheus_client 的 `generate_latest`）
    pub fn render(&self) -> String {
        let encoder = TextEncoder::new();
        // prometheus 0.13 的 Encoder 要求 `std::io::Write`，因此先写入字节缓冲
        let mut buffer = Vec::new();
        if let Err(err) = encoder.encode(&self.registry.gather(), &mut buffer) {
            tracing::error!("渲染 Prometheus 指标失败: {err}");
            return String::new();
        }
        String::from_utf8_lossy(&buffer).into_owned()
    }

    /// 底层 Registry（如需自定义渲染或单测时使用）
    pub fn registry(&self) -> &Registry {
        &self.registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_expected_metric_families() {
        let metrics = Metrics::new().unwrap();
        metrics
            .feature_total
            .with_label_values(&["srv", "vcs", "snpslmd"])
            .set(20);
        metrics
            .feature_used
            .with_label_values(&["srv", "vcs", "snpslmd"])
            .set(12);
        metrics
            .feature_idle
            .with_label_values(&["srv", "vcs", "snpslmd"])
            .set(8);
        metrics
            .feature_users
            .with_label_values(&["srv", "vcs"])
            .set(3);
        metrics
            .server_status
            .with_label_values(&["srv", "10.0.0.1", "27000"])
            .set(1);
        metrics
            .server_features_total
            .with_label_values(&["srv"])
            .set(1);
        metrics
            .scrape_duration
            .with_label_values(&["srv"])
            .set(0.123);
        metrics
            .scrape_total
            .with_label_values(&["srv", "success"])
            .inc();

        let rendered = metrics.render();
        assert!(rendered.contains(
            "flexlm_feature_total{feature=\"vcs\",server=\"srv\",vendor=\"snpslmd\"} 20"
        ));
        assert!(rendered
            .contains("flexlm_feature_used{feature=\"vcs\",server=\"srv\",vendor=\"snpslmd\"} 12"));
        assert!(rendered
            .contains("flexlm_feature_idle{feature=\"vcs\",server=\"srv\",vendor=\"snpslmd\"} 8"));
        assert!(rendered.contains("flexlm_feature_users{feature=\"vcs\",server=\"srv\"} 3"));
        assert!(rendered
            .contains("flexlm_server_status{host=\"10.0.0.1\",port=\"27000\",server=\"srv\"} 1"));
        assert!(rendered.contains("flexlm_server_features_total{server=\"srv\"} 1"));
        assert!(rendered.contains("flexlm_scrape_duration_seconds{server=\"srv\"} 0.123"));
        assert!(rendered.contains("flexlm_scrape_total{server=\"srv\",status=\"success\"} 1"));
        assert!(rendered.contains("# TYPE flexlm_feature_total gauge"));
        assert!(rendered.contains("# TYPE flexlm_scrape_total counter"));
    }

    #[test]
    fn empty_metrics_render_without_panic() {
        let metrics = Metrics::new().unwrap();
        let rendered = metrics.render();
        // 与 prometheus_client 行为一致：没有任何样本时不输出指标族，
        // 因此 Grafana 显示 "No data" 而不是 0
        assert!(rendered.is_empty());
        assert_eq!(metrics.registry().gather().len(), 0);
    }
}
