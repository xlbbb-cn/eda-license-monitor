# 裸机部署与 systemd 服务化

不跑 Docker，把采集器直接装到宿主机上长期运行。

[快速开始](quickstart.md) 里的「方式二 / 方式三」是前台运行，关掉终端就停了。本文把它们变成 systemd 服务：开机自启、崩溃自动重启、日志进 journald。

整套系统里只有采集器需要访问 License Server，所以裸机部署通常只装采集器；Prometheus 和 Grafana 继续用 Docker Compose 跑（见 [quickstart.md](quickstart.md) 方式一）也完全没问题。

## 前置条件

- Linux + systemd（本文命令在 Debian / Ubuntu / RHEL 系通用）
- Python 3.9+ **或** Rust 1.85+，二选一即可——两者暴露的指标名称、标签完全一致，可以随时换
- `lmutil`（FlexLM 客户端自带，只装客户端即可，不需要 Server 端）

## 1. 建用户、放代码

采集器不需要 root 权限，用一个无登录权限的系统用户跑：

```bash
sudo useradd --system --create-home --shell /usr/sbin/nologin eda-monitor

sudo mkdir -p /opt/eda-license-monitor
sudo chown -R eda-monitor:eda-monitor /opt/eda-license-monitor
```

> 用 `--create-home` 而不是 `--no-create-home`：`cargo` 和 `pip` 都需要一个可写的 HOME 目录放缓存，没有家目录时会直接报错。

把仓库放进去（在 `/opt/eda-license-monitor` 里 `git clone`，或者从别处 rsync 过来），然后准备本地配置——`config.local.yaml` 不在版本控制里，`git pull` 不会覆盖它：

```bash
cd /opt/eda-license-monitor
sudo -u eda-monitor -H cp config/config.yaml config/config.local.yaml
sudo -u eda-monitor -H vim config/config.local.yaml   # 填 license_servers 的地址
```

## 2. 先跑通单次采集

**先别急着写 unit。** systemd 服务里的 PATH、HOME、工作目录都和你的登录 shell 不同，出错时日志还会被 systemd 包一层，排查成本高得多。先在普通 shell 里跑通一次：

Python 版：

```bash
cd /opt/eda-license-monitor
sudo -u eda-monitor -H python3 -m venv .venv
sudo -u eda-monitor -H .venv/bin/pip install -r collector/python/requirements.txt

sudo -u eda-monitor -H .venv/bin/python collector/python/prometheus_exporter.py \
  -c config/config.local.yaml --once
```

Rust 版：

```bash
cd /opt/eda-license-monitor/collector/rust
sudo -u eda-monitor -H cargo build --release

sudo -u eda-monitor -H ./target/release/eda-license-collector \
  -c ../../config/config.local.yaml --once
```

看到 `Status: UP` 和各 Feature 的使用量，说明配置、网络、lmutil 都没问题，可以上 systemd 了。

## 3. 写 unit 文件

两份 unit 都叫 `eda-license-collector.service`，**按你选的采集器装一个就行**。

### Python 版

`/etc/systemd/system/eda-license-collector.service`：

```ini
[Unit]
Description=EDA License Monitor Collector (Python)
Documentation=https://github.com/xlbbb-cn/eda-license-monitor
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=eda-monitor
Group=eda-monitor
WorkingDirectory=/opt/eda-license-monitor

ExecStart=/opt/eda-license-monitor/.venv/bin/python /opt/eda-license-monitor/collector/python/prometheus_exporter.py -c /opt/eda-license-monitor/config/config.local.yaml
Environment=PYTHONUNBUFFERED=1
# lmutil 不在系统默认 PATH 里时在这里补上；更推荐直接在 config.local.yaml 里写 lmutil_path 绝对路径
Environment=PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/opt/flexlm/bin

# Python 版只有收到 SIGINT 才会走优雅停止分支（打日志 + 停采集线程），SIGTERM 会直接终止进程
KillSignal=SIGINT
Restart=always
RestartSec=10

NoNewPrivileges=true
ProtectSystem=full
ProtectHome=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

### Rust 版

编译产物是单个二进制，装到 `/usr/local/bin` 就不依赖源码目录了：

```bash
sudo install -m 0755 /opt/eda-license-monitor/collector/rust/target/release/eda-license-collector /usr/local/bin/
```

`/etc/systemd/system/eda-license-collector.service`：

```ini
[Unit]
Description=EDA License Monitor Collector (Rust)
Documentation=https://github.com/xlbbb-cn/eda-license-monitor
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=eda-monitor
Group=eda-monitor
WorkingDirectory=/opt/eda-license-monitor

