"""
FlexLM Prometheus Exporter

定时调用 lmutil_parser 采集 License 使用数据，
暴露 Prometheus metrics 端点供 Prometheus 抓取。

Metrics:
  flexlm_feature_total    - Feature 的 License 总数
  flexlm_feature_used     - Feature 的 License 已用量
  flexlm_feature_idle     - Feature 的 License 空闲数
  flexlm_feature_users    - Feature 当前占用用户数
  flexlm_server_status    - License Server 是否可达 (1=UP, 0=DOWN)
  flexlm_scrape_duration  - 采集耗时（秒）
"""

import time
import threading
import logging
from pathlib import Path
from typing import Optional

import yaml
from prometheus_client import (
    Gauge,
    Counter,
    CollectorRegistry,
    start_http_server,
)

try:  # 包内引用：python -m collector.python.prometheus_exporter
    from collector.python.lmutil_parser import collect_server, ServerStatus
except ImportError:  # 直接运行脚本：python prometheus_exporter.py
    from lmutil_parser import collect_server, ServerStatus

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Prometheus Metrics 定义
# ---------------------------------------------------------------------------

REGISTRY = CollectorRegistry(auto_describe=True)

# Feature 级指标
FEATURE_TOTAL = Gauge(
    "flexlm_feature_total",
    "Total licenses issued for this feature",
    ["server", "feature", "vendor"],
    registry=REGISTRY,
)
FEATURE_USED = Gauge(
    "flexlm_feature_used",
    "Licenses currently in use for this feature",
    ["server", "feature", "vendor"],
    registry=REGISTRY,
)
FEATURE_IDLE = Gauge(
    "flexlm_feature_idle",
    "Licenses idle (total - used) for this feature",
    ["server", "feature", "vendor"],
    registry=REGISTRY,
)
FEATURE_USERS = Gauge(
    "flexlm_feature_users",
    "Number of distinct users holding this feature",
    ["server", "feature"],
    registry=REGISTRY,
)

# Server 级指标
SERVER_STATUS = Gauge(
    "flexlm_server_status",
    "License server reachability (1=UP, 0=DOWN/UNKNOWN)",
    ["server", "host", "port"],
    registry=REGISTRY,
)
SERVER_FEATURES_TOTAL = Gauge(
    "flexlm_server_features_total",
    "Total number of monitored features on this server",
    ["server"],
    registry=REGISTRY,
)

# 采集指标
SCRAPE_DURATION = Gauge(
    "flexlm_scrape_duration_seconds",
    "Time spent collecting license data from lmutil",
    ["server"],
    registry=REGISTRY,
)
SCRAPE_COUNT = Counter(
    "flexlm_scrape_total",
    "Total number of scrape attempts",
    ["server", "status"],
    registry=REGISTRY,
)


# ---------------------------------------------------------------------------
# 采集逻辑
# ---------------------------------------------------------------------------

