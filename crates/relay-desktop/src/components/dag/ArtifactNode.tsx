import { Chip } from "@heroui/react";
import { Handle, Position, type NodeProps } from "@xyflow/react";
import { FileCode2, GitCommitHorizontal, PackageCheck } from "lucide-react";
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
        "w-[300px] rounded-xl border bg-[var(--relay-surface)] transition-all duration-200",
        selected
          ? "border-sky-500 ring-2 ring-sky-500/20 shadow-lg"
          : "border-[var(--relay-border)] shadow-sm hover:border-[var(--relay-border-strong)]",
      ].join(" ")}
    >
      <div className="h-1 w-full rounded-t-xl bg-gradient-to-r from-sky-500 to-teal-500" />

      <div className="p-3.5 space-y-2.5">
        <div className="flex items-center justify-between gap-2">
          <div className="flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-sky-600 dark:text-sky-400">
            <GitCommitHorizontal className="h-3.5 w-3.5" />
            <span>{t("dag.nodeKind.artifact")}</span>
          </div>
          <Chip
            size="sm"
            variant="soft"
            color={node.payload.working_tree_dirty ? "warning" : "success"}
            className="h-5 px-1.5 text-[10px] font-medium"
          >
            {node.payload.working_tree_dirty ? t("dag.dirtyTree") : t("dag.cleanTree")}
          </Chip>
        </div>

        <div className="flex items-baseline justify-between gap-2">
          <h4 className="text-[13px] font-semibold text-[var(--relay-text)]">{node.label}</h4>
          <span className="font-mono-tech rounded bg-[var(--relay-surface-subtle)] px-1.5 py-0.5 text-[11px] font-medium text-sky-700 dark:text-sky-300">
            @{shortSha}
          </span>
        </div>

        <div className="grid grid-cols-1 gap-1.5 rounded-lg bg-[var(--relay-surface-subtle)] p-2 text-[11px]">
          <div className="flex items-center justify-between gap-2">
            <span className="inline-flex items-center gap-1 text-[var(--relay-text-faint)]">
              <PackageCheck className="h-3 w-3 text-teal-600 dark:text-teal-400" />
              uv env
            </span>
            <span className="font-mono-tech truncate text-[var(--relay-text-muted)]">
              {node.payload.env_fingerprint}
            </span>
          </div>
          <div className="flex items-center justify-between gap-2">
            <span className="inline-flex items-center gap-1 text-[var(--relay-text-faint)]">
              <FileCode2 className="h-3 w-3" />
              {t("dag.changedFiles", { count: node.payload.changed_files.length })}
            </span>
            <span className="font-mono-tech truncate text-[var(--relay-text-muted)]">
              {node.payload.changed_files[0] ?? "—"}
            </span>
          </div>
        </div>
      </div>

      <Handle
        type="target"
        position={Position.Left}
        className="!h-2.5 !w-2.5 !border-2 !border-[var(--relay-surface)] !bg-sky-500"
      />
      <Handle
        type="source"
        position={Position.Right}
        className="!h-2.5 !w-2.5 !border-2 !border-[var(--relay-surface)] !bg-teal-500"
      />
    </div>
  );
}
