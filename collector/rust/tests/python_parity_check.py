"""开发辅助工具：打印 Python 版解析器对 tests/fixtures 的输出。

用于校验 Rust 版 `src/lmutil.rs` 的解析结果与 Python 版 `lmutil_parser.py` 一致。

用法（在仓库根目录执行）：

    python collector/rust/tests/python_parity_check.py
"""

import importlib.util
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
FIXTURES = Path(__file__).resolve().parent / "fixtures"

spec = importlib.util.spec_from_file_location(
    "lmutil_parser", ROOT / "collector" / "python" / "lmutil_parser.py"
)
parser = importlib.util.module_from_spec(spec)
spec.loader.exec_module(parser)


def dump(fixture: str) -> dict:
    text = (FIXTURES / fixture).read_text(encoding="utf-8")
    status = parser.parse_lmstat_output(text)
    return {
        "server_host": status.server_host,
        "lmgrd_port": status.lmgrd_port,
        "vendor_daemon": status.vendor_daemon,
        "status": status.status,
        "total_licenses": status.total_licenses,
        "total_used": status.total_used,
        "features": [
            {
                "name": f.name,
                "vendor": f.vendor,
                "total": f.total,
                "used": f.used,
                "users": [
                    {
                        "username": u.username,
                        "host": u.host,
                        "display": u.display,
                        "start_time": u.start_time,
                        "feature": u.feature,
                    }
                    for u in f.users
                ],
            }
            for f in status.features
        ],
        "users": [u.username for u in status.users],
        "parse_error": status.parse_error,
    }


if __name__ == "__main__":
    for name in ("lmstat_full.txt", "lmstat_table.txt", "lmstat_connect_error.txt"):
        print(f"### {name}")
        print(json.dumps(dump(name), indent=2, ensure_ascii=False))
