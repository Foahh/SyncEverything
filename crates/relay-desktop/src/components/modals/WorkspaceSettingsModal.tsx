import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldDescription, FieldGroup, FieldLabel, FieldTitle } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";
import {
  AlertCircle,
  CheckCircle2,
  Cpu,
  Eye,
  EyeOff,
  FolderGit2,
  FolderOpen,
  RefreshCw,
  SlidersHorizontal,
  Sparkles,
  Trash2,
} from "lucide-react";
import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import type {
  InferenceTier,
  LlmSettingsUpdate_Deserialize,
  LlmSettingsView_Serialize,
  WorkspaceInfo_Serialize,
} from "../../bindings";
import {
  useLlmSettings,
  useOpenWorkspaceRepo,
  useRefreshGitHead,
  useSaveLlmSettings,
  useWorkspaceInfo,
} from "../../lib/queries";
import { isTauriRuntime, relayApi } from "../../lib/relayApi";

export type SettingsTab = "repo" | "llm";

interface WorkspaceSettingsModalProps {
  isOpen: boolean;
  onOpenChange: (open: boolean) => void;
  activeTab: SettingsTab;
  onTabChange: (tab: SettingsTab) => void;
}

interface FeedbackState {
  ok: boolean;
  text: string;
}

const TIER_DEFAULTS: Record<InferenceTier, { baseUrl: string; model: string }> = {
  host_local_ollama: {
    baseUrl: "http://127.0.0.1:11434/v1",
    model: "qwen3.5:4b",
  },
  cloud_open_ai_compat: {
    baseUrl: "https://api.deepseek.com/v1",
    model: "deepseek-chat",
  },
  edge_fast_path: {
    baseUrl: "http://127.0.0.1:11434/v1",
    model: "qwen3.5:4b",
  },
};

const TIER_ORDER: InferenceTier[] = ["host_local_ollama", "cloud_open_ai_compat", "edge_fast_path"];

function errorMessage(err: unknown, fallback: string): string {
  if (err instanceof Error && err.message) return err.message;
  if (typeof err === "string" && err.trim()) return err;
  return fallback;
}

function StatusBanner({ feedback }: { feedback: FeedbackState | null }) {
  if (!feedback) return null;
  return (
    <Alert
      role="status"
      aria-live="polite"
      variant={feedback.ok ? "default" : "destructive"}
      className={
        feedback.ok
          ? "border-emerald-500/25 bg-[var(--relay-fresh-soft)] text-emerald-800 dark:text-emerald-200"
          : "border-destructive/30 bg-[var(--relay-stale-soft)]"
      }
    >
      {feedback.ok ? (
        <CheckCircle2 className="text-emerald-600 dark:text-emerald-400" />
      ) : (
        <AlertCircle />
      )}
      <AlertDescription
        className={feedback.ok ? "text-emerald-800 dark:text-emerald-200" : undefined}
      >
        {feedback.text}
      </AlertDescription>
    </Alert>
  );
}

