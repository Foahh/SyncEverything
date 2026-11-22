import { Button, Chip } from "@heroui/react";
import { AlertCircle, CheckCircle2, ChevronRight, ShieldCheck, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { ApprovalDecision, GraphNode, JsonValue, TaskRecord_Serialize } from "../bindings";

interface InspectorDrawerProps {
  mode: "closed" | "node" | "fix";
  selectedNode: GraphNode | null;
  pendingFixTask: TaskRecord_Serialize | null;
  onResolveApproval: (taskId: string, decision: ApprovalDecision) => void;
  isResolvingApproval: boolean;
  onClose: () => void;
}

function UnifiedDiffViewer({ diff }: { diff: string }) {
  const lines = diff.split("\n");
  return (
    <div className="font-mono-tech overflow-x-auto rounded-xl border border-[var(--relay-border)] bg-[#0e131b] text-[11.5px] leading-relaxed text-slate-200">
      <div className="p-2.5 space-y-0.5">
        {lines.map((line, index) => {
          const isAdd = line.startsWith("+") && !line.startsWith("+++");
          const isDel = line.startsWith("-") && !line.startsWith("---");
          const isHunk = line.startsWith("@@");
          const isHeader = line.startsWith("---") || line.startsWith("+++");

          return (
            <div
              key={`${index}-${line.slice(0, 12)}`}
              className={[
                "flex gap-2.5 rounded px-1.5 whitespace-pre",
                isAdd
                  ? "bg-emerald-500/18 text-emerald-300"
                  : isDel
                    ? "bg-red-500/18 text-red-300"
                    : isHunk
                      ? "bg-indigo-500/20 text-indigo-300"
                      : isHeader
                        ? "text-slate-400"
                        : "text-slate-300",
              ].join(" ")}
            >
              <span className="w-5 shrink-0 select-none text-right text-slate-500">
                {index + 1}
              </span>
              <span>{line}</span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

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

function metricEntries(metrics: JsonValue): Array<[string, string]> {
  if (!metrics || typeof metrics !== "object" || Array.isArray(metrics)) return [];
  return Object.entries(metrics).map(([k, v]) => [
    formatMetricLabel(k),
    typeof v === "number" ? String(v) : JSON.stringify(v),
  ]);
}

export function InspectorDrawer({
  mode,
  selectedNode,
  pendingFixTask,
  onResolveApproval,
  isResolvingApproval,
  onClose,
}: InspectorDrawerProps) {
  const { t } = useTranslation();

  if (mode === "closed") return null;

  const proposal = pendingFixTask?.active_proposal ?? null;
  const isFixMode = mode === "fix" && proposal != null && pendingFixTask != null;

  if (!isFixMode && !selectedNode) return null;

  return (
    <aside className="flex h-full w-[380px] shrink-0 flex-col border-l border-[var(--relay-border)] bg-[var(--relay-surface)] shadow-xl z-20">
      {/* Clean single-purpose header */}
      <div className="flex items-center justify-between gap-2 border-b border-[var(--relay-border)] px-4 py-3">
        <div className="min-w-0">
          <div className="text-[11px] font-medium text-[var(--relay-text-faint)]">
            {isFixMode ? t("detail.fixTitle") : t(`canvas.kind.${selectedNode!.payload.kind}`)}
          </div>
          <h3 className="truncate text-[14.5px] font-semibold text-[var(--relay-text)]">
            {isFixMode ? pendingFixTask.spec.command_preset : selectedNode!.label}
          </h3>
        </div>

        <Button
          size="sm"
          variant="ghost"
          isIconOnly
          aria-label={t("detail.close")}
          onPress={onClose}
          className="h-7 w-7 min-w-7 shrink-0"
        >
          <X className="h-4 w-4" />
        </Button>
      </div>

      {/* Body */}
      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        {isFixMode ? (
          <>
            <p className="text-[13px] leading-relaxed text-[var(--relay-text)]">
              {proposal.summary}
            </p>

            <div className="flex items-center gap-2 rounded-xl border border-teal-500/25 bg-[var(--relay-fresh-soft)] px-3 py-2 text-[12px] text-teal-800 dark:text-teal-200">
              <ShieldCheck className="h-4 w-4 shrink-0 text-teal-600 dark:text-teal-400" />
              <span>{t("detail.fixSafeNote")}</span>
            </div>

            <div className="space-y-1.5">
              <div className="text-[11.5px] font-medium text-[var(--relay-text-muted)]">
                {t("detail.fixChanges")}
              </div>
              <UnifiedDiffViewer diff={proposal.diff_preview} />
            </div>

            {/* Progressive disclosure for pre-check stdout */}
            <details className="group rounded-xl border border-[var(--relay-border)] bg-[var(--relay-surface-subtle)]/60">
              <summary className="flex cursor-pointer list-none items-center justify-between px-3 py-2 text-[12px] font-medium text-[var(--relay-text)]">
                <span className="inline-flex items-center gap-1.5">
                  <CheckCircle2 className="h-3.5 w-3.5 text-teal-600 dark:text-teal-400" />
                  {proposal.smoke_verified ? t("detail.fixCheckPassed") : t("detail.fixCheckLog")}
                </span>
                <ChevronRight className="h-3.5 w-3.5 text-[var(--relay-text-faint)] transition-transform group-open:rotate-90" />
              </summary>
              <pre className="font-mono-tech border-t border-[var(--relay-border)] whitespace-pre-wrap break-words px-3 py-2.5 text-[11px] leading-relaxed text-[var(--relay-text-muted)]">
                {proposal.smoke_stdout_tail}
              </pre>
            </details>
          </>
        ) : (
          selectedNode && (
            <>
              {/* Outdated explanation */}
              {selectedNode.freshness.status === "stale" && (
                <div className="rounded-xl border border-red-500/30 bg-[var(--relay-stale-soft)] p-3 text-[12px] leading-relaxed text-red-700 dark:text-red-300">
                  <div className="flex items-center gap-1.5 font-semibold">
                    <AlertCircle className="h-3.5 w-3.5" />
                    {t("detail.outdatedTitle")}
                  </div>
                  <p className="mt-1 opacity-90">
                    {t("detail.outdatedDesc", { reason: selectedNode.freshness.reason })}
                  </p>
                </div>
              )}

              {/* Decision / Note */}
              {selectedNode.payload.kind === "decision" && (
                <div className="space-y-4">
                  <div className="space-y-1.5">
                    <div className="text-[11.5px] font-medium text-[var(--relay-text-faint)]">
                      {t("detail.why")}
                    </div>
                    <p className="text-[13px] leading-relaxed text-[var(--relay-text)]">
                      {selectedNode.payload.rationale}
                    </p>
                  </div>

                  {selectedNode.payload.constraints.length > 0 && (
                    <div className="space-y-1.5">
                      <div className="text-[11.5px] font-medium text-[var(--relay-text-faint)]">
                        {t("detail.rules")}
                      </div>
                      <div className="flex flex-wrap gap-1.5">
                        {selectedNode.payload.constraints.map((rule) => (
                          <Chip key={rule} size="sm" variant="soft" className="text-[11.5px]">
                            {rule}
                          </Chip>
                        ))}
                      </div>
                    </div>
                  )}
                </div>
              )}

              {/* Artifact / Code Version */}
              {selectedNode.payload.kind === "artifact" && (
                <div className="space-y-3">
                  <div className="text-[11.5px] font-medium text-[var(--relay-text-faint)]">
                    {t("detail.files")}
                  </div>
                  <ul className="space-y-1">
                    {selectedNode.payload.changed_files.map((file) => (
                      <li
                        key={file}
                        className="font-mono-tech rounded-lg border border-[var(--relay-border)] bg-[var(--relay-surface-subtle)] px-2.5 py-1.5 text-[12px] text-[var(--relay-text)]"
                      >
                        {file}
                      </li>
                    ))}
                  </ul>
                </div>
              )}

              {/* Evidence / Run Result */}
              {selectedNode.payload.kind === "evidence" && (
                <div className="space-y-3">
                  <div className="text-[11.5px] font-medium text-[var(--relay-text-faint)]">
                    {t("detail.metrics")}
                  </div>
                  <div className="grid grid-cols-2 gap-2">
                    {metricEntries(selectedNode.payload.metrics_json).map(([label, val]) => (
                      <div
                        key={label}
                        className="rounded-xl border border-[var(--relay-border)] bg-[var(--relay-surface-subtle)]/70 p-2.5"
                      >
                        <div className="text-[11px] text-[var(--relay-text-faint)]">{label}</div>
                        <div className="font-mono-tech mt-0.5 text-[14px] font-semibold text-[var(--relay-text)]">
                          {val}
                        </div>
                      </div>
                    ))}
                  </div>
                </div>
              )}

              {/* Collapsible Technical Details (hashes, full SHA, raw JSON) */}
              <details className="group rounded-xl border border-[var(--relay-border)] bg-[var(--relay-surface-subtle)]/50">
                <summary className="flex cursor-pointer list-none items-center justify-between px-3 py-2 text-[11.5px] font-medium text-[var(--relay-text-muted)]">
                  <span>{t("detail.techDetails")}</span>
                  <ChevronRight className="h-3.5 w-3.5 transition-transform group-open:rotate-90" />
                </summary>
                <div className="space-y-2 border-t border-[var(--relay-border)] px-3 py-2.5 text-[11.5px]">
                  <div className="flex justify-between gap-2">
                    <span className="text-[var(--relay-text-faint)]">{t("detail.device")}</span>
                    <span className="font-mono-tech text-[var(--relay-text)]">
                      {selectedNode.device_origin}
                    </span>
                  </div>
                  <div className="flex justify-between gap-2">
                    <span className="text-[var(--relay-text-faint)]">{t("detail.updated")}</span>
                    <span className="font-mono-tech text-[var(--relay-text)]">
                      {new Date(selectedNode.updated_at).toLocaleTimeString()}
                    </span>
                  </div>
                  {selectedNode.payload.kind === "artifact" && (
                    <>
                      <div className="flex justify-between gap-2">
                        <span className="text-[var(--relay-text-faint)]">{t("detail.commit")}</span>
                        <span className="font-mono-tech truncate text-[var(--relay-text)]">
                          {selectedNode.payload.git_commit_sha}
                        </span>
                      </div>
                      <div className="flex justify-between gap-2">
                        <span className="text-[var(--relay-text-faint)]">{t("detail.env")}</span>
                        <span className="font-mono-tech truncate text-[var(--relay-text)]">
                          {selectedNode.payload.env_fingerprint}
                        </span>
                      </div>
                    </>
                  )}
                  {selectedNode.payload.kind === "evidence" && (
                    <div className="flex justify-between gap-2">
                      <span className="text-[var(--relay-text-faint)]">{t("detail.logHash")}</span>
                      <span className="font-mono-tech truncate text-[var(--relay-text)]">
                        {selectedNode.payload.log_blob_hash}
                      </span>
                    </div>
                  )}
                </div>
              </details>
            </>
          )
        )}
      </div>

      {/* Action Footer for Suggested Fix */}
      {isFixMode && (
        <div className="flex items-center gap-2 border-t border-[var(--relay-border)] bg-[var(--relay-surface-subtle)]/60 p-3.5">
          <Button
            variant="secondary"
            className="flex-1"
            isDisabled={isResolvingApproval}
            onPress={() => onResolveApproval(pendingFixTask.spec.task_id, "rejected")}
          >
            {t("detail.dismissFix")}
          </Button>
          <Button
            variant="primary"
            className="flex-1 font-semibold"
            isDisabled={isResolvingApproval}
            onPress={() => onResolveApproval(pendingFixTask.spec.task_id, "approved")}
          >
            <CheckCircle2 className="h-4 w-4" />
            {t("detail.applyAndRun")}
          </Button>
        </div>
      )}
    </aside>
  );
}
