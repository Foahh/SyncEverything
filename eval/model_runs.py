"""Choose comparable live runs for the model report and its paper figure."""

import json
from pathlib import Path
from typing import Any


SCORING_VERSION = 2


def load_model_runs(directory: Path) -> list[dict[str, Any]]:
    current, historical = [], []
    for path in sorted(directory.glob("*.json")):
        run = json.loads(path.read_text(encoding="utf-8"))
        if run.get("dry_run"):
            continue
        version = run.get("scoring_version", 1)
        if version == SCORING_VERSION:
            current.append(run)
        elif version == 1:
            historical.append(run)
    return current or historical
