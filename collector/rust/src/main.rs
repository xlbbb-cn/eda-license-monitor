//! FlexLM License Prometheus Exporter（Rust 版主程序）。
//!
//! 用法与 Python 版一致：
//!
//! ```text
//! eda-license-collector -c config/config.yaml          # 持续采集 + 暴露 /metrics
//! eda-license-collector -c config/config.yaml --once   # 单次采集并打印结果
//! eda-license-collector -c config/config.yaml --port 9092
//! ```

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use clap::Parser;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing_subscriber::fmt::time::ChronoLocal;
use tracing_subscriber::EnvFilter;

use eda_license_collector::collector::LicenseCollector;
use eda_license_collector::config;
use eda_license_collector::lmutil::ServerStatus;
use eda_license_collector::metrics::{Metrics, METRICS_CONTENT_TYPE};

#[derive(Parser, Debug)]
#[command(
    name = "eda-license-collector",
    version,
    about = "FlexLM License Prometheus Exporter（Rust 版）"
)]
struct Cli {
    /// 配置文件路径
    #[arg(short, long, default_value = "config/config.yaml")]
    config: PathBuf,

    /// Prometheus 指标端口（覆盖配置文件中的 prometheus.port）
    #[arg(long)]
    port: Option<u16>,

    /// 仅执行一次采集并输出结果，不启动 HTTP 服务
    #[arg(long)]
    once: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing();

    let config = config::load(&cli.config)?;
    if config.license_servers.is_empty() {
        tracing::warn!("license_servers 为空，请先在配置文件中填写 License Server 地址");
    }

    let metrics = Arc::new(Metrics::new().context("初始化 Prometheus 指标失败")?);
    let collector = Arc::new(LicenseCollector::new(&config, Arc::clone(&metrics)));

    // 单次采集模式
    if cli.once {
        tracing::info!("单次采集模式");
        collector.collect_once().await;
        print_report(&collector.last_status());
        return Ok(());
    }

    let port = cli.port.unwrap_or(config.prometheus.port);
    let address = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("监听 {address} 失败，端口可能已被占用"))?;

    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/health", get(health_handler))
        .with_state(Arc::clone(&metrics));

    tracing::info!("Prometheus 指标端口: http://0.0.0.0:{port}/metrics");

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let http_task = tokio::spawn({
        let mut http_shutdown = shutdown_rx.clone();
        async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = http_shutdown.changed().await;
                })
                .await
        }
    });

    let collector_task = tokio::spawn({
        let collector = Arc::clone(&collector);
        let collect_shutdown = shutdown_rx.clone();
        async move { collector.run(collect_shutdown).await }
    });

    shutdown_signal().await;
    tracing::info!("收到中断信号，正在停止...");
    let _ = shutdown_tx.send(true);

    let _ = tokio::time::timeout(Duration::from_secs(10), async {
        let _ = collector_task.await;
        let _ = http_task.await;
    })
    .await;

    Ok(())
}

async fn metrics_handler(State(metrics): State<Arc<Metrics>>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, METRICS_CONTENT_TYPE)],
        metrics.render(),
    )
}

async fn health_handler() -> impl IntoResponse {
    (StatusCode::OK, "ok\n")
}

/// 等待 Ctrl-C（Windows / Linux）或 SIGTERM（容器 stop）
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => {
                tracing::warn!("注册 SIGTERM 处理器失败: {err}");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

/// 打印单次采集结果（对齐 Python 版 `--once` 的输出格式）
fn print_report(statuses: &[(String, ServerStatus)]) {
    if statuses.is_empty() {
        println!("没有可输出的采集结果，请检查 license_servers 配置");
        return;
    }

    for (name, status) in statuses {
        println!(
            "\n=== {name} ({}:{}) ===",
            status.server_host, status.lmgrd_port
        );
        println!(
            "Status: {}",
            if status.status.is_empty() {
                "UNKNOWN"
            } else {
                status.status.as_str()
            }
        );
        println!(
            "Features: {}, Total: {}, Used: {}",
            status.features.len(),
            status.total_licenses(),
            status.total_used()
        );
        if !status.parse_error.is_empty() {
            println!("Error: {}", status.parse_error);
        }

        for feature in &status.features {
            println!(
                "  {:<20} {:>4}/{:>4} ({:>5.1}%) users={}",
                feature.name,
                feature.used,
                feature.total,
                feature.usage_ratio() * 100.0,
                feature.users.len()
            );
        }
    }
}

/// 日志：`2026-09-21 12:34:56  INFO module: message`，级别可用 `RUST_LOG` 覆盖
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_timer(ChronoLocal::new("%Y-%m-%d %H:%M:%S".to_owned()))
        .init();
}
