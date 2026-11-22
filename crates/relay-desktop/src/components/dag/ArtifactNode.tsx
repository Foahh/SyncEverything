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
      className={[
        "w-[248px] rounded-2xl border bg-[var(--relay-surface)] px-4 py-3.5 transition-all duration-150",
        selected
          ? "border-sky-500 ring-2 ring-sky-500/20 shadow-md"
          : "border-[var(--relay-border)] shadow-xs hover:border-[var(--relay-border-strong)] hover:shadow-sm",
      ].join(" ")}
    >
      <div className="flex items-center justify-between gap-2">
        <span className="inline-flex items-center gap-1.5 text-[11px] font-medium text-sky-600 dark:text-sky-400">
          <GitCommitHorizontal className="h-3.5 w-3.5" />
          {t("canvas.kind.artifact")}
        </span>
        <span className="font-mono-tech rounded-md bg-[var(--relay-surface-subtle)] px-1.5 py-0.5 text-[11px] text-[var(--relay-text-muted)]">
          {shortSha}
        </span>
      </div>

      <h4 className="mt-1.5 text-[13.5px] font-semibold leading-snug text-[var(--relay-text)]">
        {node.label}
      </h4>

      <div className="mt-2 flex items-center justify-between text-[11.5px] text-[var(--relay-text-faint)]">
        <span>{t("canvas.filesCount", { count: node.payload.changed_files.length })}</span>
        {node.payload.working_tree_dirty && (
          <span className="inline-flex items-center gap-1 font-medium text-amber-600 dark:text-amber-400">
            <span className="h-1.5 w-1.5 rounded-full bg-amber-500" />
            {t("canvas.dirtyBadge")}
          </span>
        )}
      </div>

      <Handle
        type="target"
        position={Position.Left}
        className="!h-2 !w-2 !border-2 !border-[var(--relay-surface)] !bg-sky-500"
      />
      <Handle
        type="source"
        position={Position.Right}
        className="!h-2 !w-2 !border-2 !border-[var(--relay-surface)] !bg-teal-500"
      />
    </div>
  );
}