class LicenseCollector:
    """定期采集 License 数据并更新 Prometheus 指标"""

    def __init__(self, config: dict):
        self.servers = config.get("license_servers", [])
        self.interval = config.get("collector", {}).get("interval", 30)
        self.timeout = config.get("collector", {}).get("timeout", 15)
        self.retries = config.get("collector", {}).get("retries", 3)
        self._last_status: dict[str, ServerStatus] = {}
        self._stop_event = threading.Event()

    def collect_once(self):
        """执行一次全量采集"""
        for srv in self.servers:
            name = srv["name"]
            host = srv["host"]
            port = srv.get("port", 27000)
            lmutil_path = srv.get("lmutil_path", "")

            start = time.time()
            status = collect_server(
                host=host,
                port=port,
                lmutil_path=lmutil_path,
                timeout=self.timeout,
                retries=self.retries,
            )
            duration = time.time() - start

            self._last_status[name] = status
            self._update_metrics(name, status, duration)

    def _update_metrics(self, server_name: str, status: ServerStatus, duration: float):
        """将采集结果写入 Prometheus 指标"""
        host = status.server_host
        port = str(status.lmgrd_port)

        # Server 状态
        is_up = 1 if status.status.upper() in ("UP", "") else 0
        SERVER_STATUS.labels(server=server_name, host=host, port=port).set(is_up)
        SERVER_FEATURES_TOTAL.labels(server=server_name).set(len(status.features))
        SCRAPE_DURATION.labels(server=server_name).set(round(duration, 3))

        if is_up:
            SCRAPE_COUNT.labels(server=server_name, status="success").inc()
        else:
            SCRAPE_COUNT.labels(server=server_name, status="failure").inc()

        # Feature 级指标
        for feat in status.features:
            labels = dict(server=server_name, feature=feat.name, vendor=feat.vendor)
            FEATURE_TOTAL.labels(**labels).set(feat.total)
            FEATURE_USED.labels(**labels).set(feat.used)
            FEATURE_IDLE.labels(
                server=server_name, feature=feat.name, vendor=feat.vendor
            ).set(feat.total - feat.used)
            FEATURE_USERS.labels(server=server_name, feature=feat.name).set(
                len(feat.users)
            )

        logger.info(
            "[%s] 采集完成: %d features, %d/%d licenses in use, %.3fs",
            server_name,
            len(status.features),
            status.total_used,
            status.total_licenses,
            duration,
        )

    def start_loop(self):
        """在后台线程中循环采集"""
        def _loop():
            logger.info(
                "采集器启动，间隔 %ds，监控 %d 台 License Server",
                self.interval, len(self.servers),
            )
            while not self._stop_event.is_set():
                try:
                    self.collect_once()
                except Exception as e:
                    logger.error("采集异常: %s", e, exc_info=True)
                self._stop_event.wait(self.interval)

        t = threading.Thread(target=_loop, daemon=True, name="license-collector")
        t.start()
        return t

    def stop(self):
        self._stop_event.set()

    def get_last_status(self) -> dict:
        return dict(self._last_status)


# ---------------------------------------------------------------------------
# 配置加载
# ---------------------------------------------------------------------------

def load_config(config_path: str = "config/config.yaml") -> dict:
    """加载 YAML 配置文件，支持 local 覆盖"""
    base = Path(config_path)
    local = base.with_suffix(".yaml.local")

    if local.exists():
        logger.info("加载本地覆盖配置: %s", local)
        with open(local) as f:
            return yaml.safe_load(f)

    with open(base) as f:
        return yaml.safe_load(f)


# ---------------------------------------------------------------------------
# 入口
# ---------------------------------------------------------------------------

def main():
    import argparse

    parser = argparse.ArgumentParser(description="FlexLM License Prometheus Exporter")
    parser.add_argument(
        "-c", "--config",
        default="config/config.yaml",
        help="配置文件路径 (默认: config/config.yaml)",
    )
    parser.add_argument(
        "--port",
        type=int,
        default=None,
        help="Prometheus 指标端口（覆盖配置文件）",
    )
    parser.add_argument(
        "--once",
        action="store_true",
        help="仅执行一次采集并输出结果，不启动 HTTP 服务",
    )
    args = parser.parse_args()

    # 日志
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
        datefmt="%Y-%m-%d %H:%M:%S",
    )

    config = load_config(args.config)
    port = args.port or config.get("prometheus", {}).get("port", 9091)

    collector = LicenseCollector(config)

    if args.once:
        logger.info("单次采集模式")
        collector.collect_once()
        for name, status in collector.get_last_status().items():
            print(f"\n=== {name} ({status.server_host}:{status.lmgrd_port}) ===")
            print(f"Status: {status.status}")
            print(f"Features: {len(status.features)}, "
                  f"Total: {status.total_licenses}, "
                  f"Used: {status.total_used}")
            for feat in status.features:
                pct = (feat.used / feat.total * 100) if feat.total else 0
                print(f"  {feat.name:20s} {feat.used:4d}/{feat.total:4d} "
                      f"({pct:5.1f}%) users={len(feat.users)}")
        return

    # 启动 Prometheus HTTP 端点
    start_http_server(port, registry=REGISTRY)
    logger.info("Prometheus 指标端口: http://0.0.0.0:%d/metrics", port)

    # 启动采集循环
    collector.start_loop()

    # 主线程保活
    try:
        while True:
            time.sleep(60)
    except KeyboardInterrupt:
        logger.info("收到中断信号，正在停止...")
        collector.stop()


if __name__ == "__main__":
    main()
