//! 定时采集循环（对应 Python 版 `LicenseCollector`）。

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio::sync::watch;

use crate::config::{Config, LicenseServerConfig};
use crate::lmutil::{self, ServerStatus};
use crate::metrics::Metrics;

/// 采集器：串行采集多台 License Server，并把结果写入 Prometheus 指标
pub struct LicenseCollector {
    servers: Vec<LicenseServerConfig>,
    interval: Duration,
    timeout: Duration,
    retries: u32,
    metrics: Arc<Metrics>,
    /// 最近一次采集结果，按配置顺序保存
    last_status: Mutex<Vec<(String, ServerStatus)>>,
}

impl LicenseCollector {
    pub fn new(config: &Config, metrics: Arc<Metrics>) -> Self {
        Self {
            servers: config.license_servers.clone(),
            interval: config.interval(),
            timeout: config.timeout(),
            retries: config.retries(),
            metrics,
            last_status: Mutex::new(Vec::new()),
        }
    }

    /// 监控的 License Server 数量
    pub fn server_count(&self) -> usize {
        self.servers.len()
    }

    /// 执行一次全量采集（逐台 Server 串行执行，避免并发冲击 License Server）
    pub async fn collect_once(&self) {
        for server in &self.servers {
            let started = Instant::now();
            let status = lmutil::collect_server(server, self.timeout, self.retries).await;
            let duration = started.elapsed().as_secs_f64();

            self.update_metrics(server, &status, duration);
            self.store_status(server, status);
        }
    }

    /// 写入 Prometheus 指标（对应 Python 版 `_update_metrics()`）
    fn update_metrics(&self, server: &LicenseServerConfig, status: &ServerStatus, duration: f64) {
        // 与 Python 版一致：标签使用配置中的 host/port
        let host = if status.server_host.is_empty() {
            server.host.as_str()
        } else {
            status.server_host.as_str()
        };
        let port = if status.lmgrd_port == 0 {
            server.port
        } else {
            status.lmgrd_port
        };
        let port_label = port.to_string();

        let is_up = status.is_up();
        self.metrics
            .server_status
            .with_label_values(&[&server.name, host, &port_label])
            .set(if is_up { 1 } else { 0 });
        self.metrics
            .server_features_total
            .with_label_values(&[&server.name])
            .set(status.features.len() as i64);
        self.metrics
            .scrape_duration
            .with_label_values(&[&server.name])
            .set(round3(duration));
        self.metrics
            .scrape_total
            .with_label_values(&[&server.name, if is_up { "success" } else { "failure" }])
            .inc();

        for feature in &status.features {
            let labels = [
                server.name.as_str(),
                feature.name.as_str(),
                feature.vendor.as_str(),
            ];
            self.metrics
                .feature_total
                .with_label_values(&labels)
                .set(feature.total);
            self.metrics
                .feature_used
                .with_label_values(&labels)
                .set(feature.used);
            self.metrics
                .feature_idle
                .with_label_values(&labels)
                .set(feature.idle());
            self.metrics
                .feature_users
                .with_label_values(&[server.name.as_str(), feature.name.as_str()])
                .set(feature.users.len() as i64);
        }

        tracing::info!(
            "[{}] 采集完成: {} features, {}/{} licenses in use, {:.3}s",
            server.name,
            status.features.len(),
            status.total_used(),
            status.total_licenses(),
            duration
        );
    }

    fn store_status(&self, server: &LicenseServerConfig, status: ServerStatus) {
        let mut guard = self.lock_status();
        match guard.iter_mut().find(|(name, _)| name == &server.name) {
            Some(entry) => entry.1 = status,
            None => guard.push((server.name.clone(), status)),
        }
    }

    /// 最近一次采集结果（按配置顺序返回）
    pub fn last_status(&self) -> Vec<(String, ServerStatus)> {
        self.lock_status().clone()
    }

    /// 后台采集循环：立即采集一次，之后每 `interval` 采集一次，直到 `shutdown` 置位
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) {
        tracing::info!(
            "采集器启动，间隔 {}s，监控 {} 台 License Server",
            self.interval.as_secs(),
            self.servers.len()
        );

        loop {
            if *shutdown.borrow() {
                break;
            }

            self.collect_once().await;

            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = tokio::time::sleep(self.interval) => {}
            }
        }

        tracing::info!("采集循环已停止");
    }

    fn lock_status(&self) -> MutexGuard<'_, Vec<(String, ServerStatus)>> {
        // 临界区不会 panic，忽略 poison 即可
        self.last_status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// 四舍五入到 3 位小数（对应 Python `round(x, 3)`）
fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lmutil::{FeatureUsage, UserUsage};

    fn collector_config() -> Config {
        serde_yaml::from_str(
            r#"
license_servers:
  - name: "main-server"
    host: "192.168.1.100"
    port: 27000
collector:
  interval: 30
  timeout: 15
  retries: 3
"#,
        )
        .unwrap()
    }

    #[test]
    fn round3_matches_python_round() {
        assert_eq!(round3(0.12349), 0.123);
        assert_eq!(round3(1.5), 1.5);
        assert_eq!(round3(2.0004), 2.0);
    }

    #[test]
    fn metrics_reflect_server_status() {
        let config = collector_config();
        let metrics = Arc::new(Metrics::new().unwrap());
        let collector = LicenseCollector::new(&config, Arc::clone(&metrics));
        let server = config.license_servers[0].clone();

        let status = ServerStatus {
            server_name: server.name.clone(),
            server_host: server.host.clone(),
            lmgrd_port: server.port,
            status: "UP".to_owned(),
            features: vec![FeatureUsage {
                name: "vcs".to_owned(),
                vendor: "snpslmd".to_owned(),
                total: 20,
                used: 12,
                users: vec![UserUsage::default(), UserUsage::default()],
            }],
            ..Default::default()
        };

        collector.update_metrics(&server, &status, 0.4567);

        let rendered = metrics.render();
        assert!(rendered.contains(
            "flexlm_server_status{host=\"192.168.1.100\",port=\"27000\",server=\"main-server\"} 1"
        ));
        assert!(rendered.contains("flexlm_server_features_total{server=\"main-server\"} 1"));
        assert!(rendered.contains(
            "flexlm_feature_total{feature=\"vcs\",server=\"main-server\",vendor=\"snpslmd\"} 20"
        ));
        assert!(rendered.contains(
            "flexlm_feature_used{feature=\"vcs\",server=\"main-server\",vendor=\"snpslmd\"} 12"
        ));
        assert!(rendered.contains(
            "flexlm_feature_idle{feature=\"vcs\",server=\"main-server\",vendor=\"snpslmd\"} 8"
        ));
        assert!(rendered.contains("flexlm_feature_users{feature=\"vcs\",server=\"main-server\"} 2"));
        assert!(rendered.contains("flexlm_scrape_duration_seconds{server=\"main-server\"} 0.457"));
        assert!(
            rendered.contains("flexlm_scrape_total{server=\"main-server\",status=\"success\"} 1")
        );
    }

    #[test]
    fn down_server_reports_zero_status() {
        let config = collector_config();
        let metrics = Arc::new(Metrics::new().unwrap());
        let collector = LicenseCollector::new(&config, Arc::clone(&metrics));
        let server = config.license_servers[0].clone();

        let status = ServerStatus {
            server_host: server.host.clone(),
            lmgrd_port: server.port,
            status: "DOWN".to_owned(),
            parse_error: "采集失败".to_owned(),
            ..Default::default()
        };
        collector.update_metrics(&server, &status, 15.0);

        let rendered = metrics.render();
        assert!(rendered.contains(
            "flexlm_server_status{host=\"192.168.1.100\",port=\"27000\",server=\"main-server\"} 0"
        ));
        assert!(
            rendered.contains("flexlm_scrape_total{server=\"main-server\",status=\"failure\"} 1")
        );
    }

    #[test]
    fn last_status_keeps_config_order() {
        let mut config = collector_config();
        config.license_servers.push(LicenseServerConfig {
            name: "backup-server".to_owned(),
            host: "192.168.1.101".to_owned(),
            port: 27000,
            lmutil_path: String::new(),
            vendor_port: None,
        });

        let metrics = Arc::new(Metrics::new().unwrap());
        let collector = LicenseCollector::new(&config, metrics);
        assert_eq!(collector.server_count(), 2);

        for server in &config.license_servers {
            collector.store_status(server, ServerStatus::default());
        }
        let names: Vec<String> = collector
            .last_status()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names, vec!["main-server", "backup-server"]);
    }
}