ExecStart=/usr/local/bin/eda-license-collector -c /opt/eda-license-monitor/config/config.local.yaml
Environment=RUST_LOG=info
Environment=PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/opt/flexlm/bin

# Rust 版把 SIGTERM 当优雅关闭信号，内部最多等 10 秒
TimeoutStopSec=30
Restart=always
RestartSec=10

NoNewPrivileges=true
ProtectSystem=full
ProtectHome=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

## 4. 启动与验证

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now eda-license-collector
systemctl status eda-license-collector
```

指标端点：

```bash
curl -s http://localhost:9091/metrics | grep flexlm_feature_total
```

日志（systemd 自动收进 journald，不用自己配日志轮转）：

```bash
journalctl -u eda-license-collector -f
journalctl -u eda-license-collector --since "10 min ago"
```

改过 unit 文件后记得 `sudo systemctl daemon-reload && sudo systemctl restart eda-license-collector`。

## 5. 升级

```bash
cd /opt/eda-license-monitor
sudo -u eda-monitor -H git pull

# Python 版：依赖有变化时才需要重装
sudo -u eda-monitor -H .venv/bin/pip install -r collector/python/requirements.txt

# Rust 版：重新编译并覆盖二进制
cd collector/rust
sudo -u eda-monitor -H cargo build --release
sudo install -m 0755 target/release/eda-license-collector /usr/local/bin/

sudo systemctl restart eda-license-collector
```

## 6. 常见坑

**lmutil 找不到** — systemd 服务的 PATH 比登录 shell 窄，不含 `/opt` 之类。两个解法：unit 里补 `Environment=PATH=...`（见上面示例），或者在 `config.local.yaml` 里直接写 `lmutil_path: "/opt/flexlm/bin/lmutil"`。**推荐后者**——配置里一眼能看到用的是哪个 lmutil，换机器时不用翻 unit 文件。

**Prometheus 在 Docker 里、采集器在宿主机** — 这种混合部署很常见。`prometheus.yml` 里的 target 别写 `localhost:9091`，那指向的是 Prometheus 容器自己。要写宿主机的 IP，或者给 Prometheus 容器加 `extra_hosts: ["host.docker.internal:host-gateway"]` 然后用 `host.docker.internal:9091`。

**和 Docker 版同时跑** — 会抢 9091 端口，而且两边都往 Prometheus 上报同名指标。裸机部署前先把容器里的采集器停掉：`docker compose stop collector-rust collector-python`。

**RHEL / CentOS 开了 SELinux** — 非标准路径下的 lmutil 可能被执行策略拦住（`journalctl` 或 `ausearch -m avc` 里能看到 AVC 拒绝）。给文件打上正确的类型标签即可：`sudo chcon -t bin_t /opt/flexlm/bin/lmutil`。

**端口小于 1024 才需要特殊权限** — 默认 9091 不需要，所以不用给服务提权。真要改成 80 之类，用 `AmbientCapabilities=CAP_NET_BIND_SERVICE`，别用 root 跑整个服务。

**unit 文件不生效** — 改完 `/etc/systemd/system/` 下的 unit 必须 `systemctl daemon-reload`，否则 systemd 用的还是旧定义。
