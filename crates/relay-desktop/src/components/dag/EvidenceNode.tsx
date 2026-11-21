import { Chip } from "@heroui/react";
import { Handle, Position, type NodeProps } from "@xyflow/react";
import { Activity, ShieldAlert, ShieldCheck } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { JsonValue } from "../../bindings";
import type { RelayFlowNode } from "../../lib/dagLayout";

function extractTopMetrics(metrics: JsonValue): Array<[string, string]> {
  if (!metrics || typeof metrics !== "object" || Array.isArray(metrics)) {
    return [];
  }
  return Object.entries(metrics)
    .slice(0, 4)
    .map(([key, val]) => [key, typeof val === "number" ? String(val) : JSON.stringify(val)]);
}

export function EvidenceNode({ data, selected }: NodeProps<RelayFlowNode>) {
  const { t } = useTranslation();
  const { node } = data;
  if (node.payload.kind !== "evidence") return null;

  const isStale = node.freshness.status === "stale";
  const isPending = node.freshness.status === "pending_verify";
  const metrics = extractTopMetrics(node.payload.metrics_json);

  return (
    <div
      className={[
        "w-[316px] rounded-xl border transition-all duration-300",
        isStale
          ? "relay-node-stale border-red-500/80 bg-[var(--relay-stale-soft)]"
          : "bg-[var(--relay-surface)]",
        !isStale && selected
          ? "border-teal-500 ring-2 ring-teal-500/20 shadow-lg"
          : !isStale
            ? "border-[var(--relay-border)] shadow-sm hover:border-[var(--relay-border-strong)]"
            : selected
              ? "ring-2 ring-red-500/35"
              : "",
      ].join(" ")}
    >
      {/* Semantic top edge stripe */}
      <div
        className={[
          "h-1.5 w-full rounded-t-xl transition-colors",
          isStale ? "bg-red-600" : isPending ? "bg-amber-500" : "bg-teal-600",
        ].join(" ")}
      />

      <div className="p-3.5 space-y-2.5">
        <div className="flex items-center justify-between gap-2">
          <div
            className={[
              "flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider",
              isStale ? "text-red-600 dark:text-red-400" : "text-teal-700 dark:text-teal-400",
            ].join(" ")}
          >
            <Activity className="h-3.5 w-3.5" />
            <span>{t("dag.nodeKind.evidence")}</span>
          </div>

          <Chip
            size="sm"
            variant={isStale ? "primary" : "soft"}
            color={isStale ? "danger" : isPending ? "warning" : "success"}
            className="font-mono-tech h-5 px-2 text-[10px] font-semibold tracking-wide"
          >
            {isStale
              ? t("dag.freshness.stale")
              : isPending
                ? t("dag.freshness.pending_verify")
                : t("dag.freshness.fresh")}
          </Chip>
        </div>

        <div>
          <h4 className="text-[13px] font-semibold text-[var(--relay-text)]">{node.label}</h4>
          <p className="font-mono-tech mt-0.5 text-[10.5px] text-[var(--relay-text-faint)]">
            preset:{node.payload.preset} · {node.device_origin}
          </p>
        </div>

        {/* Structured metrics grid */}
        {metrics.length > 0 && (
          <div className="grid grid-cols-2 gap-1.5 rounded-lg border border-[var(--relay-border)] bg-[var(--relay-surface)]/80 p-2">
            {metrics.map(([k, v]) => (
              <div key={k} className="min-w-0">
                <div className="truncate text-[10px] text-[var(--relay-text-faint)]">{k}</div>
                <div
                  className={[
                    "font-mono-tech truncate text-[12px] font-semibold",
                    isStale
                      ? "line-through decoration-red-500/80 text-red-700/75 dark:text-red-300/75"
                      : "text-[var(--relay-text)]",
                  ].join(" ")}
                >
                  {v}
                </div>
              </div>
            ))}
          </div>
        )}

        {/* Deterministic anti-hallucination guard footer */}
        <div
          className={[
            "flex items-center justify-between rounded-lg px-2 py-1.5 text-[10.5px] font-medium",
            isStale
              ? "bg-red-600/10 text-red-700 dark:bg-red-500/15 dark:text-red-300"
              : "bg-teal-600/8 text-teal-700 dark:bg-teal-500/10 dark:text-teal-300",
          ].join(" ")}
        >
          <span className="inline-flex items-center gap-1.5">
            {isStale ? (
              <>
                <ShieldAlert className="h-3.5 w-3.5 shrink-0" />
                {t("dag.citationBlocked")}
              </>
            ) : (
              <>
                <ShieldCheck className="h-3.5 w-3.5 shrink-0" />
                {node.payload.log_blob_hash.slice(0, 18)}
              </>
            )}
          </span>
        </div>
      </div>

      <Handle
        type="target"
        position={Position.Left}
        className={[
          "!h-2.5 !w-2.5 !border-2 !border-[var(--relay-surface)]",
          isStale ? "!bg-red-600" : "!bg-teal-600",
        ].join(" ")}
      />
      <Handle
        type="source"
        position={Position.Right}
        className="!h-2.5 !w-2.5 !border-2 !border-[var(--relay-surface)] !bg-slate-400"
      />
    </div>
  );
}
