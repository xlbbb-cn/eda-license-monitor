"""
FlexLM lmutil 输出解析器

解析 `lmutil lmstat -a` 的输出，提取 Feature 名称、总数、已用量、
占用用户等结构化信息。

兼容 FlexLM 9.x / 11.x / 11.16.x 等主流版本输出格式。
"""

import re
import subprocess
import logging
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Optional

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# 数据结构
# ---------------------------------------------------------------------------

@dataclass
class FeatureUsage:
    """单个 Feature 的使用情况"""
    name: str
    vendor: str = ""
    total: int = 0
    used: int = 0
    users: list = field(default_factory=list)


@dataclass
class UserUsage:
    """单个用户的占用情况"""
    username: str
    host: str
    display: str = ""
    start_time: str = ""
    feature: str = ""


@dataclass
class ServerStatus:
    """License Server 整体状态"""
    server_name: str = ""
    server_host: str = ""
    lmgrd_port: int = 0
    vendor_daemon: str = ""
    status: str = ""  # UP / DOWN / UNSUPPORTED
    features: list = field(default_factory=list)
    users: list = field(default_factory=list)
    parse_time: str = ""
    parse_error: str = ""

    @property
    def total_licenses(self) -> int:
        return sum(f.total for f in self.features)

    @property
    def total_used(self) -> int:
        return sum(f.used for f in self.features)

    @property
    def overall_usage_ratio(self) -> float:
        t = self.total_licenses
        return self.total_used / t if t > 0 else 0.0


# ---------------------------------------------------------------------------
# 正则表达式（按 lmutil 输出顺序排列）
# ---------------------------------------------------------------------------

# "License Server status: port on hostname"
RE_SERVER_STATUS = re.compile(
    r"License Server\s+status:\s*(\d+)\s+on\s+(\S+)"
)

# "    vendor daemon status: port hostname: UP"
RE_VENDOR_DAEMON = re.compile(
    r"vendor daemon status:\s*(\d+)\s+(\S+):\s*(\S+)"
)

# "Feature version vendor expiry seats used over"
# "  vcs    2023.06 synopsys 01-jan-00 20     12    "
# "  vcs    v2023.06 synopsys 01-jan-00 20     12    "
RE_FEATURE_LINE = re.compile(
    r"^\s+(\S+)\s+(?:v?[\d.]+\s+)?(\S+)\s+"
    r"(?:[\d]+-[\w]+-[\d]+|lifetime)\s+"
    r"(\d+)\s+(\d+)(?:\s+\d+)?"
)

# 用户行：
# "  username hostname display start_time [feature(s)]"
# "  john   host1:0 /dev/pts/0  vcs (v2023.06) (server:06/01/2024 10:23), 4 licenses"
RE_USER_LINE = re.compile(
    r"^\s+(\S+)\s+(\S+:\d+|\S+)"
    r"(?:\s+(\S+))?\s+"
    r"(\d{2}/\d{2}/\d{4}\s+\d{2}:\d{2})"
    r"(?:\s+\((\S+)\))?"
)

# "Users of FEATURE:  (Total of 20 licenses issued;  Total of 12 licenses in use)"
RE_FEATURE_USERS_OF = re.compile(
    r"Users of (\S+):\s+"
    r"\(Total of (\d+) licenses issued;\s+"
    r"Total of (\d+) licenses in use\)"
)

# 备用：简写格式  "FEATURE: 20 licenses, 12 in use"
RE_FEATURE_BRIEF = re.compile(
    r"(\S+):\s+(\d+)\s+licen[cs]es?,\s+(\d+)\s+(?:in use|used)"
)


# ---------------------------------------------------------------------------
# 解析函数
# ---------------------------------------------------------------------------

