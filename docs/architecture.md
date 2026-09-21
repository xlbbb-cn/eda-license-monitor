# 架构说明

## 系统架构

```
┌──────────────────────────────────────────────────────────┐
│                    EDA 工程师工作站                         │
│                                                          │
│  VCS / Xcelium / PrimeTime / Design Compiler / ...       │
│       │                                                  │
│       ▼                                                  │
│  ┌─────────────┐                                         │
│  │ FlexLM 客户端 │  flexlm许可证请求                       │
│  └──────┬──────┘                                         │
└─────────┼────────────────────────────────────────────────┘
          │
          ▼
┌─────────────────────┐
│  FlexLM License     │  lmgrd + vendor daemon
│  Server             │  管理所有 License 席位
│  (端口 27000)       │
└──────────┬──────────┘
           │ lmutil lmstat -a（每30秒）
           ▼
┌─────────────────────┐     ┌──────────────────┐
│  Data Collector     │────▶│  Prometheus       │
│  (Python / Rust)    │     │  (时序数据库)     │
│  端口 9091          │     │  端口 9090        │
└─────────────────────┘     └────────┬─────────┘
                                     │ 查询
                                     ▼
                          ┌──────────────────┐
                          │  Grafana          │
                          │  (可视化仪表盘)   │
                          │  端口 3000        │
                          └──────────────────┘
                                     │ 告警
                                     ▼
                          ┌──────────────────┐
                          │  AlertManager     │
                          │  (可选)           │
                          │  飞书/钉钉/邮件   │
                          └──────────────────┘
```

## 采集器架构

Python 和 Rust 两种实现共享相同的处理流程：

```
                    ┌──────────────────────────────────┐
                    │         Data Collector            │
                    │                                  │
  lmutil lmstat ──▶│  ┌────────────┐  ┌────────────┐  │──▶ Prometheus
  (stdout)         │  │  Parser    │  │  Metrics   │  │    /metrics
                    │  │  解析文本  │──▶│  更新 Gauge │  │    (HTTP pull)
                    │  │  输出      │  │  暴露端点   │  │
                    │  └────────────┘  └────────────┘  │
                    └──────────────────────────────────┘
```

| 模块 | Python | Rust | 职责 |
|------|--------|------|------|
| 解析器 | `lmutil_parser.py` | `src/lmutil.rs` | 正则解析 lmutil 输出 → `ServerStatus` 结构体 |
| 指标 | `prometheus_exporter.py` | `src/metrics.rs` | 定义 Prometheus Gauge/Counter |
| 采集器 | `LicenseCollector` 类 | `src/collector.rs` | 定时采集循环 + 指标更新 |
| 配置 | `load_config()` | `src/config.rs` | YAML 配置加载（支持 local 覆盖） |
| 入口 | `prometheus_exporter.py` | `src/main.rs` | CLI 参数解析 + HTTP 服务 |

## 数据流

```
lmutil lmstat -a
       │
       ▼
  ┌──────────────┐
  │ Parser       │  解析文本输出 → ServerStatus 结构体
  │              │  提取 Feature、数量、用户信息
  └──────┬───────┘
         │ ServerStatus 对象
         ▼
  ┌──────────────┐
  │ Metrics      │  更新 Prometheus Gauge 指标
  │              │  暴露 /metrics HTTP 端点
  └──────┬───────┘
         │ HTTP pull
         ▼
  ┌──────────────┐
  │ Prometheus   │  每 30s 拉取指标
  │              │  存储到本地时序数据库
  └──────┬───────┘
         │ PromQL query
         ▼
  ┌──────────────┐
  │ Grafana      │  三个仪表盘：总览 / 趋势 / 用户分析
  └──────────────┘
```

## Prometheus 指标

| 指标名 | 类型 | 说明 | 标签 |
|--------|------|------|------|
| `flexlm_feature_total` | Gauge | License 总数 | server, feature, vendor |
| `flexlm_feature_used` | Gauge | 已使用数 | server, feature, vendor |
| `flexlm_feature_idle` | Gauge | 空闲数 | server, feature, vendor |
| `flexlm_feature_users` | Gauge | 占用用户数 | server, feature |
| `flexlm_server_status` | Gauge | Server 状态 (1=UP) | server, host, port |
| `flexlm_server_features_total` | Gauge | Server 上 Feature 总数 | server |
| `flexlm_scrape_duration_seconds` | Gauge | 采集耗时 | server |
| `flexlm_scrape_total` | Counter | 采集次数 | server, status |

## 告警规则

