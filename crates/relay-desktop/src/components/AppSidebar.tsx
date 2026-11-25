import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import {
  Cpu,
  GitBranch,
  Globe,
  Laptop,
  Link2,
  ListTodo,
  MonitorSmartphone,
  Moon,
  PanelLeftClose,
  Settings,
  Sun,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import type {
  PeerStatus_Serialize,
  RecentWorkspaceEntry_Serialize,
  RuntimeMode,
  WorkspaceInfo_Serialize,
} from "../bindings";
import { setRelayLocale, type RelayLocale } from "../i18n";
import { BrandMark } from "./BrandMark";
import { WorkspaceSwitcher } from "./WorkspaceSwitcher";

export type AppNavView = "tasks" | "graph" | "devices" | "settings";

interface AppSidebarProps {
  isOpen: boolean;
  onCollapse: () => void;
  activeView: AppNavView;
  onViewChange: (view: AppNavView) => void;
  attentionCount: number;
  totalTaskCount: number;
  staleCount: number;
  status: PeerStatus_Serialize | undefined;
  workspace: WorkspaceInfo_Serialize | undefined;
  recentWorkspaces: RecentWorkspaceEntry_Serialize[];
  headCommitSha: string | null;
  theme: "light" | "dark";
  onToggleTheme: () => void;
  isSwitchingWorkspace: boolean;
  isInitializingManifest: boolean;
  onSwitchWorkspace: (repoPath: string) => Promise<void>;
  onRemoveRecentWorkspace: (repoPath: string) => Promise<void>;
  onInitManifest: () => Promise<void>;
}

function RoleIcon({ mode }: { mode: RuntimeMode }) {
  if (mode === "host") return <Cpu className="size-3.5 shrink-0" aria-hidden="true" />;
  if (mode === "client") return <Laptop className="size-3.5 shrink-0" aria-hidden="true" />;
  return <MonitorSmartphone className="size-3.5 shrink-0" aria-hidden="true" />;
}

export function AppSidebar({
  isOpen,
  onCollapse,
  activeView,
  onViewChange,
  attentionCount,
  totalTaskCount,
  staleCount,
  status,
  workspace,
  recentWorkspaces,
  headCommitSha,
  theme,
  onToggleTheme,
  isSwitchingWorkspace,
  isInitializingManifest,
  onSwitchWorkspace,
  onRemoveRecentWorkspace,
  onInitManifest,
}: AppSidebarProps) {
  const { t, i18n } = useTranslation();
  if (!isOpen) return null;

  const currentLang: RelayLocale = i18n.language === "zh-CN" ? "zh-CN" : "en";
  const activeMode: RuntimeMode = status?.mode ?? "local_dual_sandbox";
  const isConnected = status?.connected ?? false;
  const transport = status?.transport_path ?? "offline";
  const pendingOutbox = status?.pending_outbox_count ?? 0;

  const statusText = isConnected
    ? `${t(`topbar.transport.${transport}`)}${status?.rtt_ms != null ? ` · ${status.rtt_ms}ms` : ""}`
    : activeMode === "local_dual_sandbox"
      ? t("topbar.transport.loopback")
      : t("topbar.offline");

  const navItems: Array<{
    id: AppNavView;
    label: string;
    icon: typeof ListTodo;
    badge?: { text: string; tone: "primary" | "destructive" | "amber" | "muted" };
    dot?: "connected" | "offline" | "local";
  }> = [
    {
      id: "tasks",
      label: t("desk.tasks"),
      icon: ListTodo,
      badge:
        attentionCount > 0
          ? { text: String(attentionCount), tone: "primary" }
          : totalTaskCount > 0
            ? { text: String(totalTaskCount), tone: "muted" }
            : undefined,
    },
    {
      id: "graph",
      label: t("desk.graph"),
      icon: GitBranch,
      badge: staleCount > 0 ? { text: String(staleCount), tone: "destructive" } : undefined,
    },
    {
      id: "devices",
      label: t("nav.devices"),
      icon: Link2,
      badge: pendingOutbox > 0 ? { text: String(pendingOutbox), tone: "amber" } : undefined,
      dot: isConnected ? "connected" : activeMode === "local_dual_sandbox" ? "local" : "offline",
    },
    {
      id: "settings",
      label: t("nav.settings"),
      icon: Settings,
    },
  ];

  return (
    <aside className="flex h-full w-[240px] shrink-0 flex-col border-r border-sidebar-border bg-sidebar text-sidebar-foreground select-none">
      <div className="flex h-12 shrink-0 items-center justify-between gap-2 border-b border-sidebar-border px-3">
        <div className="flex min-w-0 items-center gap-2.5">
          <BrandMark className="size-6 shrink-0" />
          <span
            lang="en"
            className="truncate text-sm leading-snug font-semibold tracking-tight text-sidebar-foreground"
          >
            {t("app.name")}
          </span>
        </div>

        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          aria-label={t("nav.collapseSidebar")}
          title={t("nav.collapseSidebar")}
          onClick={onCollapse}
          className="text-muted-foreground hover:text-foreground"
        >
          <PanelLeftClose />
        </Button>
      </div>

      <div className="px-2.5 pt-2.5 pb-2">
        <WorkspaceSwitcher
          workspace={workspace}
          recentWorkspaces={recentWorkspaces}
          headCommitSha={headCommitSha}
          isSwitching={isSwitchingWorkspace}
          isInitializingManifest={isInitializingManifest}
          onSwitchWorkspace={onSwitchWorkspace}
          onRemoveRecentWorkspace={onRemoveRecentWorkspace}
          onInitManifest={onInitManifest}
        />
      </div>

      <nav aria-label={t("desk.views")} className="flex flex-1 flex-col gap-1 px-2.5 py-1.5">
        {navItems.map((item) => {
          const active = activeView === item.id;
          const Icon = item.icon;
          return (
            <button
              key={item.id}
              type="button"
              aria-current={active ? "page" : undefined}
              onClick={() => onViewChange(item.id)}
              className={cn(
                "flex h-9 w-full cursor-pointer items-center justify-between gap-2 rounded-md px-2.5 text-[13.5px] leading-snug font-medium transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/40",
                active
                  ? "bg-sidebar-accent text-sidebar-accent-foreground shadow-2xs"
                  : "text-muted-foreground hover:bg-sidebar-accent/60 hover:text-sidebar-foreground",
              )}
            >
              <span className="flex min-w-0 items-center gap-2.5">
                <Icon
                  className={cn(
                    "size-4 shrink-0",
                    active ? "text-foreground" : "text-muted-foreground",
                  )}
                  aria-hidden="true"
                />
                <span className="truncate">{item.label}</span>
              </span>

              <span className="flex shrink-0 items-center gap-1.5">
                {item.dot && (
                  <span
                    aria-hidden="true"
                    className={cn(
                      "size-1.5 rounded-full",
                      item.dot === "connected"
                        ? "bg-emerald-500"
                        : item.dot === "local"
                          ? "bg-sky-500"
                          : "bg-amber-500",
                    )}
                  />
                )}
                {item.badge && (
                  <span
                    className={cn(
                      "font-mono-tech inline-flex min-w-[18px] items-center justify-center rounded-sm px-1 text-[10.5px] leading-4",
                      item.badge.tone === "primary" && "bg-primary/15 text-primary",
                      item.badge.tone === "destructive" && "bg-destructive/15 text-destructive",
                      item.badge.tone === "amber" &&
                        "bg-amber-500/15 text-amber-700 dark:text-amber-300",
                      item.badge.tone === "muted" && "text-muted-foreground",
                    )}
                  >
                    {item.badge.text}
                  </span>
                )}
              </span>
            </button>
          );
        })}
      </nav>

      <div className="flex flex-col gap-1.5 border-t border-sidebar-border p-2.5">
        <button
          type="button"
          onClick={() => onViewChange("devices")}
          className={cn(
            "flex w-full cursor-pointer items-center justify-between gap-2 rounded-md border px-2.5 py-2 text-left transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/40",
            activeView === "devices"
              ? "border-primary/30 bg-primary/6"
              : "border-sidebar-border bg-background/60 hover:bg-background",
          )}
        >
          <div className="flex min-w-0 items-center gap-2">
            <span className="flex size-6 shrink-0 items-center justify-center rounded-md bg-muted text-foreground">
              <RoleIcon mode={activeMode} />
            </span>
            <div className="flex min-w-0 flex-col gap-0.5">
              <span className="truncate text-xs leading-snug font-medium text-foreground">
                {t(`sidebar.mode.${activeMode}`)}
              </span>
              <span className="truncate text-[11px] leading-snug text-muted-foreground">
                {statusText}
              </span>
            </div>
          </div>
          <span
            aria-hidden="true"
            className={cn(
              "size-2 shrink-0 rounded-full",
              isConnected
                ? "bg-emerald-500"
                : activeMode === "local_dual_sandbox"
                  ? "bg-sky-500"
                  : "bg-amber-500",
            )}
          />
        </button>

        <div className="flex items-center justify-between gap-1 pt-0.5">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            aria-label={t("topbar.switchLang")}
            onClick={() => setRelayLocale(currentLang === "en" ? "zh-CN" : "en")}
            className="h-7 px-2 text-xs text-muted-foreground hover:text-foreground"
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
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={t("topbar.toggleTheme")}
            title={t("topbar.toggleTheme")}
            onClick={onToggleTheme}
            className="text-muted-foreground hover:text-foreground"
          >
            {theme === "light" ? <Moon /> : <Sun />}
          </Button>
        </div>
      </div>
    </aside>
  );
}