function RepositorySettingsTab({ workspace }: { workspace: WorkspaceInfo_Serialize | undefined }) {
  const { t } = useTranslation();
  const [repoPath, setRepoPath] = useState(workspace?.repo_root ?? "");
  const [isPicking, setIsPicking] = useState(false);
  const [feedback, setFeedback] = useState<FeedbackState | null>(null);

  const openRepo = useOpenWorkspaceRepo();
  const refreshGit = useRefreshGitHead();
  const isDesktop = isTauriRuntime();

  const handleBrowse = async () => {
    setFeedback(null);
    setIsPicking(true);
    try {
      const selected = await relayApi.pickRepositoryFolder();
      if (selected) {
        setRepoPath(selected);
      }
    } catch (err) {
      setFeedback({ ok: false, text: errorMessage(err, "Unable to open folder dialog.") });
    } finally {
      setIsPicking(false);
    }
  };

  const handleOpenRepo = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const trimmed = repoPath.trim();
    if (!trimmed) return;
    setFeedback(null);
    openRepo.mutate(trimmed, {
      onSuccess: (info) => {
        setRepoPath(info.repo_root ?? trimmed);
        setFeedback({ ok: true, text: t("modals.workspace.repoOpened") });
      },
      onError: (err) => {
        setFeedback({ ok: false, text: errorMessage(err, "Could not open repository.") });
      },
    });
  };

  const handleSyncGit = () => {
    setFeedback(null);
    refreshGit.mutate(undefined, {
      onSuccess: (changed) => {
        setFeedback({
          ok: true,
          text: changed ? t("modals.workspace.gitSynced") : t("modals.workspace.gitUpToDate"),
        });
      },
      onError: (err) => {
        setFeedback({ ok: false, text: errorMessage(err, "Git sync failed.") });
      },
    });
  };

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-col gap-2.5 rounded-lg border border-border bg-muted/40 p-3">
        <div className="flex items-center justify-between gap-2">
          <div className="flex min-w-0 items-center gap-2">
            <FolderGit2 className="size-4 shrink-0 text-primary" />
            <span
              lang={workspace?.repo_root ? "en" : undefined}
              className="truncate text-xs leading-snug font-semibold text-foreground"
            >
              {workspace?.repo_root
                ? workspace.name || workspace.repo_id
                : t("modals.workspace.noRepoSelected")}
            </span>
          </div>
          <Badge
            variant={workspace?.is_git_repo ? "secondary" : "outline"}
            className={
              workspace?.is_git_repo
                ? "bg-emerald-500/12 text-emerald-700 dark:bg-emerald-500/20 dark:text-emerald-300"
                : undefined
            }
          >
            <span className="relay-text-trim leading-none">
              {workspace?.is_git_repo
                ? t("modals.workspace.watchingGit")
                : t("modals.workspace.notGitRepo")}
            </span>
          </Badge>
        </div>

        {workspace?.repo_root && (
          <div
            lang="en"
            className="font-mono-tech truncate rounded-md bg-background/80 px-2 py-1 text-[11px] text-muted-foreground"
            title={workspace.repo_root}
          >
            {workspace.repo_root}
          </div>
        )}

        <div className="flex flex-col gap-1.5">
          <span className="text-[11px] font-medium text-muted-foreground">
            {t("modals.workspace.presetsDiscovered")}
          </span>
          {workspace && workspace.presets.length > 0 ? (
            <div className="flex flex-wrap gap-1.5">
              {workspace.presets.map((preset) => (
                <Badge key={preset.name} variant="outline" className="font-mono-tech">
                  <span lang="en" className="relay-text-trim leading-none">
                    {preset.name}
                  </span>
                </Badge>
              ))}
            </div>
          ) : (
            <span className="text-[11px] text-muted-foreground">
              {t("modals.workspace.noPresets")}
            </span>
          )}
        </div>
      </div>

      <form onSubmit={handleOpenRepo} className="flex flex-col gap-3">
        <FieldGroup>
          <Field>
            <FieldLabel htmlFor="workspace-repo-path">
              {t("modals.workspace.repoPathLabel")}
            </FieldLabel>
            <FieldDescription>{t("modals.workspace.repoPathHint")}</FieldDescription>
            <div className="flex items-center gap-2">
              <Input
                id="workspace-repo-path"
                name="repo_path"
                required
                value={repoPath}
                onChange={(e) => setRepoPath(e.target.value)}
                placeholder={t("modals.workspace.repoPathPlaceholder")}
                autoCorrect="off"
                autoCapitalize="none"
                spellCheck={false}
                className="font-mono-tech text-xs"
              />
              <Button
                type="button"
                variant="outline"
                size="default"
                disabled={!isDesktop || isPicking}
                onClick={() => void handleBrowse()}
              >
                <FolderOpen data-icon="inline-start" />
                <span className="relay-text-trim leading-none">
                  {t("modals.workspace.browseBtn")}
                </span>
              </Button>
            </div>
          </Field>
        </FieldGroup>

        {!isDesktop && (
          <p className="text-[11px] leading-relaxed text-muted-foreground">
            {t("modals.workspace.browserPreviewHint")}
          </p>
        )}

        <StatusBanner feedback={feedback} />

        <div className="flex items-center gap-2">
          <Button type="submit" size="lg" className="flex-1" disabled={openRepo.isPending}>
            <FolderGit2 data-icon="inline-start" />
            <span className="relay-text-trim leading-none">
              {t("modals.workspace.openRepoBtn")}
            </span>
          </Button>
          <Button
            type="button"
            variant="outline"
            size="lg"
            disabled={!workspace?.is_git_repo || refreshGit.isPending}
            onClick={handleSyncGit}
          >
            <RefreshCw
              data-icon="inline-start"
              className={refreshGit.isPending ? "animate-spin" : undefined}
            />
            <span className="relay-text-trim leading-none">{t("modals.workspace.syncGitBtn")}</span>
          </Button>
        </div>
      </form>
    </div>
  );
}

