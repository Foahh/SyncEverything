import { Alert, AlertAction, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
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
import { AlertCircle, ArrowUp, Cpu, SlidersHorizontal, Sparkles, Square, X } from "lucide-react";
import { useId, useState, type FormEvent, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import type { AgentChatMessage_Serialize, LlmSettingsView_Serialize } from "../bindings";
import { isLlmConfigurationError } from "../lib/formatError";
import type { ApprovalInput } from "../lib/relayApi";
import { CopilotMessageItem } from "./CopilotMessageItem";

interface AgentCopilotPanelProps {
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

export function AgentCopilotPanel({
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

  const suggestions = [
    t("copilot.suggestions.checkOutdated"),
    t("copilot.suggestions.inspectEnv"),
    t("copilot.suggestions.runAndCheck"),
  ];

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

  const handlePickSuggestion = (text: string) => {
    if (isStreaming) return;
    void onSendMessage(text);
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {messages.length === 0 ? (
        <div className="flex min-h-0 flex-1 flex-col justify-center gap-5 overflow-y-auto px-5 py-6">
          <div className="flex flex-col items-start gap-2">
            <div className="flex size-9 items-center justify-center rounded-lg bg-primary/10 text-primary">
              <Sparkles className="size-4" />
            </div>
            <h3 className="text-sm font-semibold text-foreground">{t("copilot.emptyTitle")}</h3>
            <p className="text-xs leading-relaxed text-muted-foreground">
              {t("copilot.emptyDesc")}
            </p>
          </div>

          <div className="flex flex-col gap-2">
            {suggestions.map((suggestion) => (
              <Button
                key={suggestion}
                type="button"
                variant="outline"
                size="sm"
                className="h-auto justify-start px-3 py-2 text-left text-xs leading-snug font-normal whitespace-normal text-foreground"
                onClick={() => handlePickSuggestion(suggestion)}
              >
                {suggestion}
              </Button>
            ))}
          </div>
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
            <InputGroupButton
              type="button"
              variant="ghost"
              size="xs"
              onClick={onOpenLlmSettings}
              title={t("copilot.configureModel")}
              className="max-w-[210px] text-muted-foreground hover:text-foreground"
            >
              <Cpu className="size-3 shrink-0" />
              <span lang="en" className="font-mono-tech truncate text-[11px]">
                {llmSettings?.model || "—"}
              </span>
            </InputGroupButton>

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
