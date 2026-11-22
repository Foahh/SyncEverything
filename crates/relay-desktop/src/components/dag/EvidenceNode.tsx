import { Chip } from "@heroui/react";
import { Handle, Position, type NodeProps } from "@xyflow/react";
import { AlertCircle, BarChart3 } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { JsonValue } from "../../bindings";
import type { RelayFlowNode } from "../../lib/dagLayout";

const ACRONYMS = new Set(["mae", "rmse", "mse", "r2", "ms", "cpu", "gpu", "rtt"]);

function formatMetricLabel(key: string): string {
  return key
    .split("_")
    .map((word) =>
      ACRONYMS.has(word.toLowerCase())
        ? word.toUpperCase()
        : word.charAt(0).toUpperCase() + word.slice(1),
    )
    .join(" ");
}

function extractHeadlineMetrics(metrics: JsonValue): Array<[string, string]> {
  if (!metrics || typeof metrics !== "object" || Array.isArray(metrics)) {
    return [];
  }
  return Object.entries(metrics)
    .slice(0, 2)
    .map(([key, val]) => [
      formatMetricLabel(key),
      typeof val === "number" ? String(val) : JSON.stringify(val),
    ]);
}

export function EvidenceNode({ data, selected }: NodeProps<RelayFlowNode>) {
  const { t } = useTranslation();
  const { node } = data;
  if (node.payload.kind !== "evidence") return null;

  const isStale = node.freshness.status === "stale";
  const isPending = node.freshness.status === "pending_verify";
  const metrics = extractHeadlineMetrics(node.payload.metrics_json);

  return (
    <div
      className={[
        "w-[264px] rounded-2xl border px-4 py-3.5 transition-all duration-200",
        isStale
          ? "relay-node-stale border-red-400/80 bg-[var(--relay-stale-soft)]"
          : "bg-[var(--relay-surface)]",
        !isStale && selected
          ? "border-teal-500 ring-2 ring-teal-500/20 shadow-md"
          : !isStale
            ? "border-[var(--relay-border)] shadow-xs hover:border-[var(--relay-border-strong)] hover:shadow-sm"
            : selected
              ? "ring-2 ring-red-500/30"
              : "",
      ].join(" ")}
    >
      <div className="flex items-center justify-between gap-2">
        <span
          className={[
            "inline-flex items-center gap-1.5 text-[11px] font-medium",
            isStale ? "text-red-600 dark:text-red-400" : "text-teal-700 dark:text-teal-400",
          ].join(" ")}
        >
          <BarChart3 className="h-3.5 w-3.5" />
          {t("canvas.kind.evidence")}
        </span>

        <Chip
          size="sm"
          variant={isStale ? "primary" : "soft"}
          color={isStale ? "danger" : isPending ? "warning" : "success"}
          className="h-5 px-2 text-[10.5px] font-medium"
        >
          {t(`canvas.status.${node.freshness.status}`)}
        </Chip>
      </div>

      <h4 className="mt-1.5 text-[13.5px] font-semibold leading-snug text-[var(--relay-text)]">
        {node.label}
      </h4>

      {metrics.length > 0 && (
        <div className="mt-2.5 grid grid-cols-2 gap-2 rounded-xl bg-[var(--relay-surface-subtle)]/90 px-3 py-2">
          {metrics.map(([label, val]) => (
            <div key={label} className="min-w-0">
              <div className="truncate text-[10.5px] text-[var(--relay-text-faint)]">{label}</div>
              <div
                className={[
                  "font-mono-tech truncate text-[14px] font-semibold",
                  isStale
                    ? "line-through decoration-red-500/80 text-red-700/70 dark:text-red-300/70"
                    : "text-[var(--relay-text)]",
                ].join(" ")}
              >
                {val}
              </div>
            </div>
          ))}
        </div>
      )}

      {isStale && (
        <div className="mt-2 flex items-center gap-1.5 text-[11px] font-medium text-red-600 dark:text-red-400">
          <AlertCircle className="h-3.5 w-3.5 shrink-0" />
          <span>{t("canvas.outdatedHint")}</span>
        </div>
      )}

      <Handle
        type="target"
        position={Position.Left}
        className={[
          "!h-2 !w-2 !border-2 !border-[var(--relay-surface)]",
          isStale ? "!bg-red-500" : "!bg-teal-500",
        ].join(" ")}
      />
      <Handle
        type="source"
        position={Position.Right}
        className="!h-2 !w-2 !border-2 !border-[var(--relay-surface)] !bg-slate-400"
      />
    </div>
  );
}
