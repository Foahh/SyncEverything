import { Alert, AlertAction, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupTextarea,
} from "@/components/ui/input-group";
import {
  MessageScroller,
  MessageScrollerButton,
  MessageScrollerContent,
  MessageScrollerItem,
  MessageScrollerProvider,
  MessageScrollerViewport,
} from "@/components/ui/message-scroller";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import {
  AlertCircle,
  ArrowRight,
  ArrowUp,
  ChevronsUpDown,
  Cpu,
  FolderGit2,
  SlidersHorizontal,
  Square,
  X,
} from "lucide-react";
import { useId, useState, type FormEvent, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import type {
  AgentChatMessage_Serialize,
  InferenceTier,
  LlmSettingsView_Serialize,
  PeerStatus_Serialize,
  WorkspaceInfo_Serialize,
} from "../bindings";
import { formatErrorMessage, isLlmConfigurationError } from "../lib/formatError";
import { useSaveLlmSettings } from "../lib/queries";
import type { ApprovalInput } from "../lib/relayApi";
import { CopilotMessageItem } from "./CopilotMessageItem";

interface AgentCopilotPanelProps {
  workspace: WorkspaceInfo_Serialize | undefined;
  status: PeerStatus_Serialize | undefined;
  headCommitSha: string | null;
  staleCount: number;
  attentionCount: number;
  messages: AgentChatMessage_Serialize[];
  isStreaming: boolean;
  streamingMessageId: string | null;
  sendError: string | null;
  onDismissSendError?: () => void;
  llmSettings: LlmSettingsView_Serialize | undefined;
  onSendMessage: (prompt: string) => Promise<boolean>;
  onCancelTurn: () => Promise<void>;
  onResolveApproval: (approval: ApprovalInput) => void;
  isResolvingApproval: boolean;
  onOpenLlmSettings: () => void;
}

const TIER_ORDER: InferenceTier[] = ["host_local_ollama", "cloud_open_ai_compat", "edge_fast_path"];

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

const QUICK_EFFORTS = [
  { value: "", labelKey: "modals.workspace.reasoningEffortOption.default" },
  { value: "none", labelKey: "modals.workspace.reasoningEffortOption.none" },
  { value: "low", labelKey: "modals.workspace.reasoningEffortOption.low" },
  { value: "medium", labelKey: "modals.workspace.reasoningEffortOption.medium" },
  { value: "high", labelKey: "modals.workspace.reasoningEffortOption.high" },
] as const;

function findPrecedingUserPrompt(
  messages: AgentChatMessage_Serialize[],
  index: number,
): string | null {
  for (let i = index - 1; i >= 0; i -= 1) {
    if (messages[i].role === "user" && messages[i].content.trim().length > 0) {
      return messages[i].content;
    }
  }
  return null;
}

function QuickModelPopover({
  llmSettings,
  onOpenLlmSettings,
}: {
  llmSettings: LlmSettingsView_Serialize | undefined;
  onOpenLlmSettings: () => void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [tier, setTier] = useState<InferenceTier>(llmSettings?.tier ?? "host_local_ollama");
  const [model, setModel] = useState(llmSettings?.model ?? "");
  const [effort, setEffort] = useState(llmSettings?.reasoning_effort ?? "");
  const [errorText, setErrorText] = useState<string | null>(null);
  const saveLlm = useSaveLlmSettings();

  const handleOpenChange = (nextOpen: boolean) => {
    setOpen(nextOpen);
    if (nextOpen && llmSettings) {
      setTier(llmSettings.tier);
      setModel(llmSettings.model);
      setEffort(llmSettings.reasoning_effort ?? "");
      setErrorText(null);
    }
  };

  const handleTierChange = (nextTier: InferenceTier) => {
    setTier(nextTier);
    const preset = TIER_DEFAULTS[nextTier];
    const isPresetModel = Object.values(TIER_DEFAULTS).some((item) => item.model === model.trim());
    if (!model.trim() || isPresetModel) {
      setModel(preset.model);
    }
  };

  const handleSave = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!llmSettings) return;
    const trimmedModel = model.trim();
    if (!trimmedModel) return;
    setErrorText(null);

    const currentIsPresetUrl = Object.values(TIER_DEFAULTS).some(
      (item) => item.baseUrl === llmSettings.base_url.trim(),
    );
    const nextBaseUrl =
      tier !== llmSettings.tier && currentIsPresetUrl
        ? TIER_DEFAULTS[tier].baseUrl
        : llmSettings.base_url;

    saveLlm.mutate(
      {
        tier,
        base_url: nextBaseUrl,
        model: trimmedModel,
        api_key_env: llmSettings.api_key_env ?? null,
        api_key: null,
        reasoning_effort: effort.trim() || null,
      },
      {
        onSuccess: () => setOpen(false),
        onError: (err) =>
          setErrorText(formatErrorMessage(err, t, t("modals.workspace.errors.saveLlm"))),
      },
    );
  };

  return (
    <Popover open={open} onOpenChange={handleOpenChange}>
      <PopoverTrigger
        render={
          <InputGroupButton
            type="button"
            variant="ghost"
            size="xs"
            title={t("copilot.switchModel")}
            className="max-w-[230px] gap-1 text-muted-foreground hover:text-foreground"
          />
        }
      >
        <Cpu className="size-3 shrink-0" aria-hidden="true" />
        <span lang="en" className="font-mono-tech truncate text-[11px]">
          {llmSettings?.model || "—"}
        </span>
        <ChevronsUpDown className="size-3 shrink-0 opacity-60" aria-hidden="true" />
      </PopoverTrigger>

      <PopoverContent align="start" side="top" sideOffset={8} className="w-72 p-3">
        <form onSubmit={handleSave} className="flex flex-col gap-3">
          <div className="flex flex-col gap-1.5">
            <span className="text-[11px] font-medium text-muted-foreground">
              {t("modals.workspace.tierLabel")}
            </span>
            <div className="grid grid-cols-3 gap-1 rounded-md bg-muted p-0.5">
              {TIER_ORDER.map((option) => {
                const active = tier === option;
                return (
                  <button
                    key={option}
                    type="button"
                    aria-pressed={active}
                    onClick={() => handleTierChange(option)}
                    className={cn(
                      "cursor-pointer truncate rounded-sm px-1.5 py-1 text-[11px] font-medium transition-all",
                      active
                        ? "bg-background text-foreground shadow-2xs"
                        : "text-muted-foreground hover:text-foreground",
                    )}
                  >
                    {t(`modals.workspace.tier.${option}`)}
                  </button>
                );
              })}
            </div>
          </div>

          <div className="flex flex-col gap-1">
            <label
              htmlFor="quick-llm-model"
              className="text-[11px] font-medium text-muted-foreground"
            >
              {t("modals.workspace.modelLabel")}
            </label>
            <Input
              id="quick-llm-model"
              value={model}
              onChange={(event) => setModel(event.target.value)}
              placeholder={t("modals.workspace.modelPlaceholder")}
              autoCorrect="off"
              autoCapitalize="none"
              spellCheck={false}
              className="font-mono-tech h-7 text-xs"
            />
          </div>

          <div className="flex flex-col gap-1">
            <span className="text-[11px] font-medium text-muted-foreground">
              {t("modals.workspace.reasoningEffortLabel")}
            </span>
            <div className="flex flex-wrap gap-1">
              {QUICK_EFFORTS.map((option) => {
                const active = effort === option.value;
                return (
                  <button
                    key={option.value || "default"}
                    type="button"
                    aria-pressed={active}
                    onClick={() => setEffort(option.value)}
                    className={cn(
                      "cursor-pointer rounded-sm border px-2 py-0.5 text-[11px] font-medium transition-colors",
                      active
                        ? "border-primary bg-primary/10 text-foreground"
                        : "border-border text-muted-foreground hover:text-foreground",
                    )}
                  >
                    {t(option.labelKey)}
                  </button>
                );
              })}
            </div>
          </div>

          {errorText && <p className="text-[11px] text-destructive">{errorText}</p>}

          <div className="flex items-center justify-between gap-2 border-t border-border pt-2">
            <button
              type="button"
              onClick={() => {
                setOpen(false);
                onOpenLlmSettings();
              }}
              className="inline-flex cursor-pointer items-center gap-1 text-[11px] text-muted-foreground transition-colors hover:text-foreground"
            >
              <SlidersHorizontal className="size-3 shrink-0" aria-hidden="true" />
              <span>{t("copilot.providerSettingsLink")}</span>
            </button>

            <Button type="submit" size="xs" disabled={!model.trim() || saveLlm.isPending}>
              <span className="relay-text-trim leading-none">{t("copilot.applyModel")}</span>
            </Button>
          </div>
        </form>
      </PopoverContent>
    </Popover>
  );
}