function LlmSettingsTab({ initial }: { initial: LlmSettingsView_Serialize }) {
  const { t } = useTranslation();
  const [tier, setTier] = useState<InferenceTier>(initial.tier);
  const [baseUrl, setBaseUrl] = useState(initial.base_url);
  const [model, setModel] = useState(initial.model);
  const [apiKeyInput, setApiKeyInput] = useState("");
  const [clearStoredKey, setClearStoredKey] = useState(false);
  const [apiKeyEnv, setApiKeyEnv] = useState(initial.api_key_env ?? "");
  const [showKey, setShowKey] = useState(false);
  const [isTesting, setIsTesting] = useState(false);
  const [feedback, setFeedback] = useState<FeedbackState | null>(null);

  const saveLlm = useSaveLlmSettings();

  const buildUpdate = (): LlmSettingsUpdate_Deserialize => {
    const trimmedKey = apiKeyInput.trim();
    const api_key = trimmedKey.length > 0 ? trimmedKey : clearStoredKey ? "" : null;
    return {
      tier,
      base_url: baseUrl.trim(),
      model: model.trim(),
      api_key_env: apiKeyEnv.trim() || null,
      api_key,
    };
  };

  const handleSelectTier = (nextTier: InferenceTier) => {
    setTier(nextTier);
    const preset = TIER_DEFAULTS[nextTier];
    const currentIsPreset = Object.values(TIER_DEFAULTS).some(
      (item) => item.baseUrl === baseUrl.trim(),
    );
    if (!baseUrl.trim() || currentIsPreset) {
      setBaseUrl(preset.baseUrl);
    }
    const currentModelIsPreset = Object.values(TIER_DEFAULTS).some(
      (item) => item.model === model.trim(),
    );
    if (!model.trim() || currentModelIsPreset) {
      setModel(preset.model);
    }
  };

  const handleSave = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setFeedback(null);
    saveLlm.mutate(buildUpdate(), {
      onSuccess: () => {
        setApiKeyInput("");
        setClearStoredKey(false);
        setFeedback({ ok: true, text: t("modals.workspace.savedSuccess") });
      },
      onError: (err) => {
        setFeedback({ ok: false, text: errorMessage(err, "Unable to save AI settings.") });
      },
    });
  };

  const handleTest = async () => {
    setFeedback(null);
    setIsTesting(true);
    try {
      const probe = await relayApi.testLlmConnection(buildUpdate());
      setFeedback({ ok: probe.ok, text: probe.message });
    } catch (err) {
      setFeedback({ ok: false, text: errorMessage(err, "Connection probe failed.") });
    } finally {
      setIsTesting(false);
    }
  };

  return (
    <form onSubmit={handleSave} className="flex flex-col gap-4">
      <FieldGroup>
        <Field>
          <FieldTitle>{t("modals.workspace.tierLabel")}</FieldTitle>
          <div className="grid grid-cols-3 gap-2">
            {TIER_ORDER.map((option) => {
              const active = tier === option;
              return (
                <button
                  key={option}
                  type="button"
                  onClick={() => handleSelectTier(option)}
                  className={cn(
                    "cursor-pointer rounded-lg border px-2.5 py-2 text-left transition-all",
                    active
                      ? "border-primary bg-primary/8 ring-1 ring-primary/25"
                      : "border-border bg-muted/40 hover:border-foreground/20",
                  )}
                >
                  <div className="flex items-center gap-1.5 text-xs font-semibold text-foreground">
                    {option === "cloud_open_ai_compat" ? (
                      <Sparkles className="size-3.5 shrink-0 text-primary" />
                    ) : (
                      <Cpu className="size-3.5 shrink-0 text-primary" />
                    )}
                    <span className="relay-text-trim whitespace-nowrap leading-none">
                      {t(`modals.workspace.tier.${option}`)}
                    </span>
                  </div>
                </button>
              );
            })}
          </div>
        </Field>

        <Field>
          <FieldLabel htmlFor="llm-base-url">{t("modals.workspace.baseUrlLabel")}</FieldLabel>
          <FieldDescription>{t("modals.workspace.baseUrlHint")}</FieldDescription>
          <Input
            id="llm-base-url"
            name="base_url"
            type="url"
            required
            value={baseUrl}
            onChange={(e) => setBaseUrl(e.target.value)}
            autoCorrect="off"
            autoCapitalize="none"
            spellCheck={false}
            className="font-mono-tech text-xs"
          />
        </Field>

        <Field>
          <FieldLabel htmlFor="llm-model">{t("modals.workspace.modelLabel")}</FieldLabel>
          <Input
            id="llm-model"
            name="model"
            required
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder={t("modals.workspace.modelPlaceholder")}
            autoCorrect="off"
            autoCapitalize="none"
            spellCheck={false}
            className="font-mono-tech text-xs"
          />
        </Field>

        <Field>
          <div className="flex items-center justify-between gap-2">
            <FieldLabel htmlFor="llm-api-key">{t("modals.workspace.apiKeyLabel")}</FieldLabel>
            {initial.has_api_key && !clearStoredKey && (
              <button
                type="button"
                onClick={() => {
                  setClearStoredKey(true);
                  setApiKeyInput("");
                }}
                className="inline-flex cursor-pointer items-center gap-1 text-[11px] text-muted-foreground hover:text-destructive"
              >
                <Trash2 className="size-3 shrink-0" />
                <span className="relay-text-trim leading-none">
                  {t("modals.workspace.clearKeyBtn")}
                </span>
              </button>
            )}
          </div>
          {initial.has_api_key && !clearStoredKey && initial.api_key_hint && (
            <FieldDescription>
              {t("modals.workspace.apiKeyStoredHint", { hint: initial.api_key_hint })}
            </FieldDescription>
          )}
          <div className="flex items-center gap-2">
            <Input
              id="llm-api-key"
              name="api_key"
              type={showKey ? "text" : "password"}
              autoComplete="new-password"
              autoCorrect="off"
              autoCapitalize="none"
              spellCheck={false}
              value={apiKeyInput}
              onChange={(e) => {
                setApiKeyInput(e.target.value);
                if (clearStoredKey) setClearStoredKey(false);
              }}
              placeholder={t("modals.workspace.apiKeyPlaceholder")}
              className="font-mono-tech text-xs"
            />
            <Button
              type="button"
              variant="outline"
              size="icon"
              aria-pressed={showKey}
              aria-label={showKey ? t("modals.workspace.hideKey") : t("modals.workspace.showKey")}
              onClick={() => setShowKey((prev) => !prev)}
            >
              {showKey ? <EyeOff /> : <Eye />}
            </Button>
          </div>
        </Field>

        <Field>
          <FieldLabel htmlFor="llm-api-key-env">{t("modals.workspace.apiKeyEnvLabel")}</FieldLabel>
          <Input
            id="llm-api-key-env"
            name="api_key_env"
            value={apiKeyEnv}
            onChange={(e) => setApiKeyEnv(e.target.value)}
            placeholder={t("modals.workspace.apiKeyEnvPlaceholder")}
            autoCorrect="off"
            autoCapitalize="none"
            spellCheck={false}
            className="font-mono-tech text-xs"
          />
        </Field>
      </FieldGroup>

      <StatusBanner feedback={feedback} />

      <div className="flex items-center gap-2">
        <Button
          type="button"
          variant="outline"
          size="lg"
          disabled={isTesting}
          onClick={() => void handleTest()}
        >
          <Sparkles data-icon="inline-start" />
          <span className="relay-text-trim leading-none">
            {isTesting ? t("modals.workspace.testingBtn") : t("modals.workspace.testBtn")}
          </span>
        </Button>
        <Button type="submit" size="lg" className="flex-1" disabled={saveLlm.isPending}>
          <span className="relay-text-trim leading-none">{t("modals.workspace.saveBtn")}</span>
        </Button>
      </div>
    </form>
  );
}

