import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Field, FieldDescription, FieldGroup, FieldLabel, FieldTitle } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  buildMcpSnippet,
  computeJsonLineDiff,
  DEFAULT_MCP_HTTP_URL,
  getMcpClientConfigPath,
  toProjectMcpClient,
  validateMcpHttpUrl,
  type McpClientId,
} from "@/lib/mcpConfig";
import { cn } from "@/lib/utils";
import {
  AlertCircle,
  Check,
  CheckCircle2,
  Copy,
  Cpu,
  Eye,
  EyeOff,
  FileCode2,
  FilePlus2,
  FolderGit2,
  Globe,
  Moon,
  RefreshCw,
  RotateCcw,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
  Sun,
  Terminal,
  Trash2,
} from "lucide-react";
import { useEffect, useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import type {
  InferenceTier,
  LlmSettingsUpdate_Deserialize,
  LlmSettingsView_Serialize,
  McpTransportMode,
  ProjectMcpConfigPreview_Serialize,
  ProjectMcpConfigRequest_Deserialize,
  ProjectMcpFileStatus,
  WorkspaceInfo_Serialize,
} from "../bindings";
import { setRelayLocale, type RelayLocale } from "../i18n";
import {
  formatErrorMessage,
  formatLlmProbeMessage,
  isMcpPreviewChangedError,
} from "../lib/formatError";
import {
  useInitWorkspaceManifest,
  useLlmSettings,
  usePreviewProjectMcpConfig,
  useRefreshGitHead,
  useSaveLlmSettings,
  useWorkspaceInfo,
  useWriteProjectMcpConfig,
} from "../lib/queries";
import { relayApi } from "../lib/relayApi";

export type SettingsSection = "llm" | "mcp" | "workspace" | "general";

interface SettingsViewProps {
  activeSection: SettingsSection;
  onSectionChange: (section: SettingsSection) => void;
  theme: "light" | "dark";
  onToggleTheme: () => void;
  onGoToWorkspace: () => void;
}

interface FeedbackState {
  ok: boolean;
  text: string;
}

interface StatusBannerAction {
  label: string;
  onClick: () => void;
  disabled?: boolean;
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

const REASONING_EFFORT_OPTIONS = [
  { value: "", labelKey: "modals.workspace.reasoningEffortOption.default" },
  { value: "auto", labelKey: "modals.workspace.reasoningEffortOption.auto" },
  { value: "none", labelKey: "modals.workspace.reasoningEffortOption.none" },
  { value: "low", labelKey: "modals.workspace.reasoningEffortOption.low" },
  { value: "medium", labelKey: "modals.workspace.reasoningEffortOption.medium" },
  { value: "high", labelKey: "modals.workspace.reasoningEffortOption.high" },
] as const;

const MCP_CLIENTS: Array<{
  id: McpClientId;
  label: string;
  configPath: string;
}> = [
  {
    id: "claude",
    label: "Claude Code",
    configPath: getMcpClientConfigPath("claude"),
  },
  {
    id: "cursor",
    label: "Cursor",
    configPath: getMcpClientConfigPath("cursor"),
  },
  {
    id: "codex",
    label: "Codex CLI",
    configPath: getMcpClientConfigPath("codex"),
  },
];

const MCP_STATUS_BADGE_CLASSES: Record<ProjectMcpFileStatus, string> = {
  create_new: "bg-sky-500/12 text-sky-700 dark:bg-sky-500/20 dark:text-sky-300",
  add_server: "bg-emerald-500/12 text-emerald-700 dark:bg-emerald-500/20 dark:text-emerald-300",
  update_server: "bg-amber-500/12 text-amber-700 dark:bg-amber-500/20 dark:text-amber-300",
  unchanged: "bg-muted text-muted-foreground",
};

function StatusBanner({
  feedback,
  action,
}: {
  feedback: FeedbackState | null;
  action?: StatusBannerAction | null;
}) {
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
        <CheckCircle2 className="text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
      ) : (
        <AlertCircle aria-hidden="true" />
      )}
      <AlertDescription
        className={cn(
          "flex flex-col gap-2",
          feedback.ok && "text-emerald-800 dark:text-emerald-200",
        )}
      >
        <span className="break-words">{feedback.text}</span>
        {action && (
          <div>
            <Button
              type="button"
              variant="outline"
              size="xs"
              disabled={action.disabled}
              onClick={action.onClick}
            >
              <RefreshCw
                data-icon="inline-start"
                className={action.disabled ? "animate-spin" : undefined}
                aria-hidden="true"
              />
              <span className="relay-text-trim leading-none">{action.label}</span>
            </Button>
          </div>
        )}
      </AlertDescription>
    </Alert>
  );
}

