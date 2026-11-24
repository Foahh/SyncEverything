import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { cn } from "@/lib/utils";
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
import { isTauriRuntime, needsTaskApproval } from "../lib/relayApi";

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

function stateBadgeProps(state: TaskState): {
  variant: "default" | "secondary" | "destructive" | "outline";
  className?: string;
} {
  switch (state) {
    case "completed":
      return {
        variant: "secondary",
        className:
          "bg-emerald-500/12 text-emerald-700 dark:bg-emerald-500/20 dark:text-emerald-300",
      };
    case "self_healing_proposed":
      return { variant: "default" };
    case "queued":
    case "preflight_running":
    case "executing":
      return {
        variant: "secondary",
        className: "bg-amber-500/15 text-amber-700 dark:bg-amber-500/20 dark:text-amber-300",
      };
    case "failed":
    case "stale":
    case "rejected":
      return { variant: "destructive" };
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
    <aside className="flex h-full w-[268px] shrink-0 flex-col border-r border-border bg-sidebar text-sidebar-foreground">
      <div className="flex flex-1 flex-col gap-5 overflow-y-auto p-3.5">
        <Button size="lg" className="w-full justify-center" onClick={onOpenRunTask}>
          <Play data-icon="inline-start" />
          <span className="relay-text-trim leading-none">{t("sidebar.runOnHost")}</span>
        </Button>

        <section className="flex flex-col gap-2">
          <div className="text-[11px] font-medium text-muted-foreground">
            {t("sidebar.deviceSection")}
          </div>
          <div className="flex flex-col gap-2.5 rounded-lg border border-border bg-card p-3">
            <div className="flex items-center justify-between gap-2">
              <div className="flex min-w-0 items-center gap-2">
                <div className="flex size-7 shrink-0 items-center justify-center rounded-md bg-muted text-primary">
                  <Cpu className="size-3.5" />
                </div>
                <div className="min-w-0">
                  <div className="truncate text-xs font-semibold text-foreground">
                    {isConnected ? t("sidebar.connectedTo") : t("sidebar.waitingPeer")}
                  </div>
                  <div className="text-[11px] text-muted-foreground">
                    {t(`sidebar.mode.${activeMode}`)}
                    {pendingOutbox > 0
                      ? ` · ${t("sidebar.queuedCount", { count: pendingOutbox })}`
                      : ""}
                  </div>
                </div>
              </div>
              <span
                className={cn(
                  "size-2 shrink-0 rounded-full",
                  isConnected ? "bg-emerald-500" : "bg-amber-500",
                )}
              />
            </div>

            <Button variant="outline" size="default" className="w-full" onClick={onOpenPairModal}>
              <Link2 data-icon="inline-start" />
              <span className="relay-text-trim leading-none">{t("topbar.pairDevice")}</span>
            </Button>
          </div>
        </section>

        <section className="flex flex-col gap-2">
          <div className="flex items-center justify-between">
            <span className="text-[11px] font-medium text-muted-foreground">
              {t("sidebar.runsSection")}
            </span>
            {tasks.length > 0 && (
              <span className="font-mono-tech text-[11px] text-muted-foreground">
                {tasks.length}
              </span>
            )}
          </div>

          {tasks.length === 0 ? (
            <div className="rounded-lg border border-dashed border-border px-3 py-5 text-center text-xs leading-relaxed text-muted-foreground">
              {t("sidebar.noRuns")}
            </div>
          ) : (
            <div className="flex flex-col gap-2">
              {tasks.map((task) => {
                const needsFixReview = needsTaskApproval(task);
                const badge = stateBadgeProps(task.state);

                return (
                  <div
                    key={task.spec.task_id}
                    className={cn(
                      "rounded-lg border p-3 transition-colors",
                      needsFixReview
                        ? "border-primary/40 bg-[var(--relay-heal-soft)]"
                        : "border-border bg-card hover:border-foreground/20",
                    )}
                  >
                    <div className="flex items-center justify-between gap-2">
                      <span className="truncate text-xs font-semibold text-foreground">
                        {task.spec.command_preset}
                      </span>
                      <Badge variant={badge.variant} className={badge.className}>
                        <span className="relay-text-trim leading-none">
                          {needsFixReview && !task.active_proposal
                            ? t("sidebar.awaitingApproval")
                            : t(`taskState.${task.state}`)}
                        </span>
                      </Badge>
                    </div>

                    {(task.preflight_summary || task.spec.user_note) && (
                      <p className="mt-1 line-clamp-2 text-[11.5px] leading-relaxed text-muted-foreground">
                        {task.preflight_summary ?? task.spec.user_note}
                      </p>
                    )}

                    {needsFixReview && (
                      <Button
                        size="default"
                        className="mt-2.5 w-full justify-center"
                        onClick={() => onOpenApproval(task)}
                      >
                        <Sparkles data-icon="inline-start" />
                        <span className="relay-text-trim leading-none">
                          {t(task.active_proposal ? "sidebar.reviewFix" : "sidebar.reviewRun")}
                        </span>
                        <ArrowRight data-icon="inline-end" />
                      </Button>
                    )}

                    {task.evidence_node_id && (
                      <button
                        type="button"
                        onClick={() => onSelectEvidenceNode(task.evidence_node_id!)}
                        className="mt-2 inline-flex cursor-pointer items-center gap-1 text-[11.5px] font-medium text-primary hover:underline"
                      >
                        <CheckCircle2 className="size-3 shrink-0" />
                        <span className="relay-text-trim leading-none">
                          {t("sidebar.viewResult")}
                        </span>
                      </button>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </section>
      </div>

      {/* Discreet footer popover for Device Role & Developer Tools */}
      <div className="border-t border-border bg-muted/40 p-2.5">
        <button
          type="button"
          onClick={() => setSettingsOpen((prev) => !prev)}
          className="flex w-full cursor-pointer items-center justify-between rounded-md px-2 py-1.5 text-[11.5px] font-medium text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
        >
          <span className="inline-flex items-center gap-1.5">
            <Settings2 className="size-3.5 shrink-0" />
            <span className="relay-text-trim leading-none">{t("sidebar.settingsButton")}</span>
          </span>
          {settingsOpen ? <ChevronDown className="size-3.5" /> : <ChevronUp className="size-3.5" />}
        </button>

        {settingsOpen && (
          <div className="mt-2 flex flex-col gap-3 rounded-lg border border-border bg-card p-2.5">
            <div className="flex flex-col gap-1.5">
              <div className="text-[10.5px] font-medium text-muted-foreground">
                {t("sidebar.modeLabel")}
              </div>
              <div className="grid grid-cols-3 gap-1 rounded-md bg-muted p-0.5">
                {RUNTIME_MODES.map((mode) => {
                  const active = mode === activeMode;
                  return (
                    <button
                      key={mode}
                      type="button"
                      onClick={() => onSwitchMode(mode)}
                      className={cn(
                        "cursor-pointer rounded-sm px-1.5 py-1 text-[10.5px] font-medium transition-all",
                        active
                          ? "bg-background text-foreground shadow-2xs"
                          : "text-muted-foreground hover:text-foreground",
                      )}
                    >
                      {t(`sidebar.mode.${mode}`)}
                    </button>
                  );
                })}
              </div>
            </div>

            <Separator />

            <div className="flex flex-col gap-1.5">
              <div className="text-[10.5px] font-medium text-muted-foreground">
                {t("sidebar.devTools")}
              </div>
              <Button
                variant="outline"
                size="default"
                className="w-full justify-start"
                disabled={isSimulatingEdit}
                onClick={onTriggerUpstreamEdit}
              >
                <Zap data-icon="inline-start" className="text-amber-500" />
                <span className="relay-text-trim leading-none">
                  {t("sidebar.simulateCodeChange")}
                </span>
              </Button>
              {!isTauriRuntime() && (
                <Button
                  variant="ghost"
                  size="default"
                  className="w-full justify-start"
                  onClick={onResetDemo}
                >
                  <RotateCcw data-icon="inline-start" />
                  <span className="relay-text-trim leading-none">{t("sidebar.resetPreview")}</span>
                </Button>
              )}
            </div>
          </div>
        )}
      </div>
    </aside>
  );
}
