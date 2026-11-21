import { Alert, Button, Chip, Separator, Tabs } from "@heroui/react";
import {
  CheckCircle2,
  GitBranch,
  ShieldAlert,
  ShieldCheck,
  Terminal,
  X,
  XCircle,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import type { ApprovalDecision, GraphNode, TaskRecord_Serialize } from "../bindings";

interface InspectorDrawerProps {
  open: boolean;
  activeTab: "node" | "approval";
  onTabChange: (tab: "node" | "approval") => void;
  selectedNode: GraphNode | null;
  pendingApprovalTask: TaskRecord_Serialize | null;
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
          const isHeader = line.startsWith("diff ") || line.startsWith("index ");

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
              <span className="w-6 shrink-0 select-none text-right text-slate-500">
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

export function InspectorDrawer({
  open,
  activeTab,
  onTabChange,
  selectedNode,
  pendingApprovalTask,
  onResolveApproval,
  isResolvingApproval,
  onClose,
}: InspectorDrawerProps) {
  const { t } = useTranslation();

  if (!open) return null;

  const proposal = pendingApprovalTask?.active_proposal ?? null;

  return (
    <aside className="flex h-full w-[392px] shrink-0 flex-col border-l border-[var(--relay-border)] bg-[var(--relay-surface)] shadow-xl z-20">
      {/* Drawer Header */}
      <div className="flex items-center justify-between gap-2 border-b border-[var(--relay-border)] px-3.5 py-2.5">
        <Tabs
          selectedKey={activeTab}
          onSelectionChange={(key) => onTabChange(key === "approval" ? "approval" : "node")}
          className="min-w-0 flex-1"
        >
          <Tabs.ListContainer className="w-full overflow-visible">
            <Tabs.List className="grid w-full grid-cols-2 rounded-lg bg-[var(--relay-surface-subtle)] p-0.5">
              <Tabs.Tab
                id="node"
                className="whitespace-nowrap px-2.5 py-1 text-[11.5px] font-medium"
              >
                <Tabs.Indicator />
                {t("inspector.nodeTab")}
              </Tabs.Tab>
              <Tabs.Tab
                id="approval"
                className="whitespace-nowrap px-2.5 py-1 text-[11.5px] font-medium"
              >
                <Tabs.Indicator />
                <span className="inline-flex items-center gap-1.5 whitespace-nowrap">
                  {t("inspector.approvalTab")}
                  {proposal && (
                    <span className="h-2 w-2 shrink-0 rounded-full bg-indigo-500 animate-pulse" />
                  )}
                </span>
              </Tabs.Tab>
            </Tabs.List>
          </Tabs.ListContainer>
        </Tabs>

        <Button
          size="sm"
          variant="ghost"
          isIconOnly
          aria-label={t("inspector.close")}
          onPress={onClose}
          className="h-7 w-7 min-w-7 shrink-0"
        >
          <X className="h-4 w-4" />
        </Button>
      </div>

      {/* Drawer Body */}
      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        {activeTab === "node" ? (
          selectedNode ? (
            <>
              <div className="flex items-start justify-between gap-2">
                <div>
                  <span className="font-mono-tech text-[10.5px] uppercase tracking-wider text-[var(--relay-text-faint)]">
                    {selectedNode.id}
                  </span>
                  <h3 className="mt-0.5 text-[15px] font-bold text-[var(--relay-text)]">
                    {selectedNode.label}
                  </h3>
                </div>
                <Chip
                  size="sm"
                  variant={selectedNode.freshness.status === "stale" ? "primary" : "soft"}
                  color={
                    selectedNode.freshness.status === "stale"
                      ? "danger"
                      : selectedNode.freshness.status === "pending_verify"
                        ? "warning"
                        : "success"
                  }
                  className="font-mono-tech shrink-0 text-[10.5px] font-semibold"
                >
                  {t(`dag.freshness.${selectedNode.freshness.status}`)}
                </Chip>
              </div>

              {/* STALE Provenance Alert */}
              {selectedNode.freshness.status === "stale" && (
                <Alert status="danger" className="border border-red-500/35">
                  <Alert.Indicator>
                    <ShieldAlert className="h-4 w-4 text-red-600 dark:text-red-400" />
                  </Alert.Indicator>
                  <Alert.Content>
                    <Alert.Title className="text-[12px] font-semibold">
                      {t("inspector.provenanceAlertTitle")}
                    </Alert.Title>
                    <Alert.Description className="text-[11.5px] leading-relaxed">
                      {t("inspector.provenanceAlertBody", {
                        causedBy: selectedNode.freshness.caused_by,
                        reason: selectedNode.freshness.reason,
                      })}
                    </Alert.Description>
                  </Alert.Content>
                </Alert>
              )}

              {/* Kind-specific details */}
              {selectedNode.payload.kind === "decision" && (
                <div className="space-y-3">
                  <div>
                    <div className="text-[11px] font-semibold uppercase tracking-wider text-[var(--relay-text-faint)]">
                      {t("inspector.rationale")}
                    </div>
                    <p className="mt-1.5 rounded-xl border border-[var(--relay-border)] bg-[var(--relay-surface-subtle)] p-3 text-[12.5px] leading-relaxed text-[var(--relay-text)]">
                      {selectedNode.payload.rationale}
                    </p>
                  </div>
                  <div>
                    <div className="text-[11px] font-semibold uppercase tracking-wider text-[var(--relay-text-faint)]">
                      {t("inspector.constraints")}
                    </div>
                    <div className="mt-1.5 flex flex-wrap gap-1.5">
                      {selectedNode.payload.constraints.map((c) => (
                        <Chip
                          key={c}
                          size="sm"
                          variant="soft"
                          className="font-mono-tech text-[11px]"
                        >
                          {c}
                        </Chip>
                      ))}
                    </div>
                  </div>
                </div>
              )}

              {selectedNode.payload.kind === "artifact" && (
                <div className="space-y-3">
                  <div className="grid grid-cols-1 gap-2 rounded-xl border border-[var(--relay-border)] bg-[var(--relay-surface-subtle)] p-3 text-[12px]">
                    <div className="flex justify-between gap-2">
                      <span className="text-[var(--relay-text-faint)]">
                        {t("inspector.commitSha")}
                      </span>
                      <span className="font-mono-tech font-medium text-[var(--relay-text)]">
                        {selectedNode.payload.git_commit_sha}
                      </span>
                    </div>
                    <div className="flex justify-between gap-2">
                      <span className="text-[var(--relay-text-faint)]">
                        {t("inspector.envFingerprint")}
                      </span>
                      <span className="font-mono-tech text-[var(--relay-text)]">
                        {selectedNode.payload.env_fingerprint}
                      </span>
                    </div>
                  </div>
                  <div>
                    <div className="text-[11px] font-semibold uppercase tracking-wider text-[var(--relay-text-faint)]">
                      {t("inspector.changedFiles")}
                    </div>
                    <ul className="mt-1.5 space-y-1">
                      {selectedNode.payload.changed_files.map((file) => (
                        <li
                          key={file}
                          className="font-mono-tech rounded-lg border border-[var(--relay-border)] bg-[var(--relay-surface-subtle)] px-2.5 py-1.5 text-[11.5px] text-[var(--relay-text)]"
                        >
                          {file}
                        </li>
                      ))}
                    </ul>
                  </div>
                </div>
              )}

              {selectedNode.payload.kind === "evidence" && (
                <div className="space-y-3">
                  <div>
                    <div className="text-[11px] font-semibold uppercase tracking-wider text-[var(--relay-text-faint)]">
                      {t("inspector.metrics")}
                    </div>
                    <pre className="font-mono-tech mt-1.5 overflow-x-auto rounded-xl border border-[var(--relay-border)] bg-[var(--relay-surface-subtle)] p-3 text-[11.5px] leading-relaxed text-[var(--relay-text)]">
                      {JSON.stringify(selectedNode.payload.metrics_json, null, 2)}
                    </pre>
                  </div>
                  <div className="rounded-xl border border-[var(--relay-border)] bg-[var(--relay-surface-subtle)] p-3 text-[11.5px] flex items-center justify-between">
                    <span className="text-[var(--relay-text-faint)]">
                      {t("inspector.logBlobHash")}
                    </span>
                    <span className="font-mono-tech text-[var(--relay-text)]">
                      {selectedNode.payload.log_blob_hash}
                    </span>
                  </div>
                </div>
              )}

              <Separator />

              <div className="grid grid-cols-2 gap-2 text-[11px] text-[var(--relay-text-faint)]">
                <div>
                  <div>{t("inspector.deviceOrigin")}</div>
                  <div className="font-mono-tech mt-0.5 text-[var(--relay-text-muted)]">
                    {selectedNode.device_origin}
                  </div>
                </div>
                <div>
                  <div>{t("inspector.updatedAt")}</div>
                  <div className="font-mono-tech mt-0.5 text-[var(--relay-text-muted)]">
                    {new Date(selectedNode.updated_at).toLocaleTimeString()}
                  </div>
                </div>
              </div>
            </>
          ) : (
            <div className="rounded-xl border border-dashed border-[var(--relay-border)] p-6 text-center text-[12px] text-[var(--relay-text-faint)]">
              {t("inspector.selectNodePrompt")}
            </div>
          )
        ) : proposal && pendingApprovalTask ? (
          <div className="space-y-4">
            {/* Zero-pollution worktree shield banner */}
            <div className="flex items-center gap-2 rounded-xl border border-indigo-500/35 bg-[var(--relay-heal-soft)] px-3 py-2 text-[11.5px] font-medium text-indigo-700 dark:text-indigo-300">
              <ShieldCheck className="h-4 w-4 shrink-0" />
              <span>{t("inspector.worktreeShield")}</span>
            </div>

            <div className="space-y-1.5">
              <div className="flex items-center justify-between gap-2">
                <Chip
                  size="sm"
                  variant="soft"
                  color="accent"
                  className="text-[10.5px] font-semibold"
                >
                  {proposal.kind === "uv_spec"
                    ? t("inspector.patchKindUv")
                    : t("inspector.patchKindCode")}
                </Chip>
                <span className="font-mono-tech inline-flex items-center gap-1 text-[11px] text-[var(--relay-text-faint)]">
                  <GitBranch className="h-3 w-3" />
                  {proposal.worktree_branch}
                </span>
              </div>
              <p className="text-[12.5px] leading-relaxed text-[var(--relay-text)]">
                {proposal.summary}
              </p>
            </div>

            {/* Unified Diff Preview */}
            <div className="space-y-1.5">
              <div className="text-[11px] font-semibold uppercase tracking-wider text-[var(--relay-text-faint)]">
                {t("inspector.proposedDiff")}
              </div>
              <UnifiedDiffViewer diff={proposal.diff_preview} />
            </div>

            {/* Smoke Preflight Proof */}
            <div className="space-y-1.5">
              <div className="flex items-center justify-between">
                <span className="inline-flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-[var(--relay-text-faint)]">
                  <Terminal className="h-3.5 w-3.5" />
                  {t("inspector.smokeProof")}
                </span>
                <Chip
                  size="sm"
                  variant="soft"
                  color={proposal.smoke_verified ? "success" : "warning"}
                  className="font-mono-tech h-5 px-2 text-[10px] font-semibold"
                >
                  {proposal.smoke_verified
                    ? t("inspector.smokePassed")
                    : t("inspector.smokePending")}
                </Chip>
              </div>
              <pre className="font-mono-tech whitespace-pre-wrap break-words rounded-xl border border-teal-500/30 bg-[var(--relay-fresh-soft)] p-3 text-[11px] leading-relaxed text-teal-900 dark:text-teal-200">
                {proposal.smoke_stdout_tail}
              </pre>
            </div>
          </div>
        ) : (
          <div className="rounded-xl border border-dashed border-[var(--relay-border)] p-6 text-center text-[12px] text-[var(--relay-text-faint)]">
            {t("inspector.noPendingApproval")}
          </div>
        )}
      </div>

      {/* Sticky Approval Action Footer */}
      {activeTab === "approval" && proposal && pendingApprovalTask && (
        <div className="flex items-center gap-2 border-t border-[var(--relay-border)] bg-[var(--relay-surface-subtle)]/70 p-3.5">
          <Button
            variant="danger-soft"
            className="flex-1"
            isDisabled={isResolvingApproval}
            onPress={() => onResolveApproval(pendingApprovalTask.spec.task_id, "rejected")}
          >
            <XCircle className="h-4 w-4" />
            {t("inspector.rejectPatch")}
          </Button>
          <Button
            variant="primary"
            className="flex-1 font-semibold"
            isDisabled={isResolvingApproval}
            onPress={() => onResolveApproval(pendingApprovalTask.spec.task_id, "approved")}
          >
            <CheckCircle2 className="h-4 w-4" />
            {t("inspector.approveAndExecute")}
          </Button>
        </div>
      )}
    </aside>
  );
}