function LlmSettingsSection({ initial }: { initial: LlmSettingsView_Serialize }) {
  const { t } = useTranslation();
  const [tier, setTier] = useState<InferenceTier>(initial.tier);
  const [baseUrl, setBaseUrl] = useState(initial.base_url);
  const [model, setModel] = useState(initial.model);
  const [apiKeyInput, setApiKeyInput] = useState("");
  const [clearStoredKey, setClearStoredKey] = useState(false);
  const [apiKeyEnv, setApiKeyEnv] = useState(initial.api_key_env ?? "");
  const [reasoningEffort, setReasoningEffort] = useState(initial.reasoning_effort ?? "");
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
      reasoning_effort: reasoningEffort.trim() || null,
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
        setFeedback({
          ok: false,
          text: formatErrorMessage(err, t, t("modals.workspace.errors.saveLlm")),
        });
      },
    });
  };

  const handleTest = async () => {
    setFeedback(null);
    setIsTesting(true);
    try {
      const probe = await relayApi.testLlmConnection(buildUpdate());
      setFeedback({ ok: probe.ok, text: formatLlmProbeMessage(probe.message, t) });
    } catch (err) {
      setFeedback({
        ok: false,
        text: formatErrorMessage(err, t, t("modals.workspace.errors.testLlm")),
      });
    } finally {
      setIsTesting(false);
    }
  };

  const visibleReasoningOptions = REASONING_EFFORT_OPTIONS.filter(
    (option) =>
      option.value !== "auto" || tier !== "cloud_open_ai_compat" || reasoningEffort === "auto",
  );
  const hasCustomReasoning =
    Boolean(initial.reasoning_effort) &&
    !REASONING_EFFORT_OPTIONS.some((option) => option.value === initial.reasoning_effort);

  return (
    <form onSubmit={handleSave} className="flex min-w-0 flex-col gap-5">
      <div>
        <h2 className="text-sm font-semibold text-foreground">{t("modals.workspace.llmTab")}</h2>
        <p className="mt-0.5 text-xs text-muted-foreground">{t("settings.llmSubtitle")}</p>
      </div>

      <FieldGroup>
        <Field>
          <FieldTitle>{t("modals.workspace.tierLabel")}</FieldTitle>
          <FieldDescription>{t("settings.tierHint")}</FieldDescription>
          <div className="grid grid-cols-1 gap-2 sm:grid-cols-3">
            {TIER_ORDER.map((option) => {
              const active = tier === option;
              return (
                <button
                  key={option}
                  type="button"
                  aria-pressed={active}
                  onClick={() => handleSelectTier(option)}
                  className={cn(
                    "cursor-pointer rounded-lg border px-3 py-2.5 text-left transition-all",
                    active
                      ? "border-primary bg-primary/8 ring-1 ring-primary/25"
                      : "border-border bg-card hover:border-foreground/20",
                  )}
                >
                  <div className="flex items-center gap-1.5 text-xs font-semibold text-foreground">
                    {option === "cloud_open_ai_compat" ? (
                      <Sparkles className="size-3.5 shrink-0 text-primary" aria-hidden="true" />
                    ) : (
                      <Cpu className="size-3.5 shrink-0 text-primary" aria-hidden="true" />
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
          <FieldDescription>{t("settings.baseUrlHint")}</FieldDescription>
          <Input
            id="llm-base-url"
            name="base_url"
            type="url"
            required
            value={baseUrl}
            onChange={(e) => setBaseUrl(e.target.value)}
            placeholder="http://127.0.0.1:11434/v1"
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
                <Trash2 className="size-3 shrink-0" aria-hidden="true" />
                <span className="relay-text-trim leading-none">
                  {t("modals.workspace.clearKeyBtn")}
                </span>
              </button>
            )}
          </div>
          {initial.has_api_key && !clearStoredKey && initial.api_key_hint ? (
            <FieldDescription>
              {t("modals.workspace.apiKeyStoredHint", { hint: initial.api_key_hint })}
            </FieldDescription>
          ) : (
            <FieldDescription>{t("settings.apiKeyHint")}</FieldDescription>
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
          <FieldTitle>{t("modals.workspace.reasoningEffortLabel")}</FieldTitle>
          <FieldDescription>{t("settings.reasoningEffortHint")}</FieldDescription>
          <div
            className="flex flex-wrap gap-1.5"
            role="group"
            aria-label={t("modals.workspace.reasoningEffortLabel")}
          >
            {visibleReasoningOptions.map((option) => {
              const active = reasoningEffort === option.value;
              return (
                <button
                  key={option.value || "default"}
                  type="button"
                  aria-pressed={active}
                  onClick={() => setReasoningEffort(option.value)}
                  className={cn(
                    "cursor-pointer rounded-md border px-2.5 py-1.5 text-center text-[11.5px] font-medium transition-all",
                    active
                      ? "border-primary bg-primary/8 text-foreground ring-1 ring-primary/25"
                      : "border-border bg-card text-muted-foreground hover:border-foreground/20 hover:text-foreground",
                  )}
                >
                  <span className="relay-text-trim whitespace-nowrap leading-none">
                    {t(option.labelKey)}
                  </span>
                </button>
              );
            })}
            {hasCustomReasoning && initial.reasoning_effort && (
              <button
                type="button"
                aria-pressed={reasoningEffort === initial.reasoning_effort}
                onClick={() => setReasoningEffort(initial.reasoning_effort ?? "")}
                className={cn(
                  "cursor-pointer rounded-md border px-2.5 py-1.5 text-center text-[11.5px] font-medium transition-all",
                  reasoningEffort === initial.reasoning_effort
                    ? "border-primary bg-primary/8 text-foreground ring-1 ring-primary/25"
                    : "border-border bg-card text-muted-foreground hover:border-foreground/20 hover:text-foreground",
                )}
              >
                <span className="relay-text-trim whitespace-nowrap leading-none">
                  {initial.reasoning_effort}
                </span>
              </button>
            )}
          </div>
        </Field>

        <Field>
          <FieldLabel htmlFor="llm-api-key-env">{t("modals.workspace.apiKeyEnvLabel")}</FieldLabel>
          <FieldDescription>{t("settings.apiKeyEnvHint")}</FieldDescription>
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
        <Button type="submit" size="default" disabled={saveLlm.isPending}>
          <span className="relay-text-trim leading-none">{t("modals.workspace.saveBtn")}</span>
        </Button>
        <Button
          type="button"
          variant="outline"
          size="default"
          disabled={isTesting}
          onClick={() => void handleTest()}
        >
          <Sparkles data-icon="inline-start" aria-hidden="true" />
          <span className="relay-text-trim leading-none">
            {isTesting ? t("modals.workspace.testingBtn") : t("modals.workspace.testBtn")}
          </span>
        </Button>
      </div>
    </form>
  );
}

function McpSettingsSection({
  workspace,
  onGoToWorkspace,
}: {
  workspace?: WorkspaceInfo_Serialize;
  onGoToWorkspace: () => void;
}) {
  const { t } = useTranslation();
  const [client, setClient] = useState<McpClientId>("claude");
  const [transport, setTransport] = useState<McpTransportMode>("stdio");
  const [httpUrl, setHttpUrl] = useState(DEFAULT_MCP_HTTP_URL);
  const [copyFeedback, setCopyFeedback] = useState<{ snippet: string; ok: boolean } | null>(null);
  const [previewState, setPreviewState] = useState<{
    key: string;
    data: ProjectMcpConfigPreview_Serialize;
  } | null>(null);
  const [writeFeedback, setWriteFeedback] = useState<
    (FeedbackState & { needsRefresh?: boolean }) | null
  >(null);

  const previewMutation = usePreviewProjectMcpConfig();
  const writeMutation = useWriteProjectMcpConfig();

  const selectedClient = MCP_CLIENTS.find((item) => item.id === client) ?? MCP_CLIENTS[0];
  const projectClient = toProjectMcpClient(client);
  const httpValidation = validateMcpHttpUrl(httpUrl);
  const snippet = buildMcpSnippet(client, workspace, {
    transport,
    httpUrl: transport === "streamable_http" ? httpUrl : null,
  });
  const copied = copyFeedback?.snippet === snippet && copyFeedback.ok;

  const currentRequestKey = JSON.stringify([
    workspace?.repo_root,
    workspace?.repo_id,
    workspace?.mcp_data_dir,
    client,
    transport,
    transport === "streamable_http" ? httpUrl.trim() : null,
  ]);
  const activePreview =
    previewState && previewState.key === currentRequestKey ? previewState.data : null;

  const canPreviewProject =
    projectClient !== null &&
    Boolean(workspace?.repo_root) &&
    (transport === "stdio" ? Boolean(workspace?.mcp_data_dir) : httpValidation.ok);

  const diffLines =
    activePreview && activePreview.existing_content !== null && activePreview.status !== "unchanged"
      ? computeJsonLineDiff(activePreview.existing_content, activePreview.merged_content)
      : null;

  useEffect(() => {
    if (!copyFeedback?.ok) return;
    const id = window.setTimeout(() => {
      setCopyFeedback((current) => (current?.ok ? null : current));
    }, 1800);
    return () => window.clearTimeout(id);
  }, [copyFeedback]);

  const buildProjectRequest = (): ProjectMcpConfigRequest_Deserialize | null => {
    if (!projectClient) return null;
    return {
      client: projectClient,
      transport,
      http_url: transport === "streamable_http" ? httpUrl.trim() : null,
    };
  };

  const handleCopy = async () => {
    if (!snippet) return;
    try {
      await navigator.clipboard.writeText(snippet);
      setCopyFeedback({ snippet, ok: true });
    } catch {
      setCopyFeedback({ snippet, ok: false });
    }
  };

  const handlePreviewProject = () => {
    const request = buildProjectRequest();
    if (!request) return;
    const requestKey = currentRequestKey;
    setWriteFeedback(null);
    previewMutation.mutate(request, {
      onSuccess: (data) => {
        setPreviewState({ key: requestKey, data });
      },
      onError: (err) => {
        setPreviewState(null);
        setWriteFeedback({
          ok: false,
          text: formatErrorMessage(err, t, t("modals.workspace.errors.previewMcp")),
        });
      },
    });
  };

  const handleConfirmWrite = () => {
    const request = buildProjectRequest();
    if (!request || !activePreview) return;
    const requestKey = currentRequestKey;
    setWriteFeedback(null);
    writeMutation.mutate(
      {
        request,
        confirmation: {
          target_path: activePreview.target_path,
          existing_content: activePreview.existing_content,
          merged_content: activePreview.merged_content,
        },
      },
      {
        onSuccess: (result) => {
          setWriteFeedback({
            ok: true,
            text: result.created
              ? t("modals.workspace.mcpWriteSuccessCreated", { path: result.relative_path })
              : t("modals.workspace.mcpWriteSuccessUpdated", { path: result.relative_path }),
          });
          setPreviewState({
            key: requestKey,
            data: {
              ...activePreview,
              file_exists: true,
              status: "unchanged",
              existing_content: result.written_content,
              merged_content: result.written_content,
            },
          });
        },
        onError: (err) => {
          setPreviewState(null);
          setWriteFeedback({
            ok: false,
            text: formatErrorMessage(err, t, t("modals.workspace.errors.writeMcp")),
            needsRefresh: isMcpPreviewChangedError(err),
          });
        },
      },
    );
  };

  const httpValidationErrorText = httpValidation.ok
    ? null
    : t(`modals.workspace.mcpHttpUrlErrors.${httpValidation.code}`);

  const needsRepoForStdio = transport === "stdio" && !snippet;

  return (
    <div className="flex min-w-0 flex-col gap-5">
      <div>
        <h2 className="text-sm font-semibold text-foreground">{t("modals.workspace.mcpTab")}</h2>
        <p className="mt-0.5 text-xs text-muted-foreground">{t("settings.mcpSubtitle")}</p>
      </div>

      <div className="grid min-w-0 grid-cols-1 gap-4 sm:grid-cols-2">
        <div className="flex min-w-0 flex-col gap-1.5">
          <span className="text-xs font-medium text-foreground">
            {t("modals.workspace.mcpClientLabel")}
          </span>
          <div
            role="group"
            aria-label={t("modals.workspace.mcpClientLabel")}
            className="grid grid-cols-3 gap-1 rounded-lg bg-muted p-0.5"
          >
            {MCP_CLIENTS.map((item) => {
              const active = item.id === client;
              return (
                <button
                  key={item.id}
                  type="button"
                  aria-pressed={active}
                  onClick={() => {
                    setClient(item.id);
                    setWriteFeedback(null);
                  }}
                  className={cn(
                    "min-w-0 cursor-pointer truncate rounded-md px-2 py-1.5 text-center text-xs font-medium transition-all",
                    active
                      ? "bg-background text-foreground shadow-2xs"
                      : "text-muted-foreground hover:text-foreground",
                  )}
                >
                  <span lang="en">{item.label}</span>
                </button>
              );
            })}
          </div>
          <p className="text-[11px] text-muted-foreground">
            {t("settings.mcpClientPathHint", { path: selectedClient.configPath })}
          </p>
        </div>

        <div className="flex min-w-0 flex-col gap-1.5">
          <span className="text-xs font-medium text-foreground">
            {t("modals.workspace.mcpTransportLabel")}
          </span>
          <div
            role="group"
            aria-label={t("modals.workspace.mcpTransportLabel")}
            className="grid grid-cols-2 gap-1 rounded-lg bg-muted p-0.5"
          >
            {(
              [
                { id: "stdio", icon: Terminal, titleKey: "modals.workspace.mcpTransportStdio" },
                {
                  id: "streamable_http",
                  icon: Globe,
                  titleKey: "modals.workspace.mcpTransportHttp",
                },
              ] as const
            ).map((option) => {
              const active = transport === option.id;
              const Icon = option.icon;
              return (
                <button
                  key={option.id}
                  type="button"
                  aria-pressed={active}
                  onClick={() => {
                    setTransport(option.id);
                    setWriteFeedback(null);
                  }}
                  className={cn(
                    "flex min-w-0 cursor-pointer items-center justify-center gap-1.5 rounded-md px-2 py-1.5 text-xs font-medium transition-all",
                    active
                      ? "bg-background text-foreground shadow-2xs"
                      : "text-muted-foreground hover:text-foreground",
                  )}
                >
                  <Icon className="size-3 shrink-0" aria-hidden="true" />
                  <span className="truncate">{t(option.titleKey)}</span>
                </button>
              );
            })}
          </div>
          <p className="text-[11px] text-muted-foreground">
            {transport === "stdio"
              ? t("settings.mcpTransportStdioHint")
              : t("settings.mcpTransportHttpHint")}
          </p>
        </div>
      </div>

      {transport === "streamable_http" && (
        <Field>
          <div className="flex items-center justify-between gap-2">
            <FieldLabel htmlFor="mcp-http-url">{t("modals.workspace.mcpHttpUrlLabel")}</FieldLabel>
            {httpUrl !== DEFAULT_MCP_HTTP_URL && (
              <button
                type="button"
                onClick={() => {
                  setHttpUrl(DEFAULT_MCP_HTTP_URL);
                  setWriteFeedback(null);
                }}
                className="inline-flex cursor-pointer items-center gap-1 text-[11px] text-muted-foreground hover:text-foreground"
              >
                <RotateCcw className="size-3 shrink-0" aria-hidden="true" />
                <span className="relay-text-trim leading-none">
                  {t("modals.workspace.mcpHttpUrlReset")}
                </span>
              </button>
            )}
          </div>
          <Input
            id="mcp-http-url"
            name="mcp_http_url"
            type="url"
            value={httpUrl}
            onChange={(e) => {
              setHttpUrl(e.target.value);
              setWriteFeedback(null);
            }}
            placeholder={DEFAULT_MCP_HTTP_URL}
            aria-invalid={!httpValidation.ok}
            autoCorrect="off"
            autoCapitalize="none"
            spellCheck={false}
            className="font-mono-tech text-xs"
          />
          {httpValidationErrorText && (
            <p className="text-[11px] leading-snug text-destructive">{httpValidationErrorText}</p>
          )}
        </Field>
      )}

      {needsRepoForStdio ? (
        <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-dashed border-border bg-muted/25 px-4 py-5">
          <p className="text-xs text-muted-foreground">{t("modals.workspace.mcpOpenRepo")}</p>
          <Button type="button" size="sm" variant="outline" onClick={onGoToWorkspace}>
            <FolderGit2 data-icon="inline-start" aria-hidden="true" />
            <span className="relay-text-trim leading-none">
              {t("modals.workspace.openRepoBtn")}
            </span>
          </Button>
        </div>
      ) : snippet ? (
        <>
          <div className="flex min-w-0 flex-col overflow-hidden rounded-lg border border-border bg-muted/40">
            <div className="flex items-center justify-between gap-2 border-b border-border/70 px-3 py-1.5">
              <span lang="en" className="font-mono-tech truncate text-[11px] text-muted-foreground">
                {selectedClient.configPath}
              </span>
              <Button
                type="button"
                variant="ghost"
                size="xs"
                onClick={() => void handleCopy()}
                className="shrink-0"
              >
                {copied ? (
                  <Check data-icon="inline-start" aria-hidden="true" />
                ) : (
                  <Copy data-icon="inline-start" aria-hidden="true" />
                )}
                <span className="relay-text-trim leading-none">
                  {copied ? t("modals.workspace.mcpCopiedBtn") : t("modals.workspace.mcpCopyBtn")}
                </span>
              </Button>
            </div>
            <pre
              lang="en"
              className="font-mono-tech max-h-56 w-full overflow-x-auto overflow-y-auto p-3 text-[11.5px] leading-relaxed text-foreground"
            >
              {snippet}
            </pre>
          </div>

          {copyFeedback?.snippet === snippet && !copyFeedback.ok && (
            <StatusBanner feedback={{ ok: false, text: t("modals.workspace.mcpCopyFailed") }} />
          )}

          {projectClient && workspace?.repo_root && (
            <div>
              <Button
                type="button"
                size="default"
                disabled={
                  !canPreviewProject || previewMutation.isPending || writeMutation.isPending
                }
                onClick={handlePreviewProject}
              >
                <FileCode2 data-icon="inline-start" aria-hidden="true" />
                <span className="relay-text-trim leading-none">
                  {previewMutation.isPending
                    ? t("modals.workspace.mcpPreviewLoading")
                    : activePreview
                      ? t("modals.workspace.mcpRefreshPreviewBtn")
                      : t("modals.workspace.mcpPreviewBtn")}
                </span>
              </Button>
            </div>
          )}

          {!projectClient && (
            <p className="text-xs text-muted-foreground">
              {t("modals.workspace.mcpCodexCopyOnlyNotice")}
            </p>
          )}

          {transport === "streamable_http" && (
            <p className="text-xs text-muted-foreground">{t("modals.workspace.mcpHttpHint")}</p>
          )}
        </>
      ) : null}

      {activePreview && projectClient && (
        <div className="flex min-w-0 flex-col gap-3 rounded-lg border border-primary/30 bg-muted/40 p-4">
          <div className="flex items-center justify-between gap-2">
            <div className="flex items-center gap-1.5 text-xs font-semibold text-foreground">
              <ShieldCheck className="size-4 shrink-0 text-primary" aria-hidden="true" />
              <span className="relay-text-trim leading-none">
                {t("modals.workspace.mcpPreviewTitle")}
              </span>
            </div>
            <Badge variant="secondary" className={MCP_STATUS_BADGE_CLASSES[activePreview.status]}>
              <span className="relay-text-trim leading-none">
                {t(`modals.workspace.mcpStatus.${activePreview.status}`)}
              </span>
            </Badge>
          </div>

          <div
            lang="en"
            className="font-mono-tech truncate rounded-md bg-background/80 px-2.5 py-1 text-[11px] text-muted-foreground"
            title={activePreview.target_path}
          >
            {activePreview.target_path}
          </div>

          {activePreview.preserved_server_names.length > 0 && (
            <p className="text-[11px] leading-snug text-emerald-700 dark:text-emerald-300">
              {t("modals.workspace.mcpPreviewPreserved", {
                servers: activePreview.preserved_server_names.join(", "),
              })}
            </p>
          )}

          {diffLines && (
            <div className="flex min-w-0 flex-col gap-1">
              <span className="text-[10.5px] font-medium text-muted-foreground">
                {t("modals.workspace.mcpPreviewDiffLabel")}
              </span>
              <pre
                lang="en"
                className="font-mono-tech max-h-40 w-full overflow-x-auto overflow-y-auto rounded-md border border-border bg-background/90 p-2 text-[11px] leading-relaxed"
              >
                {diffLines.map((line, index) => (
                  <div
                    key={`${index}-${line.kind}`}
                    className={cn(
                      "px-1.5 whitespace-pre",
                      line.kind === "added" &&
                        "bg-emerald-500/12 text-emerald-800 dark:text-emerald-300",
                      line.kind === "removed" && "bg-destructive/12 text-destructive",
                      line.kind === "context" && "text-muted-foreground",
                    )}
                  >
                    {line.kind === "added" ? "+ " : line.kind === "removed" ? "- " : "  "}
                    {line.text}
                  </div>
                ))}
              </pre>
            </div>
          )}

          <div className="flex min-w-0 flex-col gap-1">
            <span className="text-[10.5px] font-medium text-muted-foreground">
              {t("modals.workspace.mcpPreviewMergedLabel")}
            </span>
            <pre
              lang="en"
              className="font-mono-tech max-h-48 w-full overflow-x-auto overflow-y-auto rounded-md border border-border bg-background/90 p-2.5 text-[11px] leading-relaxed text-foreground"
            >
              {activePreview.merged_content}
            </pre>
          </div>

          {activePreview.status === "unchanged" && (
            <p className="text-[11px] leading-snug text-muted-foreground">
              {t("modals.workspace.mcpWriteUnchangedHint")}
            </p>
          )}

          <div className="flex items-center justify-end gap-2 pt-0.5">
            <Button
              type="button"
              variant="outline"
              size="default"
              disabled={writeMutation.isPending}
              onClick={() => setPreviewState(null)}
            >
              <span className="relay-text-trim leading-none">
                {t("modals.workspace.mcpCancelPreviewBtn")}
              </span>
            </Button>
            <Button
              type="button"
              size="default"
              disabled={
                writeMutation.isPending ||
                previewMutation.isPending ||
                activePreview.status === "unchanged"
              }
              onClick={handleConfirmWrite}
            >
              <ShieldCheck data-icon="inline-start" aria-hidden="true" />
              <span className="relay-text-trim leading-none">
                {writeMutation.isPending
                  ? t("modals.workspace.mcpWritingBtn")
                  : t("modals.workspace.mcpConfirmWriteBtn")}
              </span>
            </Button>
          </div>
        </div>
      )}

      <StatusBanner
        feedback={writeFeedback}
        action={
          writeFeedback?.needsRefresh && canPreviewProject
            ? {
                label: previewMutation.isPending
                  ? t("modals.workspace.mcpPreviewLoading")
                  : t("modals.workspace.mcpRefreshPreviewBtn"),
                onClick: handlePreviewProject,
                disabled: previewMutation.isPending || writeMutation.isPending,
              }
            : null
        }
      />
    </div>
  );
}

function WorkspaceManifestSection({
  workspace,
  onGoToWorkspace,
}: {
  workspace: WorkspaceInfo_Serialize | undefined;
  onGoToWorkspace: () => void;
}) {
  const { t } = useTranslation();
  const [feedback, setFeedback] = useState<FeedbackState | null>(null);
  const initManifest = useInitWorkspaceManifest();
  const refreshGit = useRefreshGitHead();

  const hasRepo = Boolean(workspace?.repo_root);
  const needsManifest = hasRepo && Boolean(workspace?.is_git_repo) && !workspace?.has_manifest;

  const handleGenerateManifest = () => {
    setFeedback(null);
    initManifest.mutate(undefined, {
      onSuccess: () => {
        setFeedback({ ok: true, text: t("modals.workspace.manifestGenerated") });
      },
      onError: (err) => {
        setFeedback({
          ok: false,
          text: formatErrorMessage(err, t, t("modals.workspace.errors.generateManifest")),
        });
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
        setFeedback({
          ok: false,
          text: formatErrorMessage(err, t, t("modals.workspace.errors.syncGit")),
        });
      },
    });
  };

  return (
    <div className="flex min-w-0 flex-col gap-5">
      <div>
        <h2 className="text-sm font-semibold text-foreground">{t("settings.workspaceTab")}</h2>
        <p className="mt-0.5 text-xs text-muted-foreground">{t("settings.workspaceSubtitle")}</p>
      </div>

      {!hasRepo || !workspace ? (
        <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-dashed border-border bg-muted/25 px-4 py-5">
          <p className="text-xs text-muted-foreground">{t("settings.noActiveWorkspace")}</p>
          <Button type="button" size="sm" variant="outline" onClick={onGoToWorkspace}>
            <FolderGit2 data-icon="inline-start" aria-hidden="true" />
            <span className="relay-text-trim leading-none">
              {t("modals.workspace.openRepoBtn")}
            </span>
          </Button>
        </div>
      ) : (
        <div className="flex flex-col gap-4">
          <div className="flex flex-col gap-3 rounded-lg border border-border bg-card p-4">
            <div className="flex items-center justify-between gap-2">
              <div className="flex min-w-0 items-center gap-2">
                <FolderGit2 className="size-4 shrink-0 text-primary" aria-hidden="true" />
                <span lang="en" className="truncate text-xs font-semibold text-foreground">
                  {workspace.name || workspace.repo_id}
                </span>
              </div>
              <Badge
                variant={workspace.is_git_repo ? "secondary" : "outline"}
                className={
                  workspace.is_git_repo
                    ? "bg-emerald-500/12 text-emerald-700 dark:bg-emerald-500/20 dark:text-emerald-300"
                    : undefined
                }
              >
                <span className="relay-text-trim leading-none">
                  {workspace.is_git_repo
                    ? t("modals.workspace.watchingGit")
                    : t("modals.workspace.notGitRepo")}
                </span>
              </Badge>
            </div>

            <div
              lang="en"
              className="font-mono-tech truncate rounded-md bg-muted/60 px-2.5 py-1.5 text-[11px] text-muted-foreground"
              title={workspace.repo_root ?? undefined}
            >
              {workspace.repo_root}
            </div>

            {workspace.is_git_repo && (
              <div>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={refreshGit.isPending}
                  onClick={handleSyncGit}
                >
                  <RefreshCw
                    data-icon="inline-start"
                    className={refreshGit.isPending ? "animate-spin" : undefined}
                    aria-hidden="true"
                  />
                  <span className="relay-text-trim leading-none">
                    {t("modals.workspace.syncGitBtn")}
                  </span>
                </Button>
              </div>
            )}
          </div>

          <div className="flex flex-col gap-2.5 rounded-lg border border-border bg-card p-4">
            <div>
              <h3 className="text-xs font-semibold text-foreground">
                {t("modals.workspace.presetsDiscovered")}
              </h3>
              <p className="mt-0.5 text-[11.5px] text-muted-foreground">
                {t("settings.manifestHint")}
              </p>
            </div>

            {workspace.has_manifest ? (
              workspace.presets.length > 0 ? (
                <div className="divide-y divide-border overflow-hidden rounded-md border border-border">
                  {workspace.presets.map((preset) => (
                    <div
                      key={preset.name}
                      className="flex flex-col gap-0.5 bg-background/60 px-3 py-2"
                    >
                      <div className="flex items-center justify-between gap-2">
                        <span lang="en" className="font-mono-tech text-xs font-semibold">
                          {preset.name}
                        </span>
                        <code
                          lang="en"
                          className="font-mono-tech truncate text-[11px] text-muted-foreground"
                        >
                          {[preset.program, ...preset.args].join(" ")}
                        </code>
                      </div>
                      {preset.description && (
                        <span className="text-[11px] text-muted-foreground">
                          {preset.description}
                        </span>
                      )}
                    </div>
                  ))}
                </div>
              ) : (
                <p className="text-xs text-muted-foreground">{t("modals.workspace.noPresets")}</p>
              )
            ) : needsManifest ? (
              <div className="flex items-center justify-between gap-3 rounded-md border border-dashed border-primary/40 bg-muted/30 px-3 py-2.5">
                <span className="text-xs text-muted-foreground">
                  {t("modals.workspace.missingManifestHint")}
                </span>
                <Button
                  type="button"
                  size="sm"
                  disabled={initManifest.isPending}
                  onClick={handleGenerateManifest}
                  className="shrink-0"
                >
                  <FilePlus2 data-icon="inline-start" aria-hidden="true" />
                  <span className="relay-text-trim leading-none">
                    {t("modals.workspace.generateManifestBtn")}
                  </span>
                </Button>
              </div>
            ) : null}
          </div>

          <StatusBanner feedback={feedback} />
        </div>
      )}
    </div>
  );
}

function GeneralSettingsSection({
  theme,
  onToggleTheme,
}: {
  theme: "light" | "dark";
  onToggleTheme: () => void;
}) {
  const { t, i18n } = useTranslation();
  const currentLang: RelayLocale = i18n.language === "zh-CN" ? "zh-CN" : "en";

  return (
    <div className="flex min-w-0 flex-col gap-6">
      <div>
        <h2 className="text-sm font-semibold text-foreground">{t("settings.generalTab")}</h2>
        <p className="mt-0.5 text-xs text-muted-foreground">{t("settings.generalSubtitle")}</p>
      </div>

      <div className="divide-y divide-border overflow-hidden rounded-lg border border-border bg-card">
        <div className="flex items-center justify-between gap-4 px-4 py-3.5">
          <div>
            <div className="text-xs font-medium text-foreground">{t("settings.themeLabel")}</div>
            <p className="text-[11.5px] text-muted-foreground">{t("settings.themeDesc")}</p>
          </div>
          <Button type="button" variant="outline" size="sm" onClick={onToggleTheme}>
            {theme === "light" ? (
              <Moon data-icon="inline-start" aria-hidden="true" />
            ) : (
              <Sun data-icon="inline-start" aria-hidden="true" />
            )}
            <span className="relay-text-trim leading-none">
              {theme === "light" ? t("settings.themeLight") : t("settings.themeDark")}
            </span>
          </Button>
        </div>

        <div className="flex items-center justify-between gap-4 px-4 py-3.5">
          <div>
            <div className="text-xs font-medium text-foreground">{t("settings.languageLabel")}</div>
            <p className="text-[11.5px] text-muted-foreground">{t("settings.languageDesc")}</p>
          </div>
          <div className="grid grid-cols-2 gap-1 rounded-md bg-muted p-0.5">
            {(["en", "zh-CN"] as const).map((lang) => {
              const active = currentLang === lang;
              return (
                <button
                  key={lang}
                  type="button"
                  aria-pressed={active}
                  onClick={() => setRelayLocale(lang)}
                  className={cn(
                    "cursor-pointer rounded-sm px-2.5 py-1 text-xs font-medium transition-all",
                    active
                      ? "bg-background text-foreground shadow-2xs"
                      : "text-muted-foreground hover:text-foreground",
                  )}
                >
                  {lang === "en" ? "English" : "中文"}
                </button>
              );
            })}
          </div>
        </div>
      </div>
    </div>
  );
}

export function SettingsView({
  activeSection,
  onSectionChange,
  theme,
  onToggleTheme,
  onGoToWorkspace,
}: SettingsViewProps) {
  const { t } = useTranslation();
  const { data: workspace } = useWorkspaceInfo();
  const { data: llmSettings } = useLlmSettings();

  const sections: Array<{
    id: SettingsSection;
    label: string;
    icon: typeof Cpu;
  }> = [
    { id: "llm", label: t("modals.workspace.llmTab"), icon: Sparkles },
    { id: "mcp", label: t("modals.workspace.mcpTab"), icon: Terminal },
    { id: "workspace", label: t("settings.workspaceTab"), icon: FolderGit2 },
    { id: "general", label: t("settings.generalTab"), icon: SlidersHorizontal },
  ];

  return (
    <div className="flex min-h-0 min-w-0 flex-1 bg-background">
      <nav
        aria-label={t("modals.workspace.title")}
        className="flex w-48 shrink-0 flex-col gap-1 border-r border-border p-3"
      >
        {sections.map((section) => {
          const active = activeSection === section.id;
          const Icon = section.icon;
          return (
            <button
              key={section.id}
              type="button"
              aria-current={active ? "page" : undefined}
              onClick={() => onSectionChange(section.id)}
              className={cn(
                "flex h-9 w-full cursor-pointer items-center gap-2.5 rounded-md px-2.5 text-[13px] leading-snug font-medium transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/40",
                active
                  ? "bg-muted text-foreground"
                  : "text-muted-foreground hover:bg-muted/50 hover:text-foreground",
              )}
            >
              <Icon className="size-4 shrink-0" aria-hidden="true" />
              <span className="truncate">{section.label}</span>
            </button>
          );
        })}
      </nav>

      <div className="min-h-0 min-w-0 flex-1 overflow-y-auto overscroll-contain [scrollbar-gutter:stable]">
        <div className="w-full max-w-2xl px-8 py-6">
          {activeSection === "llm" && llmSettings && (
            <LlmSettingsSection key={workspace?.repo_root ?? "global"} initial={llmSettings} />
          )}

          {activeSection === "mcp" && (
            <McpSettingsSection workspace={workspace} onGoToWorkspace={onGoToWorkspace} />
          )}

          {activeSection === "workspace" && (
            <WorkspaceManifestSection workspace={workspace} onGoToWorkspace={onGoToWorkspace} />
          )}

          {activeSection === "general" && (
            <GeneralSettingsSection theme={theme} onToggleTheme={onToggleTheme} />
          )}
        </div>
      </div>
    </div>
  );
}