export function AgentCopilotPanel({
  workspace,
  status,
  headCommitSha,
  staleCount,
  attentionCount,
  messages,
  isStreaming,
  streamingMessageId,
  sendError,
  onDismissSendError,
  llmSettings,
  onSendMessage,
  onCancelTurn,
  onResolveApproval,
  isResolvingApproval,
  onOpenLlmSettings,
}: AgentCopilotPanelProps) {
  const { t } = useTranslation();
  const inputId = useId();
  const [input, setInput] = useState("");

  const hasRepo = Boolean(workspace?.repo_root);
  const repoLabel = workspace?.name || workspace?.repo_id || null;
  const transport = status?.transport_path ?? "offline";
  const isConnected = status?.connected ?? false;

  const quickActions: Array<{ id: string; label: string; prompt: string }> = [];
  if (hasRepo) {
    if (staleCount > 0) {
      quickActions.push({
        id: "stale",
        label: t("copilot.quick.checkStale"),
        prompt: t("copilot.quick.checkStale"),
      });
    }
    if (attentionCount > 0) {
      quickActions.push({
        id: "attention",
        label: t("copilot.quick.checkAttention"),
        prompt: t("copilot.quick.checkAttention"),
      });
    }
    quickActions.push({
      id: "status",
      label: t("copilot.quick.inspectEnv"),
      prompt: t("copilot.quick.inspectEnv"),
    });
    if (quickActions.length < 3 && (workspace?.presets.length ?? 0) > 0) {
      quickActions.push({
        id: "preset",
        label: t("copilot.quick.suggestPreset"),
        prompt: t("copilot.quick.suggestPreset"),
      });
    }
  }
  const visibleQuickActions = quickActions.slice(0, 3);

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const trimmed = input.trim();
    if (!trimmed || isStreaming) return;
    const submitted = input;
    setInput("");
    const ok = await onSendMessage(trimmed);
    if (!ok) {
      setInput((current) => (current.trim().length === 0 ? submitted : current));
    }
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      // Guard against premature submission during CJK IME composition (including WebKit bug 165004).
      if (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) {
        return;
      }
      if (input.trim() && !isStreaming) {
        event.currentTarget.form?.requestSubmit();
      }
    }
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {messages.length === 0 ? (
        <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-4">
          <div className="flex flex-col gap-2 rounded-lg border border-border bg-muted/30 p-3">
            <div className="flex items-center justify-between gap-2">
              <div className="flex min-w-0 items-center gap-2">
                <FolderGit2
                  className={cn(
                    "size-3.5 shrink-0",
                    hasRepo ? "text-primary" : "text-muted-foreground",
                  )}
                  aria-hidden="true"
                />
                <span
                  lang={repoLabel ? "en" : undefined}
                  className="truncate text-xs font-medium text-foreground"
                >
                  {repoLabel ?? t("copilot.contextNoRepo")}
                </span>
              </div>
              {headCommitSha && (
                <code
                  lang="en"
                  className="font-mono-tech shrink-0 rounded-sm bg-background px-1.5 py-0.5 text-[10.5px] text-muted-foreground"
                >
                  {headCommitSha.slice(0, 7)}
                </code>
              )}
            </div>

            <div className="flex flex-wrap items-center gap-1.5">
              <Badge
                variant="secondary"
                className={cn(
                  "h-5 gap-1 px-1.5 text-[10.5px]",
                  isConnected &&
                    "bg-emerald-500/12 text-emerald-700 dark:bg-emerald-500/20 dark:text-emerald-300",
                )}
              >
                <span
                  aria-hidden="true"
                  className={cn(
                    "size-1.5 rounded-full",
                    isConnected
                      ? "bg-emerald-500"
                      : status?.mode === "local_dual_sandbox"
                        ? "bg-sky-500"
                        : "bg-amber-500",
                  )}
                />
                <span>{t(`topbar.transport.${transport}`)}</span>
              </Badge>

              {staleCount > 0 && (
                <Badge variant="destructive" className="h-5 px-1.5 text-[10.5px]">
                  {t("topbar.outdatedCount", { count: staleCount })}
                </Badge>
              )}

              {attentionCount > 0 && (
                <Badge
                  variant="secondary"
                  className="h-5 bg-primary/12 px-1.5 text-[10.5px] text-primary"
                >
                  {t("copilot.attentionCount", { count: attentionCount })}
                </Badge>
              )}
            </div>
          </div>

          {visibleQuickActions.length > 0 && (
            <div className="flex flex-col gap-1.5">
              <span className="px-0.5 text-[11px] font-medium text-muted-foreground">
                {t("copilot.quickTitle")}
              </span>
              <div className="flex flex-col gap-1">
                {visibleQuickActions.map((item) => (
                  <button
                    key={item.id}
                    type="button"
                    disabled={isStreaming}
                    onClick={() => void onSendMessage(item.prompt)}
                    className="group flex w-full cursor-pointer items-center justify-between gap-2 rounded-md border border-border/80 bg-background px-3 py-2 text-left text-xs text-foreground transition-colors hover:border-primary/40 hover:bg-muted/40 disabled:pointer-events-none disabled:opacity-50"
                  >
                    <span className="truncate">{item.label}</span>
                    <ArrowRight
                      className="size-3.5 shrink-0 text-muted-foreground transition-transform group-hover:translate-x-0.5 group-hover:text-primary"
                      aria-hidden="true"
                    />
                  </button>
                ))}
              </div>
            </div>
          )}
        </div>
      ) : (
        <MessageScrollerProvider autoScroll>
          <MessageScroller className="min-h-0 flex-1">
            <MessageScrollerViewport className="px-3.5 py-4">
              <MessageScrollerContent className="gap-4">
                {messages.map((message, index) => {
                  const retryPrompt =
                    message.role === "assistant" && message.error != null
                      ? findPrecedingUserPrompt(messages, index)
                      : null;
                  return (
                    <MessageScrollerItem
                      key={message.id}
                      messageId={message.id}
                      scrollAnchor={message.role === "user"}
                    >
                      <CopilotMessageItem
                        message={message}
                        isStreaming={isStreaming && streamingMessageId === message.id}
                        onResolveApproval={onResolveApproval}
                        isResolvingApproval={isResolvingApproval}
                        onOpenLlmSettings={onOpenLlmSettings}
                        onRetry={
                          retryPrompt && !isStreaming
                            ? () => void onSendMessage(retryPrompt)
                            : undefined
                        }
                      />
                    </MessageScrollerItem>
                  );
                })}
              </MessageScrollerContent>
            </MessageScrollerViewport>
            <MessageScrollerButton />
          </MessageScroller>
        </MessageScrollerProvider>
      )}

      {sendError && (
        <div className="px-3 pt-2">
          <Alert
            role="alert"
            variant="destructive"
            className="border-destructive/30 bg-[var(--relay-stale-soft)] py-2"
          >
            <AlertCircle />
            <AlertDescription className="flex flex-col gap-2 text-xs">
              <span className="min-w-0 break-words">{sendError}</span>
              {isLlmConfigurationError(sendError) && (
                <div>
                  <Button
                    type="button"
                    variant="outline"
                    size="xs"
                    onClick={onOpenLlmSettings}
                    className="shrink-0"
                  >
                    <SlidersHorizontal data-icon="inline-start" />
                    <span className="relay-text-trim leading-none">
                      {t("copilot.configureModel")}
                    </span>
                  </Button>
                </div>
              )}
            </AlertDescription>
            {onDismissSendError && (
              <AlertAction>
                <Button
                  type="button"
                  size="icon-xs"
                  variant="ghost"
                  aria-label={t("detail.close")}
                  onClick={onDismissSendError}
                >
                  <X />
                </Button>
              </AlertAction>
            )}
          </Alert>
        </div>
      )}

      <form
        onSubmit={(event) => void handleSubmit(event)}
        className="shrink-0 border-t border-border bg-card p-3"
      >
        <label htmlFor={inputId} className="sr-only">
          {t("copilot.inputLabel")}
        </label>
        <InputGroup className="flex-col items-stretch rounded-lg bg-background">
          <InputGroupTextarea
            id={inputId}
            value={input}
            onChange={(event) => setInput(event.target.value)}
            onKeyDown={handleKeyDown}
            placeholder={t("copilot.placeholder")}
            rows={2}
            className="max-h-32 min-h-[52px] px-3 py-2 text-[13px] leading-relaxed"
          />
          <InputGroupAddon
            align="block-end"
            className="justify-between border-t border-border/60 px-2 py-1.5"
          >
            <QuickModelPopover llmSettings={llmSettings} onOpenLlmSettings={onOpenLlmSettings} />

            {isStreaming ? (
              <InputGroupButton
                type="button"
                variant="destructive"
                size="icon-sm"
                aria-label={t("copilot.stop")}
                title={t("copilot.stop")}
                onClick={() => void onCancelTurn()}
              >
                <Square className="size-3 fill-current" />
              </InputGroupButton>
            ) : (
              <InputGroupButton
                type="submit"
                variant="default"
                size="icon-sm"
                disabled={!input.trim()}
                aria-label={t("copilot.send")}
                title={t("copilot.send")}
              >
                <ArrowUp className="size-3.5" />
              </InputGroupButton>
            )}
          </InputGroupAddon>
        </InputGroup>
      </form>
    </div>
  );
}
