import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { AlertCircle, AlertTriangle, ArrowRight, FolderGit2, FolderOpen, X } from "lucide-react";
import { useMemo, useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import type { RecentWorkspaceEntry_Serialize } from "../bindings";
import { formatErrorMessage } from "../lib/formatError";
import { isTauriRuntime, relayApi } from "../lib/relayApi";
import {
  MAX_RECENT_WORKSPACES,
  sortAndCapRecentWorkspaces,
  truncateWorkspacePath,
} from "../lib/workspaceSwitcher";

interface WorkspaceWelcomeProps {
  recentWorkspaces: RecentWorkspaceEntry_Serialize[];
  isSwitching: boolean;
  onSwitchWorkspace: (repoPath: string) => Promise<void>;
  onRemoveRecentWorkspace: (repoPath: string) => Promise<void>;
}

export function WorkspaceWelcome({
  recentWorkspaces,
  isSwitching,
  onSwitchWorkspace,
  onRemoveRecentWorkspace,
}: WorkspaceWelcomeProps) {
  const { t } = useTranslation();
  const isDesktop = isTauriRuntime();
  const [repoPath, setRepoPath] = useState("");
  const [isPicking, setIsPicking] = useState(false);
  const [errorText, setErrorText] = useState<string | null>(null);

  const entries = useMemo(
    () => sortAndCapRecentWorkspaces(recentWorkspaces, MAX_RECENT_WORKSPACES),
    [recentWorkspaces],
  );

  const handleOpenPath = async (targetPath: string) => {
    const trimmed = targetPath.trim();
    if (!trimmed) return;
    setErrorText(null);
    try {
      await onSwitchWorkspace(trimmed);
      setRepoPath("");
    } catch (error) {
      setErrorText(formatErrorMessage(error, t, t("modals.workspace.errors.openRepo")));
    }
  };

  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    void handleOpenPath(repoPath);
  };

  const handlePickFolder = async () => {
    setErrorText(null);
    setIsPicking(true);
    try {
      const selected = await relayApi.pickRepositoryFolder();
      if (selected) {
        await handleOpenPath(selected);
      }
    } catch (error) {
      setErrorText(formatErrorMessage(error, t, t("modals.workspace.errors.pickFolder")));
    } finally {
      setIsPicking(false);
    }
  };

  const handleRemove = async (targetPath: string) => {
    setErrorText(null);
    try {
      await onRemoveRecentWorkspace(targetPath);
    } catch (error) {
      setErrorText(formatErrorMessage(error, t));
    }
  };

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto overscroll-contain bg-background [scrollbar-gutter:stable]">
      <div className="flex w-full max-w-2xl flex-col gap-8 px-8 py-10">
        <div className="flex flex-col gap-2">
          <div className="flex items-center gap-2.5">
            <div className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
              <FolderGit2 className="size-4.5" aria-hidden="true" />
            </div>
            <div>
              <h2 className="text-base font-semibold tracking-tight text-foreground">
                {t("welcome.title")}
              </h2>
              <p className="text-xs text-muted-foreground">{t("welcome.subtitle")}</p>
            </div>
          </div>
        </div>

        <div className="flex flex-col gap-3 rounded-lg border border-border bg-card p-5">
          {isDesktop && (
            <div className="flex items-center justify-between gap-3">
              <div className="min-w-0">
                <div className="text-xs font-medium text-foreground">
                  {t("welcome.folderTitle")}
                </div>
                <p className="text-[11.5px] text-muted-foreground">{t("welcome.folderDesc")}</p>
              </div>
              <Button
                type="button"
                size="default"
                disabled={isPicking || isSwitching}
                onClick={() => void handlePickFolder()}
                className="shrink-0"
              >
                <FolderOpen data-icon="inline-start" aria-hidden="true" />
                <span className="relay-text-trim leading-none">
                  {t("workspaceSwitcher.openFolder")}
                </span>
              </Button>
            </div>
          )}

          {isDesktop && <div className="border-t border-border/70" />}

          <form onSubmit={handleSubmit} className="flex flex-col gap-2">
            <label htmlFor="welcome-repo-path" className="text-xs font-medium text-foreground">
              {t("modals.workspace.repoPathLabel")}
            </label>
            <div className="flex items-center gap-2">
              <Input
                id="welcome-repo-path"
                value={repoPath}
                onChange={(event) => setRepoPath(event.target.value)}
                placeholder={t("modals.workspace.repoPathPlaceholder")}
                autoCorrect="off"
                autoCapitalize="none"
                spellCheck={false}
                className="font-mono-tech flex-1 text-xs"
              />
              <Button
                type="submit"
                variant={isDesktop ? "outline" : "default"}
                size="default"
                disabled={!repoPath.trim() || isSwitching}
              >
                <span className="relay-text-trim leading-none">
                  {t("modals.workspace.openRepoBtn")}
                </span>
                <ArrowRight data-icon="inline-end" aria-hidden="true" />
              </Button>
            </div>
            <p className="text-[11px] text-muted-foreground">{t("welcome.pathHint")}</p>
          </form>

          {errorText && (
            <Alert
              role="alert"
              variant="destructive"
              className="border-destructive/30 bg-[var(--relay-stale-soft)]"
            >
              <AlertCircle aria-hidden="true" />
              <AlertDescription>{errorText}</AlertDescription>
            </Alert>
          )}
        </div>

        {entries.length > 0 && (
          <section className="flex flex-col gap-2">
            <h3 className="text-xs font-medium text-muted-foreground">
              {t("workspaceSwitcher.panelTitle")}
            </h3>
            <ul
              role="list"
              className="divide-y divide-border overflow-hidden rounded-lg border border-border bg-card"
            >
              {entries.map((entry) => {
                const isMissing = !entry.exists;
                const displayName = entry.name || entry.repo_id;
                return (
                  <li
                    key={entry.path}
                    className="flex items-center justify-between gap-3 px-3.5 py-2.5 transition-colors hover:bg-muted/40"
                  >
                    <button
                      type="button"
                      disabled={isMissing || isSwitching}
                      onClick={() => void handleOpenPath(entry.path)}
                      className="flex min-w-0 flex-1 cursor-pointer items-center gap-3 text-left outline-none focus-visible:underline disabled:cursor-not-allowed disabled:opacity-60"
                    >
                      {isMissing ? (
                        <AlertTriangle
                          className="size-4 shrink-0 text-destructive"
                          aria-hidden="true"
                        />
                      ) : (
                        <FolderGit2
                          className="size-4 shrink-0 text-muted-foreground"
                          aria-hidden="true"
                        />
                      )}
                      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                        <div className="flex min-w-0 items-center gap-2">
                          <span lang="en" className="truncate text-xs font-medium text-foreground">
                            {displayName}
                          </span>
                          {entry.package_manager && (
                            <Badge
                              variant="outline"
                              className="font-mono-tech h-4 px-1.5 text-[10px]"
                            >
                              {entry.package_manager}
                            </Badge>
                          )}
                          {isMissing ? (
                            <Badge variant="destructive" className="h-4 px-1.5 text-[10px]">
                              {t("workspaceSwitcher.missingBadge")}
                            </Badge>
                          ) : !entry.has_manifest ? (
                            <Badge
                              variant="secondary"
                              className="h-4 bg-amber-500/15 px-1.5 text-[10px] text-amber-700 dark:text-amber-300"
                            >
                              {t("workspaceSwitcher.noManifestBadge")}
                            </Badge>
                          ) : null}
                        </div>
                        <span
                          lang="en"
                          title={entry.path}
                          className="font-mono-tech truncate text-[11px] text-muted-foreground"
                        >
                          {truncateWorkspacePath(entry.path, 68)}
                        </span>
                      </div>
                    </button>

                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-xs"
                      aria-label={t("workspaceSwitcher.removeAria", { name: displayName })}
                      onClick={() => void handleRemove(entry.path)}
                      className="shrink-0 text-muted-foreground hover:text-destructive"
                    >
                      <X />
                    </Button>
                  </li>
                );
              })}
            </ul>
          </section>
        )}
      </div>
    </div>
  );
}
