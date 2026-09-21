# Rust 版 FlexLM 采集器

`collector/python` 的 Rust 实现，功能与对外行为完全等价：

- 调用 `lmutil lmstat -a -c <port>@<host>` 采集 License 使用数据
- 解析输出得到 Feature / 用户维度的使用情况
- 暴露与 Python 版**同名同标签**的 Prometheus 指标（`flexlm_*`），
  现有 `config/prometheus.yml`、`config/alert_rules.yaml`、Grafana 仪表盘无需任何改动

## 快速开始

前置条件：Rust 1.85+（依赖树中 clap / indexmap 等使用 edition 2024）与 Cargo。

### 本地运行

```bash
cargo build --release
```

```bash
# 单次采集并打印结果（用于确认能连上 License Server）
./target/release/eda-license-collector -c ../../config/config.yaml --once

# 持续采集 + 暴露 /metrics
./target/release/eda-license-collector -c ../../config/config.yaml

# 覆盖指标端口 / 调整日志级别
./target/release/eda-license-collector -c ../../config/config.yaml --port 9092
RUST_LOG=debug ./target/release/eda-license-collector -c ../../config/config.yaml
```

| 参数 | 说明 |
|------|------|
| `-c, --config <PATH>` | 配置文件路径，默认 `config/config.yaml` |
| `--port <PORT>` | 指标端口，覆盖配置文件中的 `prometheus.port` |
| `--once` | 只采集一次并打印结果，不启动 HTTP 服务 |
| `RUST_LOG` | 日志级别（`error` / `warn` / `info` / `debug`），默认 `info` |

HTTP 端点：

| 路径 | 说明 |
|------|------|
| `/metrics` | Prometheus 指标 |
| `/health` | 存活探针，返回 `ok` |

### Docker

`docker-compose.yml` 中的 `collector` 服务已指向本目录，直接 `docker-compose up -d` 即可。
容器需要厂商提供的 `lmutil`，请从宿主机挂载，并在配置中指定路径：

```yaml
license_servers:
  - name: "main-server"
    host: "192.168.1.100"
    port: 27000
    lmutil_path: "/opt/flexlm/lmutil"
```

```yaml
    volumes:
      - ./config:/app/config:ro
      - /opt/flexlm:/opt/flexlm:ro
```

## 没有 FlexLM 环境时如何验证

内置了一个假的 `lmutil`（仅用于自测）：

```bash
cargo build --examples
cargo run --example fake_lmutil -- full     # 正常输出
cargo run --example fake_lmutil -- table    # 旧版表格 + 简写格式
cargo run --example fake_lmutil -- error    # Server 不可达
```

把它填到 `lmutil_path` 即可跑通全链路；`host` 名中包含 `down` / `error` 时输出
“无法连接”，包含 `table` 时输出旧版表格格式。

## 测试

```bash
cargo test      # 单元测试 + 解析器/采集集成测试
cargo clippy --all-targets -- -D warnings
```

`tests/fixtures/` 中的样例输出同时用于 Python 版对照：

```bash
python collector/rust/tests/python_parity_check.py
```

该脚本打印 Python 版解析器对同一批样例的结果，Rust 版测试的期望值与之对齐。

## 与 Python 版的行为差异

除以下各点外，解析逻辑、指标语义、重试策略与 Python 版一致：

| 差异 | Python 版 | Rust 版 |
|------|-----------|---------|
| Server 不可达 | 解析不到 vendor daemon 行 → 状态为空 → `flexlm_server_status = 1`（误报 UP） | 识别 `Cannot connect to license server` 等失败信息 → `flexlm_server_status = 0`，并记录 `parse_error` |
| 现代 `lmstat -a` 格式 | `License server status: 27000@host`、`snpslmd: UP v11.19.6` 均无法匹配 | 两种格式均可识别 |
| 明确不可达时重试 | 空输出才重试；不可达时不会重试 | 不可达时立即返回（重试无意义），超时/无输出仍按 `retries` 重试 |
| lmutil 缺失 | 重试 `retries` 次后才返回 | 立即返回，并给出 `找不到 lmutil 可执行文件: <path>` |
| 输出编码 | `text=True` 按本地编码严格解码，非法字节抛 `UnicodeDecodeError` | 按 UTF-8 宽松解码，不中断采集 |
| 配置文件 BOM | PyYAML 容忍 UTF-8 BOM | 同样容忍（serde_yaml 本身不接受 BOM，已显式剥离） |
| 配置文件宿主 | `config/config.yaml.local` | `config/config.yaml.local`，其次 `config/config.local.yaml`（README 记载的命名） |
| 指标清理 | 不再出现的 Feature/Server 会保留最后一次数值 | 相同（保证 Grafana 行为一致），需要清理时重启采集器即可 |
| 数值渲染 | Gauge 渲染为浮点（`8.0`） | 渲染为整数（`8`）；Prometheus 文本格式两者等价，查询结果一致 |
| `*_created` 序列 | prometheus_client 为 Counter 额外导出 `flexlm_scrape_total_created` | `prometheus` crate 不导出该序列（Prometheus 2.43+ 默认丢弃 `_created`，现有查询未使用） |

> 注：`--once` 输出多一行 `Error: ...`，便于排查失败原因；`Status:` 为空时显示 `UNKNOWN`。

## 代码结构

```
src/
├── main.rs       # CLI、HTTP 服务（/metrics、/health）、优雅退出
├── lib.rs        # 库入口，导出各模块
├── config.rs     # YAML 配置加载 + local 覆盖 + 默认值
├── lmutil.rs     # lmstat 输出解析、lmutil 执行、单 Server 采集（带重试）
├── metrics.rs    # Prometheus 指标定义与渲染
└── collector.rs  # 定时采集循环、指标写入
tests/
├── parser.rs     # 解析器集成测试（fixtures 驱动）
├── collect.rs    # 采集进程级行为测试
└── fixtures/     # lmstat 样例输出
examples/
└── fake_lmutil.rs  # 自测用假 lmutil
```

## 指标一览

| 指标 | 类型 | 标签 |
|------|------|------|
| `flexlm_feature_total` | Gauge | server, feature, vendor |
| `flexlm_feature_used` | Gauge | server, feature, vendor |
| `flexlm_feature_idle` | Gauge | server, feature, vendor |
| `flexlm_feature_users` | Gauge | server, feature |
| `flexlm_server_status` | Gauge | server, host, port |
| `flexlm_server_features_total` | Gauge | server |
| `flexlm_scrape_duration_seconds` | Gauge | server |
| `flexlm_scrape_total` | Counter | server, status |
