"""Choose comparable live runs for the model report and its paper figure."""

import json
from pathlib import Path
from typing import Any


SCORING_VERSION = 4


def latest_scoring_version(directory: Path) -> int:
    versions = [
        int(run.get("scoring_version", 1))
        for path in sorted(directory.glob("*.json"))
        for run in (json.loads(path.read_text(encoding="utf-8")),)
        if not run.get("dry_run")
    ]
    return max(versions) if versions else SCORING_VERSION


def load_model_runs(directory: Path, scoring_version: int = SCORING_VERSION) -> list[dict[str, Any]]:
    runs = []
    configs = {}
    for path in sorted(directory.glob("*.json")):
        run = json.loads(path.read_text(encoding="utf-8"))
        if run.get("dry_run"):
            continue
        if run.get("scoring_version", 1) == scoring_version:
            label = run["model"]
            config = (run["base_url"], run.get("endpoint_model", label), run.get("reasoning_effort"))
            if configs.setdefault(label, config) != config:
                raise ValueError(f"Multiple endpoint/model/reasoning configurations share label {label!r}")
            runs.append(run)
    return runs
