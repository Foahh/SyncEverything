import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";
import { Handle, Position, type NodeProps } from "@xyflow/react";
import { AlertCircle, BarChart3 } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { GraphNode, JsonValue } from "../../bindings";
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

function freshnessBadgeStyle(status: GraphNode["freshness"]["status"]): {
  variant: "destructive" | "secondary";
  className?: string;
} {
  if (status === "stale") {
    return { variant: "destructive" };
  }
  if (status === "pending_verify") {
    return {
      variant: "secondary",
      className: "bg-amber-500/15 text-amber-700 dark:bg-amber-500/20 dark:text-amber-300",
    };
  }
  return {
    variant: "secondary",
    className: "bg-emerald-500/12 text-emerald-700 dark:bg-emerald-500/20 dark:text-emerald-300",
  };
}

function evidenceCardStyle(isStale: boolean, selected: boolean): string {
  if (isStale) {
    return cn(
      "relay-node-stale border-destructive/70 bg-[var(--relay-stale-soft)]",
      selected && "ring-2 ring-destructive/30",
    );
  }
  return cn(
    "bg-card",
    selected
      ? "border-emerald-500 ring-2 ring-emerald-500/20 shadow-md"
      : "border-border shadow-xs hover:border-foreground/20 hover:shadow-sm",
  );
}

export function EvidenceNode({ data, selected }: NodeProps<RelayFlowNode>) {
  const { t } = useTranslation();
  const { node } = data;
  if (node.payload.kind !== "evidence") return null;

  const isStale = node.freshness.status === "stale";
  const metrics = extractHeadlineMetrics(node.payload.metrics_json);
  const badge = freshnessBadgeStyle(node.freshness.status);

  return (
    <div
      className={cn(
        "w-[264px] rounded-lg border px-4 py-3.5 transition-all duration-200",
        evidenceCardStyle(isStale, Boolean(selected)),
      )}
    >
      <div className="flex items-center justify-between gap-2">
        <span
          className={cn(
            "inline-flex items-center gap-1.5 text-[11px] font-medium",
            isStale ? "text-destructive" : "text-emerald-700 dark:text-emerald-400",
          )}
        >
          <BarChart3 className="size-3.5 shrink-0" />
          <span className="relay-text-trim leading-none">{t("canvas.kind.evidence")}</span>
        </span>

        <Badge variant={badge.variant} className={badge.className}>
          <span className="relay-text-trim leading-none">
            {t(`canvas.status.${node.freshness.status}`)}
          </span>
        </Badge>
      </div>

      <h4 className="mt-1.5 text-[13px] font-semibold leading-snug text-foreground">
        {node.label}
      </h4>

      {metrics.length > 0 && (
        <div className="mt-2.5 grid grid-cols-2 gap-2 rounded-md bg-muted/80 px-3 py-2">
          {metrics.map(([label, val]) => (
            <div key={label} className="min-w-0">
              <div className="truncate text-[10.5px] text-muted-foreground">{label}</div>
              <div
                className={cn(
                  "font-mono-tech truncate text-sm font-semibold",
                  isStale
                    ? "text-destructive/75 line-through decoration-destructive/80"
                    : "text-foreground",
                )}
              >
                {val}
              </div>
            </div>
          ))}
        </div>
      )}

      {isStale && (
        <div className="mt-2 flex items-center gap-1.5 text-[11px] font-medium text-destructive">
          <AlertCircle className="size-3.5 shrink-0" />
          <span className="relay-text-trim leading-none">{t("canvas.outdatedHint")}</span>
        </div>
      )}

      <Handle
        type="target"
        position={Position.Left}
        className={cn(
          "!size-2 !border-2 !border-card",
          isStale ? "!bg-destructive" : "!bg-emerald-500",
        )}
      />
      <Handle
        type="source"
        position={Position.Right}
        className="!size-2 !border-2 !border-card !bg-muted-foreground"
      />
    </div>
  );
}
