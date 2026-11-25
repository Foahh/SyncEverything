import { useQueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type {
  AgentApprovalCard_Serialize,
  AgentChatMessage_Serialize,
  AgentChatStreamEvent_Serialize,
  AgentToolTrace_Serialize,
} from "../bindings";
import { formatErrorMessage } from "../lib/formatError";
import { relayKeys, useChatMessages, useClearChatMessages } from "../lib/queries";
import { relayApi } from "../lib/relayApi";

interface StreamingDraft {
  messageId: string;
  content: string;
  traces: AgentToolTrace_Serialize[];
  approval: AgentApprovalCard_Serialize | null;
  error: string | null;
  createdAt: string;
  active: boolean;
}

function applyStreamEvent(
  prev: StreamingDraft | null,
  event: AgentChatStreamEvent_Serialize,
  t: TFunction,
): StreamingDraft {
  const base: StreamingDraft =
    prev && prev.messageId === event.message_id
      ? prev
      : {
          messageId: event.message_id,
          content: "",
          traces: [],
          approval: null,
          error: null,
          createdAt: new Date().toISOString(),
          active: true,
        };

  switch (event.kind) {
    case "turn_started":
      return { ...base, active: true };
    case "token_delta":
      return {
        ...base,
        active: true,
        content: base.content + (event.delta ?? ""),
      };
    case "tool_started": {
      if (!event.trace) return base;
      const exists = base.traces.some((tr) => tr.id === event.trace!.id);
      return {
        ...base,
        active: true,
        traces: exists ? base.traces : [...base.traces, event.trace],
      };
    }
    case "tool_finished": {
      if (!event.trace) return base;
      const found = base.traces.some((tr) => tr.id === event.trace!.id);
      return {
        ...base,
        active: true,
        traces: found
          ? base.traces.map((tr) => (tr.id === event.trace!.id ? event.trace! : tr))
          : [...base.traces, event.trace],
      };
    }
    case "approval_needed":
      return {
        ...base,
        approval: event.approval ?? base.approval,
      };
    case "turn_completed":
      return {
        ...base,
        active: false,
        approval: event.approval ?? base.approval,
      };
    case "turn_error":
      return {
        ...base,
        active: false,
        error: event.error
          ? formatErrorMessage(event.error, t, t("copilot.turnFailed"))
          : t("copilot.turnFailed"),
      };
    default:
      return base;
  }
}

export function useCopilotChat() {
  const { t } = useTranslation();
  const tRef = useRef(t);

  useEffect(() => {
    tRef.current = t;
  }, [t]);

  const queryClient = useQueryClient();
  const { data: persisted = [] } = useChatMessages();
  const clearMutation = useClearChatMessages();
  const clearing = useRef(false);
  const generation = useRef(0);
  const lastFinishedMessage = useRef<string | null>(null);
  const [draft, setDraft] = useState<StreamingDraft | null>(null);
  const [sendError, setSendError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    let unsub: (() => void) | undefined;

    void relayApi
      .subscribeAgentChatEvents((event) => {
        if (!active || clearing.current) return;
        setDraft((prev) => applyStreamEvent(prev, event, tRef.current));
        if (event.kind === "turn_completed" || event.kind === "turn_error") {
          lastFinishedMessage.current = event.message_id;
          void queryClient.invalidateQueries({ queryKey: relayKeys.all }).then(() => {
            if (active) {
              setDraft((current) =>
                current && current.messageId === event.message_id && !current.active
                  ? null
                  : current,
              );
            }
          });
        }
      })
      .then((fn) => {
        if (active) {
          unsub = fn;
        } else {
          fn();
        }
      });

    return () => {
      active = false;
      unsub?.();
    };
  }, [queryClient]);

  const messages: AgentChatMessage_Serialize[] = useMemo(() => {
    if (!draft) return persisted;
    const idx = persisted.findIndex((m) => m.id === draft.messageId);
    if (idx === -1) {
      return [
        ...persisted,
        {
          id: draft.messageId,
          repo_id: "",
          role: "assistant",
          content: draft.content,
          traces: draft.traces,
          approval: draft.approval,
          error: draft.error,
          created_at: draft.createdAt,
        },
      ];
    }
    return persisted.map((m) => {
      if (m.id !== draft.messageId) return m;
      return {
        ...m,
        content: draft.content.length >= m.content.length ? draft.content : m.content,
        traces: draft.traces.length >= m.traces.length ? draft.traces : m.traces,
        approval: m.approval?.resolved ? m.approval : (draft.approval ?? m.approval),
        error: draft.error ?? m.error,
      };
    });
  }, [persisted, draft]);

  const sendMessage = useCallback(
    async (prompt: string): Promise<boolean> => {
      const trimmed = prompt.trim();
      if (!trimmed || clearing.current) return false;
      const turn = ++generation.current;
      setSendError(null);
      try {
        const placeholder = await relayApi.sendChatMessage(trimmed);
        if (turn !== generation.current) return false;
        setDraft((prev) =>
          lastFinishedMessage.current === placeholder.id || prev?.messageId === placeholder.id
            ? prev
            : {
                messageId: placeholder.id,
                content: "",
                traces: [],
                approval: null,
                error: null,
                createdAt: placeholder.created_at,
                active: true,
              },
        );
        await queryClient.invalidateQueries({ queryKey: relayKeys.chat() });
        return true;
      } catch (err) {
        if (turn === generation.current) {
          setSendError(formatErrorMessage(err, tRef.current, tRef.current("copilot.turnFailed")));
        }
        return false;
      }
    },
    [queryClient],
  );

  const clearSendError = useCallback(() => {
    setSendError(null);
  }, []);

  const cancelTurn = useCallback(async () => {
    await relayApi.cancelChatTurn();
    setDraft((prev) => (prev ? { ...prev, active: false } : null));
    await queryClient.invalidateQueries({ queryKey: relayKeys.chat() });
  }, [queryClient]);

  const clearChat = useCallback(async () => {
    clearing.current = true;
    generation.current++;
    setSendError(null);
    try {
      await clearMutation.mutateAsync();
      setDraft(null);
    } catch (error) {
      setSendError(formatErrorMessage(error, tRef.current));
    } finally {
      clearing.current = false;
    }
  }, [clearMutation]);

  return {
    messages,
    isStreaming: draft?.active ?? false,
    streamingMessageId: draft?.active ? draft.messageId : null,
    sendError,
    clearSendError,
    sendMessage,
    cancelTurn,
    clearChat,
    isClearing: clearMutation.isPending,
  };
}