def parse_lmstat_output(output: str) -> ServerStatus:
    """解析 lmutil lmstat -a 的完整输出文本"""
    status = ServerStatus(parse_time=datetime.now().isoformat())
    lines = output.strip().splitlines()

    current_feature: Optional[str] = None
    feature_map: dict[str, FeatureUsage] = {}

    i = 0
    while i < len(lines):
        line = lines[i]

        # --- Server status ---
        m = RE_SERVER_STATUS.search(line)
        if m:
            status.lmgrd_port = int(m.group(1))
            status.server_host = m.group(2)
            i += 1
            continue

        # --- Vendor daemon ---
        m = RE_VENDOR_DAEMON.search(line)
        if m:
            status.vendor_daemon = m.group(2)
            status.status = m.group(3)
            i += 1
            continue

        # --- Feature summary header ---
        m = RE_FEATURE_USERS_OF.search(line)
        if m:
            feat_name = m.group(1)
            total = int(m.group(2))
            used = int(m.group(3))
            if feat_name not in feature_map:
                feature_map[feat_name] = FeatureUsage(name=feat_name)
            feature_map[feat_name].total = total
            feature_map[feat_name].used = used
            current_feature = feat_name
            i += 1
            continue

        # --- Feature detail line (alternative format) ---
        m = RE_FEATURE_BRIEF.match(line)
        if m:
            feat_name = m.group(1)
            total = int(m.group(2))
            used = int(m.group(3))
            if feat_name not in feature_map:
                feature_map[feat_name] = FeatureUsage(name=feat_name)
            feature_map[feat_name].total = total
            feature_map[feat_name].used = used
            i += 1
            continue

        # --- Feature usage detail line ---
        m = RE_FEATURE_LINE.match(line)
        if m:
            feat_name = m.group(1)
            vendor = m.group(2)
            total = int(m.group(3))
            used = int(m.group(4))
            if feat_name not in feature_map:
                feature_map[feat_name] = FeatureUsage(name=feat_name)
            feature_map[feat_name].vendor = vendor
            feature_map[feat_name].total = total
            feature_map[feat_name].used = used
            current_feature = feat_name
            i += 1
            continue

        # --- User line ---
        m = RE_USER_LINE.match(line)
        if m:
            user = UserUsage(
                username=m.group(1),
                host=m.group(2),
                display=m.group(3) or "",
                start_time=m.group(4) or "",
                feature=m.group(5) or current_feature or "",
            )
            status.users.append(user)
            if current_feature and current_feature in feature_map:
                feature_map[current_feature].users.append(user)
            i += 1
            continue

        i += 1

    status.features = list(feature_map.values())
    return status


# ---------------------------------------------------------------------------
# lmutil 执行
# ---------------------------------------------------------------------------

def run_lmutil(
    host: str,
    port: int = 27000,
    lmutil_path: str = "",
    timeout: int = 15,
) -> str:
    """执行 lmutil lmstat -a 并返回输出文本"""
    lmutil_cmd = lmutil_path if lmutil_path else "lmutil"
    cmd = [lmutil_cmd, "lmstat", "-a", "-c", f"{port}@{host}"]

    logger.info("执行: %s", " ".join(cmd))

    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        # lmutil 有时返回非零退出码但仍有有效输出
        if result.stdout:
            return result.stdout
        if result.stderr:
            logger.warning("lmutil stderr: %s", result.stderr[:500])
        return ""
    except FileNotFoundError:
        logger.error(
            "找不到 lmutil，请确认已安装 FlexLM 或在 config.yaml 中配置 lmutil_path"
        )
        return ""
    except subprocess.TimeoutExpired:
        logger.error("lmutil 执行超时（%ds），License Server 可能不可达", timeout)
        return ""
    except Exception as e:
        logger.error("lmutil 执行异常: %s", e)
        return ""


def collect_server(
    host: str,
    port: int = 27000,
    lmutil_path: str = "",
    timeout: int = 15,
    retries: int = 3,
) -> ServerStatus:
    """采集单台 License Server 的使用数据，带重试"""
    last_error = ServerStatus(
        server_host=host,
        lmgrd_port=port,
        status="DOWN",
        parse_error="采集失败",
        parse_time=datetime.now().isoformat(),
    )

    for attempt in range(1, retries + 1):
        output = run_lmutil(host, port, lmutil_path, timeout)
        if output:
            status = parse_lmstat_output(output)
            status.server_host = host
            status.lmgrd_port = port
            if not status.parse_error:
                return status
            last_error = status

        logger.warning(
            "第 %d/%d 次采集失败 (host=%s:%d)",
            attempt, retries, host, port,
        )

    return last_error
