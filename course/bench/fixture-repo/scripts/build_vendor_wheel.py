#!/usr/bin/env python3
"""Build the offline vendor wheel used by the missing-dependency Chaos case.

Track 4 must run with no PyPI access, so the fixture ships a tiny pure-Python
wheel built from this script. Regenerating it is deterministic: fixed entry
timestamps and fixed metadata mean the same bytes every run.

Usage:
    uv run --offline python scripts/build_vendor_wheel.py
"""

from __future__ import annotations

import base64
import hashlib
import zipfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
VENDOR_DIR = REPO_ROOT / "vendor"
WHEEL_NAME = "tiny_stats-0.1.0-py3-none-any.whl"

INIT_PY = '''"""Tiny pure-Python statistics helpers (offline vendor fixture)."""

from __future__ import annotations

from typing import Iterable


def mean(values: Iterable[float]) -> float:
    data = list(values)
    if not data:
        raise ValueError("mean() requires at least one value")
    return sum(data) / len(data)


def stdev(values: Iterable[float]) -> float:
    data = list(values)
    if len(data) < 2:
        raise ValueError("stdev() requires at least two values")
    mu = mean(data)
    return (sum((value - mu) ** 2 for value in data) / (len(data) - 1)) ** 0.5
'''

METADATA = """Metadata-Version: 2.1
Name: tiny-stats
Version: 0.1.0
Summary: Offline vendor fixture for the Relay Chaos Suite
Requires-Python: >=3.11
"""

WHEEL = """Wheel-Version: 1.0
Generator: relay-fixture (deterministic)
Root-Is-Purelib: true
Tag: py3-none-any
"""

ENTRY_TIMESTAMP = (1980, 1, 1, 0, 0, 0)


def record_line(path: str, payload: bytes) -> str:
    digest = base64.urlsafe_b64encode(hashlib.sha256(payload).digest()).rstrip(b"=").decode()
    return f"{path},sha256={digest},{len(payload)}"


def write_entry(archive: zipfile.ZipFile, name: str, payload: bytes) -> None:
    info = zipfile.ZipInfo(name, date_time=ENTRY_TIMESTAMP)
    info.external_attr = 0o644 << 16
    info.compress_type = zipfile.ZIP_DEFLATED
    archive.writestr(info, payload)


def main() -> None:
    VENDOR_DIR.mkdir(parents=True, exist_ok=True)
    dist_info = "tiny_stats-0.1.0.dist-info"
    files = {
        "tiny_stats/__init__.py": INIT_PY.encode(),
        f"{dist_info}/METADATA": METADATA.encode(),
        f"{dist_info}/WHEEL": WHEEL.encode(),
    }
    record = "\n".join(record_line(path, payload) for path, payload in files.items())
    record += f"\n{dist_info}/RECORD,,\n"

    target = VENDOR_DIR / WHEEL_NAME
    with zipfile.ZipFile(target, "w") as archive:
        for path, payload in files.items():
            write_entry(archive, path, payload)
        write_entry(archive, f"{dist_info}/RECORD", record.encode())

    digest = hashlib.sha256(target.read_bytes()).hexdigest()
    print(f"wrote {target.relative_to(REPO_ROOT)} sha256={digest}")


if __name__ == "__main__":
    main()
