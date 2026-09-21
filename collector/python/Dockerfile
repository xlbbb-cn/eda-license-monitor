FROM python:3.11-slim

WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY . .

# 如果系统已安装 FlexLM，确保 lmutil 在 PATH 中
# 否则通过 config.yaml 的 lmutil_path 指定路径
# 容器内通常需要从宿主机挂载，例如 -v /opt/flexlm:/opt/flexlm:ro

EXPOSE 9091

CMD ["python", "prometheus_exporter.py", "-c", "/app/config/config.yaml"]
