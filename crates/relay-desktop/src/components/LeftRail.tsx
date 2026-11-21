import { Button, Card, Chip, Separator } from "@heroui/react";
import {
  ArrowLeftRight,
  Clock,
  Cpu,
  History,
  Laptop,
  RotateCcw,
  Sparkles,
  Wrench,
  Zap,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import type {
  HandoffSummary,
  PeerStatus_Serialize,
  TaskRecord_Serialize,
  TaskState,
} from "../bindings";
import { isTauriRuntime } from "../lib/relayApi";

interface LeftRailProps {
  status: PeerStatus_Serialize | undefined;
  tasks: TaskRecord_Serialize[];
  handoffs: HandoffSummary[];
  onOpenApproval: (task: TaskRecord_Serialize) => void;
  onSelectEvidenceNode: (nodeId: string) => void;
  onTriggerUpstreamEdit: () => void;
  onResetDemo: () => void;
  isSimulatingEdit: boolean;
}

function taskStateColor(state: TaskState): "default" | "accent" | "success" | "warning" | "danger" {
  switch (state) {
    case "completed":
      return "success";
    case "self_healing_proposed":
      return "accent";
    case "preflight_running":
    case "executing":
    case "queued":
      return "warning";
    case "failed":
    case "stale":
    case "rejected":
      return "danger";
  }
}

export function LeftRail({
  status,
  tasks,
  handoffs,
  onOpenApproval,
  onSelectEvidenceNode,
  onTriggerUpstreamEdit,
  onResetDemo,
  isSimulatingEdit,
}: LeftRailProps) {
  const { t } = useTranslation();

  return (
    <aside className="flex h-full w-[292px] shrink-0 flex-col border-r border-[var(--relay-border)] bg-[var(--relay-surface)] overflow-y-auto">
      <div className="p-3 space-y-3">
        {/* 1. Dual-Device Topology Card */}
        <Card className="border border-[var(--relay-border)] bg-[var(--relay-surface-subtle)]/70 shadow-none">
          <Card.Header className="pb-2 pt-3 px-3.5">
            <div className="flex items-center justify-between w-full">
              <Card.Title className="text-[11.5px] font-semibold uppercase tracking-wider text-[var(--relay-text-muted)]">
                {t("rail.topologyTitle")}
              </Card.Title>
              <Chip
                size="sm"
                variant="soft"
                color={(status?.pending_outbox_count ?? 0) > 0 ? "warning" : "success"}
                className="font-mono-tech h-5 px-1.5 text-[10px]"
              >
                {(status?.pending_outbox_count ?? 0) > 0
                  ? t("rail.outboxQueued", { count: status?.pending_outbox_count ?? 0 })
                  : t("rail.synced")}
              </Chip>
            </div>
          </Card.Header>
          <Card.Content className="px-3.5 pb-3.5 pt-1 space-y-2">
            {/* Local Edge Device */}
            <div className="flex items-center justify-between rounded-lg border border-[var(--relay-border)] bg-[var(--relay-surface)] px-2.5 py-2">
              <div className="flex items-center gap-2 min-w-0">
                <div className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-sky-500/10 text-sky-600 dark:text-sky-400">
                  <Laptop className="h-3.5 w-3.5" />
                </div>
                <div className="min-w-0">
                  <div className="truncate text-[11.5px] font-semibold text-[var(--relay-text)]">
                    {t("rail.localPeer")}
                  </div>
                  <div className="font-mono-tech truncate text-[10px] text-[var(--relay-text-faint)]">
                    {status?.local_node_id ?? "—"}
                  </div>
                </div>
              </div>
              <span className="h-2 w-2 shrink-0 rounded-full bg-teal-500" />
            </div>

            {/* QUIC Link Indicator */}
            <div className="flex items-center justify-center gap-2 text-[10px] font-mono-tech text-[var(--relay-text-faint)]">
              <div className="h-px flex-1 bg-[var(--relay-border)]" />
              <span className="inline-flex items-center gap-1 rounded-full border border-[var(--relay-border)] bg-[var(--relay-surface)] px-2 py-0.5">
                <ArrowLeftRight className="h-2.5 w-2.5 text-teal-600 dark:text-teal-400" />
                iroh 1.5 QUIC
              </span>
              <div className="h-px flex-1 bg-[var(--relay-border)]" />
            </div>

            {/* Remote Host Workstation */}
            <div className="flex items-center justify-between rounded-lg border border-[var(--relay-border)] bg-[var(--relay-surface)] px-2.5 py-2">
              <div className="flex items-center gap-2 min-w-0">
                <div className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-indigo-500/10 text-indigo-600 dark:text-indigo-400">
                  <Cpu className="h-3.5 w-3.5" />
                </div>
                <div className="min-w-0">
                  <div className="truncate text-[11.5px] font-semibold text-[var(--relay-text)]">
                    {t("rail.remotePeer")}
                  </div>
                  <div className="font-mono-tech truncate text-[10px] text-[var(--relay-text-faint)]">
                    {status?.remote_node_id ?? "unpaired"}
                  </div>
                </div>
              </div>
              <span
                className={[
                  "h-2 w-2 shrink-0 rounded-full",
                  status?.connected ? "bg-teal-500" : "bg-amber-500",
                ].join(" ")}
              />
            </div>
          </Card.Content>
        </Card>

        {/* 2. Active Task & Self-Healing Approval Queue */}
        <section className="space-y-2">
          <div className="flex items-center justify-between px-0.5">
            <h2 className="flex items-center gap-1.5 text-[11.5px] font-semibold uppercase tracking-wider text-[var(--relay-text-muted)]">
              <Wrench className="h-3.5 w-3.5" />
              {t("rail.taskQueueTitle")}
            </h2>
            <span className="font-mono-tech text-[11px] text-[var(--relay-text-faint)]">
              {tasks.length}
            </span>
          </div>

          {tasks.length === 0 ? (
            <div className="rounded-xl border border-dashed border-[var(--relay-border)] p-4 text-center text-[11.5px] text-[var(--relay-text-faint)]">
              {t("rail.noTasks")}
            </div>
          ) : (
            <div className="space-y-2">
              {tasks.map((task) => {
                const needsApproval =
                  task.state === "self_healing_proposed" && task.active_proposal != null;
                return (
                  <div
                    key={task.spec.task_id}
                    className={[
                      "rounded-xl border p-3 transition-all",
                      needsApproval
                        ? "border-indigo-500/60 bg-[var(--relay-heal-soft)] shadow-xs"
                        : "border-[var(--relay-border)] bg-[var(--relay-surface)] hover:border-[var(--relay-border-strong)]",
                    ].join(" ")}
                  >
                    <div className="flex items-center justify-between gap-2">
                      <span className="font-mono-tech text-[12px] font-semibold text-[var(--relay-text)]">
                        {task.spec.command_preset}
                      </span>
                      <Chip
                        size="sm"
                        variant={needsApproval ? "primary" : "soft"}
                        color={taskStateColor(task.state)}
                        className="h-5 px-1.5 text-[10px] font-semibold"
                      >
                        {t(`taskState.${task.state}`)}
                      </Chip>
                    </div>

                    <p className="mt-1 line-clamp-2 text-[11px] leading-relaxed text-[var(--relay-text-muted)]">
                      {task.preflight_summary ?? task.spec.user_note ?? task.spec.base_commit_sha}
                    </p>

                    <div className="mt-2 flex items-center justify-between text-[10px] font-mono-tech text-[var(--relay-text-faint)]">
                      <span>@{task.spec.base_commit_sha.slice(0, 7)}</span>
                      <span>#{task.spec.task_id.slice(-6)}</span>
                    </div>

                    {needsApproval && (
                      <Button
                        size="sm"
                        variant="primary"
                        fullWidth
                        className="mt-2.5 h-8 text-[11.5px] font-semibold"
                        onPress={() => onOpenApproval(task)}
                      >
                        <Sparkles className="h-3.5 w-3.5" />
                        {t("rail.reviewDiffCta")}
                      </Button>
                    )}

                    {task.evidence_node_id && (
                      <button
                        type="button"
                        onClick={() => onSelectEvidenceNode(task.evidence_node_id!)}
                        className="mt-2 font-mono-tech text-[10.5px] font-medium text-teal-700 hover:underline dark:text-teal-400 cursor-pointer"
                      >
                        → {task.evidence_node_id}
                      </button>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </section>

        <Separator />

        {/* 3. Recent Handoffs */}
        <section className="space-y-2">
          <h2 className="flex items-center gap-1.5 px-0.5 text-[11.5px] font-semibold uppercase tracking-wider text-[var(--relay-text-muted)]">
            <History className="h-3.5 w-3.5" />
            {t("rail.handoffsTitle")}
          </h2>

          {handoffs.length === 0 ? (
            <p className="px-1 text-[11.5px] text-[var(--relay-text-faint)]">
              {t("rail.noHandoffs")}
            </p>
          ) : (
            <div className="space-y-1.5">
              {handoffs.map((h) => (
                <div
                  key={h.handoff_id}
                  className="rounded-lg border border-[var(--relay-border)] bg-[var(--relay-surface-subtle)]/50 px-2.5 py-2"
                >
                  <div className="flex items-center justify-between text-[10.5px]">
                    <span className="font-medium text-[var(--relay-text)]">{h.source_device}</span>
                    <span className="font-mono-tech inline-flex items-center gap-1 text-[10px] text-[var(--relay-text-faint)]">
                      <Clock className="h-2.5 w-2.5" />@{h.base_commit_sha.slice(0, 7)}
                    </span>
                  </div>
                  <p className="mt-0.5 line-clamp-2 text-[11px] leading-snug text-[var(--relay-text-muted)]">
                    {h.summary}
                  </p>
                </div>
              ))}
            </div>
          )}
        </section>

        {/* Demo Quick Controls */}
        <div className="rounded-xl border border-[var(--relay-border)] bg-[var(--relay-surface-subtle)]/60 p-3 space-y-2">
          <div className="text-[10.5px] font-semibold uppercase tracking-wider text-[var(--relay-text-faint)]">
            {t("rail.demoControlsTitle")}
          </div>
          <Button
            size="sm"
            variant="danger-soft"
            fullWidth
            isDisabled={isSimulatingEdit}
            onPress={onTriggerUpstreamEdit}
            className="h-8 text-[11.5px]"
          >
            <Zap className="h-3.5 w-3.5" />
            {t("rail.simulateUpstreamEdit")}
          </Button>
          {!isTauriRuntime() && (
            <Button
              size="sm"
              variant="outline"
              fullWidth
              onPress={onResetDemo}
              className="h-7 text-[11px]"
            >
              <RotateCcw className="h-3 w-3" />
              {t("rail.resetDemo")}
            </Button>
          )}
        </div>
      </div>
    </aside>
  );
}
