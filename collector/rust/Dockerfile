# ── Stage 1: build ──────────────────────────────────────────────
# 注意：依赖树要求 Rust 1.85+（clap / indexmap 等使用 edition 2024），
# 与 Cargo.toml 中的 rust-version 保持一致
FROM rust:1-slim-bookworm AS builder

WORKDIR /src

# 先复制 manifest + lock，利用 Docker 层缓存预编译依赖
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src \
    && echo "" > src/lib.rs \
    && echo "fn main() {}" > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src

# 复制真实源码，仅重新编译本 crate
COPY src/ src/
RUN cargo build --release --locked

# ── Stage 2: runtime ───────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*

# FlexLM 的 lmutil 是厂商专有二进制，镜像不便内置，这里放一个占位脚本：
# 未正确挂载时会在日志和 parse_error 中给出明确提示，而不是静默失败。
# 实际部署请从宿主机挂载，例如：-v /opt/flexlm:/opt/flexlm:ro
RUN printf '%s\n' '#!/bin/sh' \
    'echo "ERROR: lmutil not found. Please mount or install the FlexLM client." >&2' \
    'exit 1' > /usr/local/bin/lmutil && chmod +x /usr/local/bin/lmutil

WORKDIR /app

COPY --from=builder /src/target/release/eda-license-collector /usr/local/bin/

EXPOSE 9091

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${METRICS_PORT:-9091}/health" || exit 1

ENTRYPOINT ["eda-license-collector"]
CMD ["-c", "config/config.yaml"]
