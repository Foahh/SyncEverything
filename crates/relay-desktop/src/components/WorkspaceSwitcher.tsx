import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTitle, PopoverTrigger } from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import {
  AlertCircle,
  AlertTriangle,
  Check,
  ChevronsUpDown,
  FilePlus2,
  FolderGit2,
  FolderOpen,
  GitBranch,
  SlidersHorizontal,
  X,
} from "lucide-react";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { RecentWorkspaceEntry_Serialize, WorkspaceInfo_Serialize } from "../bindings";
import { formatErrorMessage } from "../lib/formatError";
import { isTauriRuntime, relayApi } from "../lib/relayApi";
import {
  MAX_RECENT_WORKSPACES,
  isActiveWorkspacePath,
  sortAndCapRecentWorkspaces,
  truncateWorkspacePath,
} from "../lib/workspaceSwitcher";

interface WorkspaceSwitcherProps {
  workspace: WorkspaceInfo_Serialize | undefined;
  recentWorkspaces: RecentWorkspaceEntry_Serialize[];
  headCommitSha: string | null;
  isSwitching: boolean;
  isInitializingManifest: boolean;
  onSwitchWorkspace: (repoPath: string) => Promise<void>;
  onRemoveRecentWorkspace: (repoPath: string) => Promise<void>;
  onInitManifest: () => Promise<void>;
  onOpenRepoSettings: () => void;
}

