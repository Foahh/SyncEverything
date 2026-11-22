import { cn } from "@/lib/utils";
import { Handle, Position, type NodeProps } from "@xyflow/react";
import { GitCommitHorizontal } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { RelayFlowNode } from "../../lib/dagLayout";

export function ArtifactNode({ data, selected }: NodeProps<RelayFlowNode>) {
  const { t } = useTranslation();
  const { node } = data;
  if (node.payload.kind !== "artifact") return null;

  const shortSha = node.payload.git_commit_sha.slice(0, 7);

  return (
    <div
      className={cn(
        "w-[248px] rounded-xl border bg-card px-4 py-3.5 text-card-foreground transition-all duration-150",
        selected
          ? "border-sky-500 ring-2 ring-sky-500/20 shadow-md"
          : "border-border shadow-xs hover:border-foreground/20 hover:shadow-sm",
      )}
    >
      <div className="flex items-center justify-between gap-2">
        <span className="inline-flex items-center gap-1.5 text-[11px] font-medium text-sky-600 dark:text-sky-400">
          <GitCommitHorizontal className="size-3.5" />
          {t("canvas.kind.artifact")}
        </span>
        <span className="font-mono-tech rounded-md bg-muted px-1.5 py-0.5 text-[11px] text-muted-foreground">
          {shortSha}
        </span>
      </div>

      <h4 className="mt-1.5 text-[13px] font-semibold leading-snug text-foreground">
        {node.label}
      </h4>

      <div className="mt-2 flex items-center justify-between text-[11.5px] text-muted-foreground">
        <span>{t("canvas.filesCount", { count: node.payload.changed_files.length })}</span>
        {node.payload.working_tree_dirty && (
          <span className="inline-flex items-center gap-1 font-medium text-amber-600 dark:text-amber-400">
            <span className="size-1.5 rounded-full bg-amber-500" />
            {t("canvas.dirtyBadge")}
          </span>
        )}
      </div>

      <Handle
        type="target"
        position={Position.Left}
        className="!size-2 !border-2 !border-card !bg-sky-500"
      />
      <Handle
        type="source"
        position={Position.Right}
        className="!size-2 !border-2 !border-card !bg-emerald-500"
      />
    </div>
  );
}
