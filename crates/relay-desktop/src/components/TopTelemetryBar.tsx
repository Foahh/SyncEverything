import { Alert, AlertAction, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Separator } from "@/components/ui/separator";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";
import {
  AlertCircle,
  Cpu,
  GitBranch,
  Globe,
  Laptop,
  Link2,
  ListTodo,
  MonitorSmartphone,
  Moon,
  Play,
  SlidersHorizontal,
  Sparkles,
  Sun,
  X,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import type {
  PeerStatus_Serialize,
  RecentWorkspaceEntry_Serialize,
  RuntimeMode,
  WorkspaceInfo_Serialize,
} from "../bindings";
import { setRelayLocale, type RelayLocale } from "../i18n";
import { isTauriRuntime } from "../lib/relayApi";
import { WorkspaceSwitcher } from "./WorkspaceSwitcher";

export type WorkspaceView = "tasks" | "graph";

const RUNTIME_MODES: readonly RuntimeMode[] = ["client", "host", "local_dual_sandbox"];

interface TopTelemetryBarProps {
  activeView: WorkspaceView;
  onViewChange: (view: WorkspaceView) => void;
  attentionCount: number;
  status: PeerStatus_Serialize | undefined;
  workspace: WorkspaceInfo_Serialize | undefined;
  recentWorkspaces: RecentWorkspaceEntry_Serialize[];
  headCommitSha: string | null;
  staleCount: number;
  theme: "light" | "dark";
  isCopilotOpen: boolean;
  isSwitchingWorkspace: boolean;
  isInitializingManifest: boolean;
  isSwitchingMode?: boolean;
  switchModeError?: string | null;
  onDismissSwitchModeError?: () => void;
  onSwitchMode: (mode: RuntimeMode) => void;
  onTriggerUpstreamEdit: () => void;
  onResetDemo: () => void;
  isSimulatingEdit: boolean;
  onToggleTheme: () => void;
  onToggleCopilot: () => void;
  onOpenPairModal: () => void;
  onOpenRunTask: () => void;
  onOpenRepoSettings: () => void;
  onOpenLlmSettings: () => void;
  onSwitchWorkspace: (repoPath: string) => Promise<void>;
  onRemoveRecentWorkspace: (repoPath: string) => Promise<void>;
  onInitManifest: () => Promise<void>;
}

function ModeIcon({ mode }: { mode: RuntimeMode }) {
  if (mode === "host") return <Cpu className="size-3.5 shrink-0" aria-hidden="true" />;
  if (mode === "client") return <Laptop className="size-3.5 shrink-0" aria-hidden="true" />;
  return <MonitorSmartphone className="size-3.5 shrink-0" aria-hidden="true" />;
}

export function TopTelemetryBar({
  activeView,
  onViewChange,
  attentionCount,
  status,
  workspace,
  recentWorkspaces,
  headCommitSha,
  staleCount,
  theme,
  isCopilotOpen,
  isSwitchingWorkspace,
  isInitializingManifest,
  isSwitchingMode = false,
  switchModeError = null,
  onDismissSwitchModeError,
  onSwitchMode,
  onTriggerUpstreamEdit,
  onResetDemo,
  isSimulatingEdit,
  onToggleTheme,
  onToggleCopilot,
  onOpenPairModal,
  onOpenRunTask,
  onOpenRepoSettings,
  onOpenLlmSettings,
  onSwitchWorkspace,
  onRemoveRecentWorkspace,
  onInitManifest,
}: TopTelemetryBarProps) {
  const { t, i18n } = useTranslation();
  const currentLang: RelayLocale = i18n.language === "zh-CN" ? "zh-CN" : "en";
  const activeMode: RuntimeMode = status?.mode ?? "local_dual_sandbox";
  const isConnected = status?.connected ?? false;
  const transport = status?.transport_path ?? "offline";
  const pendingOutbox = status?.pending_outbox_count ?? 0;

  return (
    <header className="flex h-12 shrink-0 items-center justify-between gap-3 border-b border-border bg-background px-4">
      <div className="flex min-w-0 items-center gap-2">
        <div className="flex items-center gap-2">
          <div className="flex size-7 shrink-0 items-center justify-center rounded-md bg-primary text-primary-foreground shadow-xs">
            <svg
              aria-hidden="true"
              viewBox="0 0 24 24"
              className="size-4"
              fill="none"
              stroke="currentColor"
              strokeWidth="2.3"
            >
              <circle cx="6" cy="12" r="2.5" />
              <circle cx="18" cy="6" r="2.5" />
              <circle cx="18" cy="18" r="2.5" />
              <path d="M8.5 11.2L15.6 7.2M8.5 12.8L15.6 16.8" strokeLinecap="round" />
            </svg>
          </div>
          <span
            lang="en"
            className="relay-text-trim hidden text-sm leading-none font-semibold tracking-tight text-foreground md:inline"
          >
            {t("app.name")}
          </span>
        </div>

        <span
          lang="en"
          aria-hidden="true"
          className="relay-text-trim hidden text-xs leading-none text-border select-none md:inline"
        >
          /
        </span>

        <WorkspaceSwitcher
          workspace={workspace}
          recentWorkspaces={recentWorkspaces}
          headCommitSha={headCommitSha}
          isSwitching={isSwitchingWorkspace}
          isInitializingManifest={isInitializingManifest}
          onSwitchWorkspace={onSwitchWorkspace}
          onRemoveRecentWorkspace={onRemoveRecentWorkspace}
          onInitManifest={onInitManifest}
          onOpenRepoSettings={onOpenRepoSettings}
        />
      </div>

      <nav aria-label={t("desk.views")} className="shrink-0">
        <Tabs
          value={activeView}
          onValueChange={(value) => {
            if (value === "tasks" || value === "graph") {
              onViewChange(value);
            }
          }}
          className="gap-0"
        >
          <TabsList className="h-7">
            <TabsTrigger value="tasks" className="gap-1.5 px-2.5">
              <ListTodo aria-hidden="true" />
              <span className="relay-text-trim leading-none">{t("desk.tasks")}</span>
              {attentionCount > 0 && (
                <span className="font-mono-tech inline-flex min-w-4 items-center justify-center rounded-sm bg-primary/15 px-1 text-[10px] leading-4 text-primary">
                  {attentionCount}
                </span>
              )}
            </TabsTrigger>
            <TabsTrigger value="graph" className="gap-1.5 px-2.5">
              <GitBranch aria-hidden="true" />
              <span className="relay-text-trim leading-none">{t("desk.graph")}</span>
            </TabsTrigger>
          </TabsList>
        </Tabs>
      </nav>

      <div className="flex shrink-0 items-center gap-1.5">
        {staleCount > 0 && (
          <Badge variant="destructive" className="hidden gap-1 xl:inline-flex">
            <AlertCircle data-icon="inline-start" aria-hidden="true" />
            <span className="relay-text-trim leading-none">
              {t("topbar.outdatedCount", { count: staleCount })}
            </span>
          </Badge>
        )}

        <Button variant="outline" size="default" onClick={onOpenPairModal}>
          <span
            aria-hidden="true"
            className={cn(
              "size-2 shrink-0 rounded-full",
              isConnected ? "bg-emerald-500" : "bg-amber-500",
            )}
          />
          <span className="relay-text-trim leading-none">
            {isConnected
              ? `${t(`topbar.transport.${transport}`)}${status?.rtt_ms != null ? ` · ${status.rtt_ms}ms` : ""}`
              : t("topbar.pairDevice")}
          </span>
          {pendingOutbox > 0 && (
            <span className="font-mono-tech rounded-sm bg-amber-500/15 px-1 text-[10px] leading-4 text-amber-700 dark:text-amber-300">
              {pendingOutbox}
            </span>
          )}
          <Link2 data-icon="inline-end" className="text-muted-foreground" aria-hidden="true" />
        </Button>

        <Button variant="default" size="default" onClick={onOpenRunTask}>
          <Play data-icon="inline-start" aria-hidden="true" />
          <span className="relay-text-trim leading-none">{t("topbar.runTask")}</span>
        </Button>

        <Button
          variant={isCopilotOpen ? "secondary" : "outline"}
          size="default"
          onClick={onToggleCopilot}
          aria-pressed={isCopilotOpen}
        >
          <Sparkles data-icon="inline-start" className="text-primary" aria-hidden="true" />
          <span className="relay-text-trim leading-none">{t("topbar.copilot")}</span>
        </Button>

        <Separator
          orientation="vertical"
          className="mx-0.5 my-auto h-4 data-vertical:self-center"
        />

        <Popover>
          <PopoverTrigger
            render={
              <Button
                type="button"
                variant="ghost"
                size="default"
                aria-label={t("sidebar.settingsButton")}
                title={`${t("sidebar.modeLabel")}: ${t(`sidebar.mode.${activeMode}`)}`}
                className="px-2 text-muted-foreground hover:text-foreground"
              />
            }
          >
            <ModeIcon mode={activeMode} />
            <span className="relay-text-trim hidden leading-none lg:inline">
              {t(`sidebar.mode.${activeMode}`)}
            </span>
          </PopoverTrigger>
          <PopoverContent align="end" side="bottom" className="w-72 p-3">
            <div className="flex flex-col gap-3">
              <div className="flex flex-col gap-1.5">
                <div className="text-[11px] font-medium text-muted-foreground">
                  {t("sidebar.modeLabel")}
                </div>
                <div className="grid grid-cols-3 gap-1 rounded-md bg-muted p-0.5">
                  {RUNTIME_MODES.map((mode) => {
                    const active = mode === activeMode;
                    return (
                      <button
                        key={mode}
                        type="button"
                        disabled={isSwitchingMode}
                        aria-pressed={active}
                        onClick={() => onSwitchMode(mode)}
                        className={cn(
                          "cursor-pointer rounded-sm px-1.5 py-1 text-[11px] font-medium transition-all disabled:pointer-events-none disabled:opacity-50",
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
                {switchModeError && (
                  <Alert
                    role="alert"
                    variant="destructive"
                    className="border-destructive/30 bg-[var(--relay-stale-soft)] py-1.5"
                  >
                    <AlertCircle className="size-3.5" aria-hidden="true" />
                    <AlertDescription className="text-[11px] leading-snug break-words">
                      {switchModeError}
                    </AlertDescription>
                    {onDismissSwitchModeError && (
                      <AlertAction>
                        <Button
                          type="button"
                          size="icon-xs"
                          variant="ghost"
                          aria-label={t("detail.close")}
                          onClick={onDismissSwitchModeError}
                        >
                          <X />
                        </Button>
                      </AlertAction>
                    )}
                  </Alert>
                )}
              </div>

              {import.meta.env.DEV && (
                <div className="flex flex-col gap-1.5 border-t border-border pt-2.5">
                  <div className="text-[11px] font-medium text-muted-foreground">
                    {t("sidebar.devTools")}
                  </div>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="w-full justify-start"
                    disabled={isSimulatingEdit}
                    onClick={onTriggerUpstreamEdit}
                  >
                    <span className="relay-text-trim leading-none">
                      {t("sidebar.simulateCodeChange")}
                    </span>
                  </Button>
                  {!isTauriRuntime() && (
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      className="w-full justify-start"
                      onClick={onResetDemo}
                    >
                      <span className="relay-text-trim leading-none">
                        {t("sidebar.resetPreview")}
                      </span>
                    </Button>
                  )}
                </div>
              )}
            </div>
          </PopoverContent>
        </Popover>

        <Button
          variant="ghost"
          size="default"
          aria-label={t("topbar.switchLang")}
          onClick={() => setRelayLocale(currentLang === "en" ? "zh-CN" : "en")}
        >
          <Globe data-icon="inline-start" aria-hidden="true" />
          <span
            lang={currentLang === "en" ? "zh-CN" : "en"}
            className="relay-text-trim leading-none"
          >
            {currentLang === "en" ? "中文" : "EN"}
          </span>
        </Button>

        <Button
          variant="ghost"
          size="icon"
          aria-label={t("topbar.openSettings")}
          title={t("topbar.openSettings")}
          onClick={onOpenLlmSettings}
        >
          <SlidersHorizontal />
        </Button>

        <Button
          variant="ghost"
          size="icon"
          aria-label={t("topbar.toggleTheme")}
          onClick={onToggleTheme}
        >
          {theme === "light" ? <Moon /> : <Sun />}
        </Button>
      </div>
    </header>
  );
}