| 规则 | 条件 | 持续时间 | 级别 | 用途 |
|------|------|---------|------|------|
| LicenseHighUsage | 使用率 > 90% | 5分钟 | warning | 即将不够用 |
| LicenseFull | 使用率 = 100% | 2分钟 | critical | 已全部占满 |
| LicenseLowUsage | 使用率 < 20% 且总数 > 5 | 24小时 | info | 可考虑缩减 |
| LicenseServerDown | 采集失败 | 2分钟 | critical | Server 异常 |

## 目录结构

```
eda-license-monitor/
├── README.md                           # 项目说明
├── LICENSE                             # MIT License
├── .gitignore
├── docker-compose.yml                  # 一键部署（默认构建 Rust 采集器）
├── config/
│   ├── config.yaml                     # 主配置（License Server 地址等）
│   ├── config.yaml.local               # 本地覆盖配置（不入库）
│   ├── prometheus.yml                  # Prometheus 采集配置
│   ├── alert_rules.yaml                # Prometheus 告警规则
│   └── alertmanager.yml                # AlertManager 配置（飞书/钉钉模板）
├── collector/
│   ├── python/                         # ── Python 采集器 ──
│   │   ├── Dockerfile
│   │   ├── lmutil_parser.py            # lmutil 输出解析器
│   │   ├── prometheus_exporter.py      # Prometheus Exporter 主程序
│   │   ├── requirements.txt            # Python 依赖
│   │   └── __init__.py
│   └── rust/                           # ── Rust 采集器 ──
│       ├── Dockerfile                  # 多阶段构建（编译 300MB → 运行 ~15MB）
│       ├── Cargo.toml                  # 项目配置
│       ├── Cargo.lock
│       ├── src/
│       │   ├── main.rs                 # CLI 入口 + HTTP 服务
│       │   ├── lib.rs                  # 库导出
│       │   ├── lmutil.rs               # lmutil 输出解析器
│       │   ├── metrics.rs              # Prometheus 指标定义
│       │   ├── config.rs               # YAML 配置加载
│       │   └── collector.rs            # 定时采集循环
│       ├── tests/
│       │   ├── parser.rs               # 解析器集成测试（fixtures 驱动）
│       │   ├── collect.rs              # 采集流程进程级测试
│       │   ├── python_parity_check.py  # 打印 Python 版解析结果用于对照
│       │   └── fixtures/               # 测试数据
│       └── examples/
│           └── fake_lmutil.rs          # 模拟 lmutil 用于测试
├── grafana/
│   └── provisioning/
│       ├── datasources/
│       │   └── prometheus.yaml         # 数据源自动注册
│       └── dashboards/
│           ├── dashboard.yaml          # 仪表盘自动加载
│           ├── overview.json           # 总览仪表盘
│           ├── trend.json              # 趋势分析仪表盘
│           └── users.json              # 用户分析仪表盘
└── docs/
    ├── quickstart.md                   # 快速开始（Docker + Python + Rust）
    ├── architecture.md                 # 架构说明（本文件）
    └── screenshots/                    # 仪表盘截图（部署后替换）
        ├── overview.png
        ├── trend.png
        └── users.png
```

## 技术选型

### 为什么同时提供 Python 和 Rust

| 场景 | 推荐 | 原因 |
|------|------|------|
| 快速验证 / POC | Python | 依赖安装快，5 分钟跑起来 |
| 生产环境部署 | Rust | 镜像小（15MB vs 120MB）、内存低（5MB vs 30MB）、无 Python 运行时依赖 |
| 团队技术栈为 Python | Python | 维护成本最低 |
| 资源受限环境 | Rust | CPU 和内存占用均优于 Python |
| 需要自定义扩展 | Python | 改个脚本比重新编译快 |

### 一致性保证

两种实现共享：
- **相同的配置格式**：`config/config.yaml`，无需修改
- **相同的 Prometheus 指标**：指标名、标签、HELP 文本完全一致
- **相同的 Grafana 仪表盘**：同一套 JSON 模板
- **相同的告警规则**：`alert_rules.yaml` 无需修改

测试一致性：

```bash
# Rust 侧断言（期望值来自 Python 版解析器对同一批 fixtures 的输出）
cargo test --manifest-path collector/rust/Cargo.toml

# 打印 Python 版解析器对同一批 fixtures 的结果，用于人工对照
python collector/rust/tests/python_parity_check.py
```

`collector/rust/tests/fixtures/` 下的 lmstat 样例输出是两种实现的共同输入，
`tests/parser.rs` 中断言的解析结果与 Python 版 `parse_lmstat_output()` 一致。
