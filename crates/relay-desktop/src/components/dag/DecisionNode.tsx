import { Chip } from "@heroui/react";
import { Handle, Position, type NodeProps } from "@xyflow/react";
import { Compass, Laptop } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { RelayFlowNode } from "../../lib/dagLayout";

export function DecisionNode({ data, selected }: NodeProps<RelayFlowNode>) {
  const { t } = useTranslation();
  const { node } = data;
  if (node.payload.kind !== "decision") return null;

  return (
    <div
      className={[
        "w-[292px] rounded-xl border bg-[var(--relay-surface)] transition-all duration-200",
        selected
          ? "border-indigo-500 ring-2 ring-indigo-500/20 shadow-lg"
          : "border-[var(--relay-border)] shadow-sm hover:border-[var(--relay-border-strong)]",
      ].join(" ")}
    >
      {/* Precision top accent bar */}
      <div className="h-1 w-full rounded-t-xl bg-gradient-to-r from-indigo-500 via-violet-500 to-sky-500" />

      <div className="p-3.5 space-y-2.5">
        <div className="flex items-center justify-between gap-2">
          <div className="flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-indigo-600 dark:text-indigo-400">
            <Compass className="h-3.5 w-3.5" />
            <span>{t("dag.nodeKind.decision")}</span>
          </div>
          <span className="font-mono-tech inline-flex items-center gap-1 rounded-md bg-[var(--relay-surface-subtle)] px-1.5 py-0.5 text-[10px] text-[var(--relay-text-faint)]">
            <Laptop className="h-2.5 w-2.5" />
            {node.device_origin}
          </span>
        </div>

        <div>
          <h4 className="text-[13px] font-semibold leading-snug text-[var(--relay-text)]">
            {node.label}
          </h4>
          <p className="mt-1 line-clamp-2 text-[11.5px] leading-relaxed text-[var(--relay-text-muted)]">
            {node.payload.rationale}
          </p>
        </div>

        {node.payload.constraints.length > 0 && (
          <div className="flex flex-wrap gap-1 pt-0.5">
            {node.payload.constraints.slice(0, 3).map((constraint) => (
              <Chip
                key={constraint}
                size="sm"
                variant="soft"
                color="default"
                className="font-mono-tech h-5 px-1.5 text-[10px]"
              >
                {constraint}
              </Chip>
            ))}
          </div>
        )}
      </div>

      <Handle
        type="target"
        position={Position.Left}
        className="!h-2.5 !w-2.5 !border-2 !border-[var(--relay-surface)] !bg-indigo-500"
      />
      <Handle
        type="source"
        position={Position.Right}
        className="!h-2.5 !w-2.5 !border-2 !border-[var(--relay-surface)] !bg-indigo-500"
      />
    </div>
  );
}