export function WorkspaceSwitcher({
  workspace,
  recentWorkspaces,
  headCommitSha,
  isSwitching,
  isInitializingManifest,
  onSwitchWorkspace,
  onRemoveRecentWorkspace,
  onInitManifest,
  onOpenRepoSettings,
}: WorkspaceSwitcherProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [isPickingFolder, setIsPickingFolder] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  const entries = useMemo(
    () => sortAndCapRecentWorkspaces(recentWorkspaces, MAX_RECENT_WORKSPACES),
    [recentWorkspaces],
  );

  const hasActiveRepo = Boolean(workspace?.repo_root);
  const activeName = hasActiveRepo ? workspace?.name || workspace?.repo_id : null;
  const activePm = hasActiveRepo ? (workspace?.package_manager ?? null) : null;
  const needsManifest =
    hasActiveRepo && Boolean(workspace?.is_git_repo) && !workspace?.has_manifest;

  const handleOpenChange = (nextOpen: boolean) => {
    setOpen(nextOpen);
    if (nextOpen) {
      setActionError(null);
    }
  };

  const handleSelectWorkspace = async (repoPath: string) => {
    setActionError(null);
    try {
      await onSwitchWorkspace(repoPath);
      setOpen(false);
    } catch (error) {
      setActionError(formatErrorMessage(error, t, t("workspaceSwitcher.switchFailed")));
    }
  };

  const handleRemoveEntry = async (repoPath: string) => {
    setActionError(null);
    try {
      await onRemoveRecentWorkspace(repoPath);
    } catch (error) {
      setActionError(formatErrorMessage(error, t));
    }
  };

  const handleOpenFolder = async () => {
    setActionError(null);
    if (!isTauriRuntime()) {
      setOpen(false);
      onOpenRepoSettings();
      return;
    }
    setIsPickingFolder(true);
    try {
      const selected = await relayApi.pickRepositoryFolder();
      if (selected) {
        await onSwitchWorkspace(selected);
        setOpen(false);
      }
    } catch (error) {
      setActionError(formatErrorMessage(error, t, t("workspaceSwitcher.switchFailed")));
    } finally {
      setIsPickingFolder(false);
    }
  };

  const handleInitManifest = async () => {
    setActionError(null);
    try {
      await onInitManifest();
      setOpen(false);
    } catch (error) {
      setActionError(formatErrorMessage(error, t, t("workspaceSwitcher.initFailed")));
    }
  };

  return (
    <Popover open={open} onOpenChange={handleOpenChange}>
      <PopoverTrigger
        render={
          <Button
            variant="ghost"
            size="default"
            title={workspace?.repo_root ?? t("workspaceSwitcher.triggerLabel")}
            aria-label={t("workspaceSwitcher.triggerLabel")}
            className="min-w-0 max-w-[340px] gap-1.5 px-2"
          />
        }
      >
        <GitBranch data-icon="inline-start" className="text-muted-foreground" />
        <span
          lang={activeName ? "en" : undefined}
          className="truncate text-xs leading-snug font-medium text-foreground"
        >
          {activeName ?? t("topbar.noRepo")}
        </span>
        {activePm && (
          <Badge
            variant="secondary"
            className="font-mono-tech h-4 shrink-0 px-1.5 text-[10px] leading-none"
          >
            <span lang="en" className="relay-text-trim leading-none">
              {activePm}
            </span>
          </Badge>
        )}
        {headCommitSha && (
          <span
            lang="en"
            className="font-mono-tech relay-text-trim hidden rounded-sm bg-muted px-1.5 py-1 text-[11px] leading-none text-muted-foreground sm:inline-flex sm:items-center"
          >
            {headCommitSha.slice(0, 7)}
          </span>
        )}
        <ChevronsUpDown data-icon="inline-end" className="text-muted-foreground" />
      </PopoverTrigger>

      <PopoverContent
        align="start"
        side="bottom"
        sideOffset={6}
        collisionPadding={12}
        className="w-[356px] max-w-[calc(100vw-2rem)] overflow-hidden"
      >
        <div className="flex items-center justify-between gap-2 border-b border-border px-3 py-2">
          <PopoverTitle>{t("workspaceSwitcher.panelTitle")}</PopoverTitle>
          <span className="font-mono-tech text-[11px] text-muted-foreground">
            {t("workspaceSwitcher.recentCount", {
              count: entries.length,
              max: MAX_RECENT_WORKSPACES,
            })}
          </span>
        </div>

        {actionError && (
          <div className="border-b border-border p-2">
            <Alert
              role="alert"
              variant="destructive"
              className="border-destructive/30 bg-[var(--relay-stale-soft)] py-2"
            >
              <AlertCircle />
              <AlertDescription className="text-[11px] leading-snug">
                {actionError}
              </AlertDescription>
            </Alert>
          </div>
        )}

        <div className="flex max-h-72 flex-col gap-1 overflow-y-auto p-1.5">
          {entries.length === 0 ? (
            <div className="flex flex-col items-center gap-1 rounded-lg border border-dashed border-border px-3 py-5 text-center">
              <div className="text-xs font-medium text-foreground">
                {t("workspaceSwitcher.emptyTitle")}
              </div>
              <p className="text-[11px] leading-relaxed text-muted-foreground">
                {t("workspaceSwitcher.emptyDesc")}
              </p>
            </div>
          ) : (
            entries.map((entry) => {
              const isActive = isActiveWorkspacePath(entry.path, workspace?.repo_root);
              const isMissing = !entry.exists;
              const displayName = entry.name || entry.repo_id;
              const truncatedPath = truncateWorkspacePath(entry.path, 42);

              return (
                <div
                  key={entry.path}
                  className={cn(
                    "group flex items-center gap-1 rounded-lg border px-1.5 py-1 transition-colors",
                    isActive
                      ? "border-primary/30 bg-primary/6"
                      : isMissing
                        ? "border-destructive/20 bg-destructive/5"
                        : "border-transparent hover:bg-muted/70",
                  )}
                >
                  <button
                    type="button"
                    disabled={isMissing || isSwitching}
                    onClick={() => void handleSelectWorkspace(entry.path)}
                    title={isMissing ? t("workspaceSwitcher.missingHint") : entry.path}
                    className="flex min-w-0 flex-1 cursor-pointer items-start gap-2 rounded-md px-1 py-1 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/40 disabled:cursor-not-allowed disabled:opacity-65"
                  >
                    <span
                      aria-hidden="true"
                      className="mt-0.5 flex size-4 shrink-0 items-center justify-center"
                    >
                      {isActive ? (
                        <Check className="size-3.5 text-emerald-600 dark:text-emerald-400" />
                      ) : isMissing ? (
                        <AlertTriangle className="size-3.5 text-destructive" />
                      ) : (
                        <FolderGit2 className="size-3.5 text-muted-foreground" />
                      )}
                    </span>

                    <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                      <div className="flex min-w-0 items-center gap-1.5">
                        <span
                          lang="en"
                          className="truncate text-xs leading-snug font-medium text-foreground"
                        >
                          {displayName}
                        </span>
                        {entry.package_manager && (
                          <Badge
                            variant="outline"
                            className="font-mono-tech h-4 shrink-0 px-1.5 text-[10px] leading-none"
                          >
                            <span lang="en" className="relay-text-trim leading-none">
                              {entry.package_manager}
                            </span>
                          </Badge>
                        )}
                        {isMissing ? (
                          <Badge
                            variant="destructive"
                            className="h-4 shrink-0 px-1.5 text-[10px] leading-none"
                          >
                            <span className="relay-text-trim leading-none">
                              {t("workspaceSwitcher.missingBadge")}
                            </span>
                          </Badge>
                        ) : !entry.has_manifest ? (
                          <Badge
                            variant="secondary"
                            className="h-4 shrink-0 bg-amber-500/15 px-1.5 text-[10px] leading-none text-amber-700 dark:bg-amber-500/20 dark:text-amber-300"
                          >
                            <span className="relay-text-trim leading-none">
                              {t("workspaceSwitcher.noManifestBadge")}
                            </span>
                          </Badge>
                        ) : null}
                      </div>

                      <span
                        lang="en"
                        title={entry.path}
                        className="font-mono-tech truncate text-[10.5px] leading-snug text-muted-foreground"
                      >
                        {truncatedPath}
                      </span>
                    </div>
                  </button>

                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-xs"
                    aria-label={t("workspaceSwitcher.removeAria", { name: displayName })}
                    title={t("workspaceSwitcher.removeBtn")}
                    onClick={() => void handleRemoveEntry(entry.path)}
                    className="shrink-0 text-muted-foreground hover:text-destructive"
                  >
                    <X />
                  </Button>
                </div>
              );
            })
          )}
        </div>

        <div className="flex flex-col gap-1.5 border-t border-border bg-muted/30 p-2">
          {needsManifest && (
            <Button
              type="button"
              variant="default"
              size="default"
              disabled={isInitializingManifest}
              onClick={() => void handleInitManifest()}
              className="w-full justify-center"
            >
              <FilePlus2 data-icon="inline-start" />
              <span className="relay-text-trim leading-none">
                {isInitializingManifest
                  ? t("workspaceSwitcher.initializingManifest")
                  : t("workspaceSwitcher.initManifest")}
              </span>
            </Button>
          )}

          <div className="flex items-center gap-1.5">
            <Button
              type="button"
              variant="outline"
              size="default"
              disabled={isPickingFolder || isSwitching}
              onClick={() => void handleOpenFolder()}
              className="flex-1 justify-start"
            >
              <FolderOpen data-icon="inline-start" />
              <span className="relay-text-trim leading-none">
                {isSwitching ? t("workspaceSwitcher.switching") : t("workspaceSwitcher.openFolder")}
              </span>
            </Button>

            <Button
              type="button"
              variant="ghost"
              size="icon"
              aria-label={t("workspaceSwitcher.repoSettings")}
              title={t("workspaceSwitcher.repoSettings")}
              onClick={() => {
                setOpen(false);
                onOpenRepoSettings();
              }}
            >
              <SlidersHorizontal />
            </Button>
          </div>
        </div>
      </PopoverContent>
    </Popover>
  );
}
