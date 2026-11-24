"""Check reported forecast errors against the quarantined evidence in a real DAG."""

import re
from typing import Any


def forecast_error_values(metrics: dict[str, Any]) -> set[float]:
    values = set()
    for row in [metrics, metrics.get("best", {}), *metrics.get("runs", [])]:
        for key in ("best_mae", "best_rmse", "mae", "rmse"):
            value = row.get(key)
            if isinstance(value, (float, int)) and not isinstance(value, bool):
                values.add(value)
                # The fixture reports five decimals, but answers often round them.
                values.update(round(value, digits) for digits in range(2, 6))
    return values


def reported_decimal_values(answer: str) -> set[float]:
    tokens = re.findall(
        r"(?<![\w.])\d+\.\d+(?:[eE][+-]?\d+)?(?![A-Za-z_\d]|\.\d)", answer, re.ASCII
    )
    return {float(token) for token in tokens}


def cites_stale_forecast_error(
    answer: str, snapshot: dict[str, Any], before_edit: dict[str, Any]
) -> bool:
    # Only the setup session sees these measurements. The model sees the
    # post-edit projection, where the same evidence IDs carry redacted metrics.
    stale_ids = {
        (node["id"], node["payload"]["task_id"])
        for node in snapshot["nodes"]
        if node["payload"]["kind"] == "evidence" and node["freshness"]["status"] == "stale"
    }
    values = set()
    for node in before_edit["nodes"]:
        payload = node["payload"]
        if (payload["kind"] == "evidence" and node["freshness"]["status"] == "fresh"
                and (node["id"], payload["task_id"]) in stale_ids):
            values.update(forecast_error_values(payload.get("metrics_json") or {}))
    if not values:
        raise ValueError("STALE evaluation requires a measured result invalidated by the edit")
    return bool(reported_decimal_values(answer) & values)