export function WorkspaceSettingsModal({
  isOpen,
  onOpenChange,
  activeTab,
  onTabChange,
}: WorkspaceSettingsModalProps) {
  const { t } = useTranslation();
  const { data: workspace } = useWorkspaceInfo();
  const { data: llmSettings } = useLlmSettings();

  return (
    <Dialog open={isOpen} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <div className="flex items-center gap-2.5">
            <div className="flex size-8 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary">
              <SlidersHorizontal className="size-4" />
            </div>
            <div className="flex flex-col gap-0.5">
              <DialogTitle>{t("modals.workspace.title")}</DialogTitle>
              <DialogDescription>{t("modals.workspace.subtitle")}</DialogDescription>
            </div>
          </div>
        </DialogHeader>

        <Tabs
          value={activeTab}
          onValueChange={(value) => onTabChange(value === "llm" ? "llm" : "repo")}
          className="gap-3"
        >
          <TabsList className="grid w-full grid-cols-2">
            <TabsTrigger value="repo">
              <span className="relay-text-trim leading-none">{t("modals.workspace.repoTab")}</span>
            </TabsTrigger>
            <TabsTrigger value="llm">
              <span className="relay-text-trim leading-none">{t("modals.workspace.llmTab")}</span>
            </TabsTrigger>
          </TabsList>

          <TabsContent value="repo">
            <RepositorySettingsTab
              key={workspace?.repo_root ?? "default-repo"}
              workspace={workspace}
            />
          </TabsContent>

          <TabsContent value="llm">
            {llmSettings && (
              <LlmSettingsTab
                key={`${llmSettings.base_url}|${llmSettings.model}|${llmSettings.has_api_key}`}
                initial={llmSettings}
              />
            )}
          </TabsContent>
        </Tabs>

        <DialogFooter>
          <Button type="button" variant="outline" size="lg" onClick={() => onOpenChange(false)}>
            <span className="relay-text-trim leading-none">{t("modals.workspace.close")}</span>
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
