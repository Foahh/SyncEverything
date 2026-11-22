import { Button, Chip, Separator } from "@heroui/react";
import {
  ArrowRight,
  CheckCircle2,
  ChevronDown,
  ChevronUp,
  Cpu,
  Link2,
  Play,
  RotateCcw,
  Settings2,
  Sparkles,
  Zap,
} from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type {
  PeerStatus_Serialize,
  RuntimeMode,
  TaskRecord_Serialize,
  TaskState,
} from "../bindings";
import { isTauriRuntime } from "../lib/relayApi";

interface LeftRailProps {
  status: PeerStatus_Serialize | undefined;
  tasks: TaskRecord_Serialize[];
  onOpenRunTask: () => void;
  onOpenPairModal: () => void;
  onOpenApproval: (task: TaskRecord_Serialize) => void;
  onSelectEvidenceNode: (nodeId: string) => void;
  onSwitchMode: (mode: RuntimeMode) => void;
  onTriggerUpstreamEdit: () => void;
  onResetDemo: () => void;
  isSimulatingEdit: boolean;
}

const RUNTIME_MODES: RuntimeMode[] = ["client", "host", "local_dual_sandbox"];

function stateBadgeColor(
  state: TaskState,
): "default" | "accent" | "success" | "warning" | "danger" {
  switch (state) {
    case "completed":
      return "success";
    case "self_healing_proposed":
      return "accent";
    case "queued":
    case "preflight_running":
    case "executing":
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
  onOpenRunTask,
  onOpenPairModal,
  onOpenApproval,
  onSelectEvidenceNode,
  onSwitchMode,
  onTriggerUpstreamEdit,
  onResetDemo,
  isSimulatingEdit,
}: LeftRailProps) {
  const { t } = useTranslation();
  const [settingsOpen, setSettingsOpen] = useState(false);

  const activeMode: RuntimeMode = status?.mode ?? "local_dual_sandbox";
  const isConnected = status?.connected ?? false;
  const pendingOutbox = status?.pending_outbox_count ?? 0;

  return (
    <aside className="flex h-full w-[268px] shrink-0 flex-col border-r border-[var(--relay-border)] bg-[var(--relay-surface)]">
      {/* Main scrollable section */}
      <div className="flex-1 overflow-y-auto p-3.5 space-y-5">
        {/* Primary Action */}
        <Button
          variant="primary"
          fullWidth
          className="h-9 text-[13px] font-semibold shadow-xs"
          onPress={onOpenRunTask}
        >
          <Play className="h-3.5 w-3.5" />
          {t("sidebar.runOnHost")}
        </Button>

        {/* Connection Summary */}
        <section className="space-y-2">
          <div className="text-[11px] font-medium text-[var(--relay-text-faint)]">
            {t("sidebar.deviceSection")}
          </div>
          <div className="rounded-xl border border-[var(--relay-border)] bg-[var(--relay-surface-subtle)]/70 p-3">
            <div className="flex items-center justify-between gap-2">
              <div className="flex items-center gap-2 min-w-0">
                <div className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-[var(--relay-surface)] text-teal-600 shadow-2xs dark:text-teal-400">
                  <Cpu className="h-3.5 w-3.5" />
                </div>
                <div className="min-w-0">
                  <div className="truncate text-[12.5px] font-semibold text-[var(--relay-text)]">
                    {isConnected ? t("sidebar.connectedTo") : t("sidebar.waitingPeer")}
                  </div>
                  <div className="text-[11px] text-[var(--relay-text-faint)]">
                    {t(`sidebar.mode.${activeMode}`)}
                    {pendingOutbox > 0
                      ? ` · ${t("sidebar.queuedCount", { count: pendingOutbox })}`
                      : ""}
                  </div>
                </div>
              </div>
              <span
                className={[
                  "h-2 w-2 shrink-0 rounded-full",
                  isConnected ? "bg-teal-500" : "bg-amber-500",
                ].join(" ")}
              />
            </div>

            <Button
              size="sm"
              variant="outline"
              fullWidth
              className="mt-2.5 h-7 text-[11.5px]"
              onPress={onOpenPairModal}
            >
              <Link2 className="h-3 w-3" />
              {t("topbar.pairDevice")}
            </Button>
          </div>
        </section>

        {/* Runs List */}
        <section className="space-y-2">
          <div className="flex items-center justify-between">
            <span className="text-[11px] font-medium text-[var(--relay-text-faint)]">
              {t("sidebar.runsSection")}
            </span>
            {tasks.length > 0 && (
              <span className="font-mono-tech text-[11px] text-[var(--relay-text-faint)]">
                {tasks.length}
              </span>
            )}
          </div>

          {tasks.length === 0 ? (
            <div className="rounded-xl border border-dashed border-[var(--relay-border)] px-3 py-5 text-center text-[12px] leading-relaxed text-[var(--relay-text-faint)]">
              {t("sidebar.noRuns")}
            </div>
          ) : (
            <div className="space-y-2">
              {tasks.map((task) => {
                const needsFixReview =
                  task.state === "self_healing_proposed" && task.active_proposal != null;

                return (
                  <div
                    key={task.spec.task_id}
                    className={[
                      "rounded-xl border p-3 transition-all",
                      needsFixReview
                        ? "border-indigo-400/60 bg-[var(--relay-heal-soft)]"
                        : "border-[var(--relay-border)] bg-[var(--relay-surface)] hover:border-[var(--relay-border-strong)]",
                    ].join(" ")}
                  >
                    <div className="flex items-center justify-between gap-2">
                      <span className="truncate text-[12.5px] font-semibold text-[var(--relay-text)]">
                        {task.spec.command_preset}
                      </span>
                      <Chip
                        size="sm"
                        variant={needsFixReview ? "primary" : "soft"}
                        color={stateBadgeColor(task.state)}
                        className="h-5 px-2 text-[10.5px] font-medium"
                      >
                        {t(`taskState.${task.state}`)}
                      </Chip>
                    </div>

                    {(task.preflight_summary || task.spec.user_note) && (
                      <p className="mt-1 line-clamp-2 text-[11.5px] leading-relaxed text-[var(--relay-text-muted)]">
                        {task.preflight_summary ?? task.spec.user_note}
                      </p>
                    )}

                    {needsFixReview && (
                      <Button
                        size="sm"
                        variant="primary"
                        fullWidth
                        className="mt-2.5 h-7.5 text-[11.5px] font-medium"
                        onPress={() => onOpenApproval(task)}
                      >
                        <Sparkles className="h-3.5 w-3.5" />
                        {t("sidebar.reviewFix")}
                        <ArrowRight className="h-3 w-3" />
                      </Button>
                    )}

                    {task.evidence_node_id && (
                      <button
                        type="button"
                        onClick={() => onSelectEvidenceNode(task.evidence_node_id!)}
                        className="mt-2 inline-flex items-center gap-1 text-[11.5px] font-medium text-teal-700 hover:underline dark:text-teal-400 cursor-pointer"
                      >
                        <CheckCircle2 className="h-3 w-3" />
                        {t("sidebar.viewResult")}
                      </button>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </section>
      </div>

      {/* Discreet collapsible footer for Device Role & Developer / Lab Tools */}
      <div className="border-t border-[var(--relay-border)] bg-[var(--relay-surface-subtle)]/50 p-2.5">
        <button
          type="button"
          onClick={() => setSettingsOpen((prev) => !prev)}
          className="flex w-full items-center justify-between rounded-lg px-2 py-1.5 text-[11.5px] font-medium text-[var(--relay-text-muted)] transition-colors hover:bg-[var(--relay-surface-subtle)] hover:text-[var(--relay-text)] cursor-pointer"
        >
          <span className="inline-flex items-center gap-1.5">
            <Settings2 className="h-3.5 w-3.5" />
            {t("sidebar.settingsButton")}
          </span>
          {settingsOpen ? (
            <ChevronDown className="h-3.5 w-3.5" />
          ) : (
            <ChevronUp className="h-3.5 w-3.5" />
          )}
        </button>

        {settingsOpen && (
          <div className="mt-2 space-y-3 rounded-xl border border-[var(--relay-border)] bg-[var(--relay-surface)] p-2.5">
            <div className="space-y-1.5">
              <div className="text-[10.5px] font-medium text-[var(--relay-text-faint)]">
                {t("sidebar.modeLabel")}
              </div>
              <div className="grid grid-cols-3 gap-1 rounded-lg bg-[var(--relay-surface-subtle)] p-0.5">
                {RUNTIME_MODES.map((mode) => {
                  const active = mode === activeMode;
                  return (
                    <button
                      key={mode}
                      type="button"
                      onClick={() => onSwitchMode(mode)}
                      className={[
                        "rounded-md px-1.5 py-1 text-[10.5px] font-medium transition-all cursor-pointer",
                        active
                          ? "bg-[var(--relay-surface)] text-[var(--relay-text)] shadow-2xs"
                          : "text-[var(--relay-text-muted)] hover:text-[var(--relay-text)]",
                      ].join(" ")}
                    >
                      {t(`sidebar.mode.${mode}`)}
                    </button>
                  );
                })}
              </div>
            </div>

            <Separator />

            <div className="space-y-1.5">
              <div className="text-[10.5px] font-medium text-[var(--relay-text-faint)]">
                {t("sidebar.devTools")}
              </div>
              <Button
                size="sm"
                variant="outline"
                fullWidth
                isDisabled={isSimulatingEdit}
                onPress={onTriggerUpstreamEdit}
                className="h-7 justify-start text-[11px]"
              >
                <Zap className="h-3 w-3 text-amber-500" />
                {t("sidebar.simulateCodeChange")}
              </Button>
              {!isTauriRuntime() && (
                <Button
                  size="sm"
                  variant="ghost"
                  fullWidth
                  onPress={onResetDemo}
                  className="h-7 justify-start text-[11px]"
                >
                  <RotateCcw className="h-3 w-3" />
                  {t("sidebar.resetPreview")}
                </Button>
              )}
            </div>
          </div>
        )}
      </div>
    </aside>
  );
}
