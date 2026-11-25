import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { cn } from "@/lib/utils";
import { AlertCircle, PanelLeftOpen, PanelRight, PanelRightClose, Play, Plus } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AppNavView } from "./AppSidebar";

interface WorkspaceHeaderProps {
  activeView: AppNavView;
  hasRepo: boolean;
  repoName: string | null;
  headCommitSha: string | null;
  staleCount: number;
  sidebarOpen: boolean;
  onExpandSidebar: () => void;
  rightRailOpen: boolean;
  hasRightRailActivity: boolean;
  onToggleRightRail: () => void;
  onOpenRunTask: () => void;
  onOpenRecordDecision: () => void;
}

export function WorkspaceHeader({
  activeView,
  hasRepo,
  repoName,
  headCommitSha,
  staleCount,
  sidebarOpen,
  onExpandSidebar,
  rightRailOpen,
  hasRightRailActivity,
  onToggleRightRail,
  onOpenRunTask,
  onOpenRecordDecision,
}: WorkspaceHeaderProps) {
  const { t } = useTranslation();

  const viewTitle =
    activeView === "tasks"
      ? hasRepo
        ? t("desk.tasks")
        : t("welcome.headerTitle")
      : activeView === "graph"
        ? hasRepo
          ? t("desk.graph")
          : t("welcome.headerTitle")
        : activeView === "devices"
          ? t("nav.devices")
          : t("nav.settings");

  const showRepoContext = (activeView === "tasks" || activeView === "graph") && hasRepo;

  return (
    <header className="flex h-12 shrink-0 items-center justify-between gap-3 border-b border-border bg-background px-4">
      <div className="flex min-w-0 items-center gap-2">
        {!sidebarOpen && (
          <>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              aria-label={t("nav.expandSidebar")}
              title={t("nav.expandSidebar")}
              onClick={onExpandSidebar}
              className="text-muted-foreground hover:text-foreground"
            >
              <PanelLeftOpen />
            </Button>
            <Separator
              orientation="vertical"
              className="mx-0.5 my-auto h-4 data-vertical:self-center"
            />
          </>
        )}

        <h1 className="truncate text-sm font-semibold tracking-tight text-foreground">
          {viewTitle}
        </h1>

        {showRepoContext && repoName && (
          <span lang="en" className="hidden truncate text-xs text-muted-foreground sm:inline">
            / {repoName}
          </span>
        )}

        {showRepoContext && headCommitSha && (
          <code
            lang="en"
            title={headCommitSha}
            className="font-mono-tech hidden shrink-0 rounded-sm bg-muted px-1.5 py-0.5 text-[11px] text-muted-foreground md:inline"
          >
            {headCommitSha.slice(0, 7)}
          </code>
        )}

        {showRepoContext && staleCount > 0 && (
          <Badge variant="destructive" className="hidden gap-1 lg:inline-flex">
            <AlertCircle data-icon="inline-start" aria-hidden="true" />
            <span className="relay-text-trim leading-none">
              {t("topbar.outdatedCount", { count: staleCount })}
            </span>
          </Badge>
        )}
      </div>

      <div className="flex shrink-0 items-center gap-1.5">
        {showRepoContext && activeView === "graph" && (
          <Button type="button" variant="outline" size="default" onClick={onOpenRecordDecision}>
            <Plus data-icon="inline-start" aria-hidden="true" />
            <span className="relay-text-trim leading-none">{t("canvas.dock.addNote")}</span>
          </Button>
        )}

        {showRepoContext && (
          <Button type="button" variant="default" size="default" onClick={onOpenRunTask}>
            <Play data-icon="inline-start" aria-hidden="true" />
            <span className="relay-text-trim leading-none">{t("topbar.runTask")}</span>
          </Button>
        )}

        {showRepoContext && (
          <Separator
            orientation="vertical"
            className="mx-0.5 my-auto h-4 data-vertical:self-center"
          />
        )}

        <Button
          type="button"
          variant={rightRailOpen ? "secondary" : "ghost"}
          size="icon-sm"
          aria-pressed={rightRailOpen}
          aria-label={t("nav.toggleRightPanel")}
          title={t("nav.toggleRightPanelHint")}
          onClick={onToggleRightRail}
          className={cn(
            "relative",
            rightRailOpen ? "text-foreground" : "text-muted-foreground hover:text-foreground",
          )}
        >
          {rightRailOpen ? <PanelRightClose /> : <PanelRight />}
          {!rightRailOpen && hasRightRailActivity && (
            <span
              aria-hidden="true"
              className="absolute top-1 right-1 size-1.5 rounded-full bg-primary"
            />
          )}
        </Button>
      </div>
    </header>
  );
}
