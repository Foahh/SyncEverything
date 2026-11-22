import { cn } from "@/lib/utils";
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
      className={cn(
        "w-[244px] rounded-lg border bg-card px-4 py-3.5 text-card-foreground transition-all duration-150",
        selected
          ? "border-primary ring-2 ring-primary/20 shadow-md"
          : "border-border shadow-xs hover:border-foreground/20 hover:shadow-sm",
      )}
    >
      <div className="flex items-center gap-1.5 text-[11px] font-medium text-primary">
        <StickyNote className="size-3.5 shrink-0" />
        <span className="relay-text-trim leading-none">{t("canvas.kind.decision")}</span>
      </div>

      <h4 className="mt-1.5 text-[13px] font-semibold leading-snug text-foreground">
        {node.label}
      </h4>

      <p className="mt-1 line-clamp-2 text-xs leading-relaxed text-muted-foreground">
        {node.payload.rationale}
      </p>

      <Handle
        type="target"
        position={Position.Left}
        className="!size-2 !border-2 !border-card !bg-primary/70"
      />
      <Handle
        type="source"
        position={Position.Right}
        className="!size-2 !border-2 !border-card !bg-primary"
      />
    </div>
  );
}
