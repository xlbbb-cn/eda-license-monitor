# 快速开始

15 分钟内完成 EDA License 监控系统部署。

## 前置条件

- Docker + Docker Compose（推荐方式）
- 或 Python 3.9+ / Rust 1.85+（手动方式）
- FlexLM 客户端的 `lmutil` 可执行文件，或服务器可访问 FlexLM Server 端口

---

## 方式一：Docker Compose（推荐）

### 1. 克隆项目

```bash
git clone https://github.com/xlbbb-cn/eda-license-monitor.git
cd eda-license-monitor
```

### 2. 编辑配置

```bash
cp config/config.yaml config/config.local.yaml
vim config/config.local.yaml
```

修改 `license_servers` 部分，填入你的 License Server 地址：

```yaml
license_servers:
  - name: "main-server"
    host: "192.168.1.100"
    port: 27000
```

### 3. 启动服务

```bash
# 默认使用 Rust 采集器（推荐生产环境）
docker compose up -d

# 改用 Python 采集器（两个采集器只能启用一个，否则会重复上报同名指标）
docker compose stop collector-rust
docker compose --profile python up -d collector-python
```

### 4. 访问服务

| 服务 | 地址 | 说明 |
|------|------|------|
| Grafana | http://localhost:3000 | 用户名 `admin`，密码 `eda-license-2025` |
| Prometheus | http://localhost:9090 | 查看采集状态和指标 |
| Metrics | http://localhost:9091/metrics | 采集器暴露的原始指标 |

---

## 方式二：手动部署 — Python

### 1. 安装依赖

```bash
cd collector/python
pip install -r requirements.txt
```

### 2. 单次采集测试

```bash
python prometheus_exporter.py -c ../../config/config.yaml --once
```

正常输出类似：

```
=== main-server (192.168.1.100:27000) ===
Status: UP
Features: 8, Total: 120, Used: 45
  VCS                     20/20 ( 85.0%) users=6
  PT                      15/15 ( 40.0%) users=2
  DC                      10/10 ( 90.0%) users=3
  ...
```

### 3. 启动持续采集

```bash
python prometheus_exporter.py -c ../../config/config.yaml
```

---

## 方式三：手动部署 — Rust

### 1. 编译

```bash
cd collector/rust
cargo build --release
```

### 2. 单次采集测试

```bash
./target/release/eda-license-collector -c ../../config/config.yaml --once
```

输出格式与 Python 版一致（失败时额外打印一行 `Error: ...`，便于排查原因）。

### 3. 启动持续采集

```bash
./target/release/eda-license-collector -c ../../config/config.yaml
```

可选参数：

```bash
# 指定端口（覆盖配置文件）
./target/release/eda-license-collector -c config/config.yaml --port 9092

# 设置日志级别
RUST_LOG=debug ./target/release/eda-license-collector -c config/config.yaml
```

---

## 配置 Prometheus

在 `prometheus.yml` 中添加：

```yaml
scrape_configs:
  - job_name: "flexlm"
    static_configs:
      - targets: ["your-host:9091"]
```

---

## 导入 Grafana 仪表盘

1. 打开 Grafana → + → Import
2. 上传 `grafana/provisioning/dashboards/overview.json`
3. 选择 Prometheus 数据源
4. 点击 Import

Docker Compose 方式已自动导入，无需手动操作。

---

## 验证

```bash
# 检查指标是否正常输出
curl http://localhost:9091/metrics | grep flexlm_feature_total
```

正常输出类似：

```
flexlm_feature_total{server="main-server",feature="VCS",vendor="synopsys"} 20
flexlm_feature_used{server="main-server",feature="VCS",vendor="synopsys"} 12
flexlm_feature_idle{server="main-server",feature="VCS",vendor="synopsys"} 8
```

---

## 切换采集器

Python 和 Rust 采集器的 Prometheus 指标名称、标签完全一致，可随时切换：

```bash
# 停止当前采集器
docker compose down

# 切换到另一种语言的采集器
docker compose --profile python up -d    # 切到 Python
docker compose up -d                     # 切到 Rust
```

Grafana 仪表盘和 Prometheus 告警规则无需任何修改。

---

## 常见问题

<details>
<summary><b>Q: 采集器连不上 License Server 怎么办？</b></summary>

1. 确认网络连通：`telnet <license-server-ip> 27000`
2. 确认 lmutil 可用：`lmutil lmstat -a -c 27000@<license-server-ip>`
3. 检查防火墙是否放行了采集器所在机器的访问
4. 查看采集器日志：`docker logs eda-license-collector`
</details>

<details>
<summary><b>Q: 不安装 lmutil 能用吗？</b></summary>

不行。采集器通过调用 FlexLM 官方的 `lmutil` 命令行工具获取数据，这是 FlexLM 的标准接口，不依赖任何第三方库。你需要在采集器所在机器上安装 FlexLM 客户端（不需要 Server 端）。
</details>

<details>
<summary><b>Q: 采集器会不会打挂 License Server？</b></summary>

不会。默认 30 秒采集一次，每次只发送一个 `lmutil lmstat -a` 请求，对 License Server 的负载可以忽略不计。FlexLM Server 本身有连接数限制，建议采集间隔不低于 15 秒。
</details>

<details>
<summary><b>Q: Grafana 默认密码安全吗？</b></summary>

仅限内网使用。生产环境请修改 `docker-compose.yml` 中的 `GF_SECURITY_ADMIN_PASSWORD`，或通过环境变量覆盖。
</details>

<details>
<summary><b>Q: 数据保留多久？</b></summary>

- Prometheus 默认保留 90 天（可在 docker-compose.yml 中修改 `--storage.tsdb.retention.time`）
- 建议：原始数据保留 90 天，聚合数据可通过 Recording Rules 保留 2 年
</details>

<details>
<summary><b>Q: Rust 编译很慢怎么办？</b></summary>

首次编译约需 2-5 分钟（取决于机器性能），后续增量编译只需几秒。如果不想本地编译，可以直接使用 Docker 方式：`docker compose up -d` 会自动构建 Rust 镜像。生产环境推荐使用预构建的 Docker 镜像。
</details>
