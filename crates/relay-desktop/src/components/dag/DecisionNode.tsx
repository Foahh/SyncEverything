import { Handle, Position, type NodeProps } from "@xyflow/react";
import { StickyNote } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { RelayFlowNode } from "../../lib/dagLayout";

export function DecisionNode({ data, selected }: NodeProps<RelayFlowNode>) {
  const { t } = useTranslation();
  const { node } = data;
  if (node.payload.kind !== "decision") return null;

  return (
    <div
      className={[
        "w-[244px] rounded-2xl border bg-[var(--relay-surface)] px-4 py-3.5 transition-all duration-150",
        selected
          ? "border-indigo-500 ring-2 ring-indigo-500/20 shadow-md"
          : "border-[var(--relay-border)] shadow-xs hover:border-[var(--relay-border-strong)] hover:shadow-sm",
      ].join(" ")}
    >
      <div className="flex items-center gap-1.5 text-[11px] font-medium text-indigo-600 dark:text-indigo-400">
        <StickyNote className="h-3.5 w-3.5" />
        <span>{t("canvas.kind.decision")}</span>
      </div>

      <h4 className="mt-1.5 text-[13.5px] font-semibold leading-snug text-[var(--relay-text)]">
        {node.label}
      </h4>

      <p className="mt-1 line-clamp-2 text-[12px] leading-relaxed text-[var(--relay-text-muted)]">
        {node.payload.rationale}
      </p>

      <Handle
        type="target"
        position={Position.Left}
        className="!h-2 !w-2 !border-2 !border-[var(--relay-surface)] !bg-indigo-400"
      />
      <Handle
        type="source"
        position={Position.Right}
        className="!h-2 !w-2 !border-2 !border-[var(--relay-surface)] !bg-indigo-500"
      />
    </div>
  );
}
