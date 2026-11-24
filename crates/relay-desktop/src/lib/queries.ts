import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import type {
  ApprovalDecision,
  GraphEdge,
  GraphNode,
  LlmSettingsUpdate_Deserialize,
  RuntimeMode,
  TaskSpec_Deserialize,
} from "../bindings";
import { relayApi, resetBrowserPreviewState } from "./relayApi";

export const relayKeys = {
  all: ["relay"] as const,
  status: () => [...relayKeys.all, "status"] as const,
  workspace: () => [...relayKeys.all, "workspace"] as const,
  llm: () => [...relayKeys.all, "llm"] as const,
  dag: (includeStale: boolean) => [...relayKeys.all, "dag", includeStale] as const,
  tasks: () => [...relayKeys.all, "tasks"] as const,
  handoffs: () => [...relayKeys.all, "handoffs"] as const,
  chat: () => [...relayKeys.all, "chat"] as const,
};

export function useRelayEventSync() {
  const queryClient = useQueryClient();

  useEffect(() => {
    let active = true;
    let unsub: (() => void) | undefined;

    void relayApi
      .subscribeEvents({
        onStaleAlert: () => {
          void queryClient.invalidateQueries({ queryKey: relayKeys.all });
        },
        onStateChanged: () => {
          void queryClient.invalidateQueries({ queryKey: relayKeys.all });
        },
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
}

export function useRuntimeStatus() {
  return useQuery({
    queryKey: relayKeys.status(),
    queryFn: () => relayApi.getRuntimeStatus(),
    refetchInterval: 4000,
  });
}

export function useWorkspaceInfo() {
  return useQuery({
    queryKey: relayKeys.workspace(),
    queryFn: () => relayApi.getWorkspaceInfo(),
  });
}

export function useLlmSettings() {
  return useQuery({
    queryKey: relayKeys.llm(),
    queryFn: () => relayApi.getLlmSettings(),
  });
}

export function useOpenWorkspaceRepo() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (repoPath: string) => relayApi.openWorkspaceRepo(repoPath),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: relayKeys.all });
    },
  });
}

export function useInitWorkspaceManifest() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (repoPath?: string | null) => relayApi.initWorkspaceManifest(repoPath),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: relayKeys.all });
    },
  });
}

export function useRefreshGitHead() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => relayApi.refreshGitHead(),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: relayKeys.all });
    },
  });
}

export function useSaveLlmSettings() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (update: LlmSettingsUpdate_Deserialize) => relayApi.saveLlmSettings(update),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: relayKeys.llm() });
    },
  });
}

export function useDagSnapshot(includeStale: boolean) {
  return useQuery({
    queryKey: relayKeys.dag(includeStale),
    queryFn: () => relayApi.getDagSnapshot(includeStale),
  });
}

export function useTasks() {
  return useQuery({
    queryKey: relayKeys.tasks(),
    queryFn: () => relayApi.listTasks(),
  });
}

export function useHandoffs() {
  return useQuery({
    queryKey: relayKeys.handoffs(),
    queryFn: () => relayApi.listHandoffs(),
  });
}

export function useChatMessages() {
  return useQuery({
    queryKey: relayKeys.chat(),
    queryFn: () => relayApi.listChatMessages(100),
  });
}

export function useClearChatMessages() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => relayApi.clearChatMessages(),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: relayKeys.chat() });
    },
  });
}

export function useSwitchRuntimeMode() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (mode: RuntimeMode) => relayApi.switchRuntimeMode(mode),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: relayKeys.all });
    },
  });
}

export function useSubmitTask() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (spec: TaskSpec_Deserialize) => relayApi.submitTask(spec),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: relayKeys.tasks() });
      void queryClient.invalidateQueries({ queryKey: relayKeys.handoffs() });
    },
  });
}

export function useResolvePatchApproval() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ taskId, decision }: { taskId: string; decision: ApprovalDecision }) =>
      relayApi.resolvePatchApproval(taskId, decision),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: relayKeys.all });
    },
  });
}

export function useSimulateArtifactUpdate() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      oldArtifactId,
      newCommitSha,
      reason,
    }: {
      oldArtifactId: string;
      newCommitSha: string;
      reason: string;
    }) => relayApi.simulateArtifactUpdate(oldArtifactId, newCommitSha, reason),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: relayKeys.all });
    },
  });
}

export function useUpsertDagNode() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ node, edges }: { node: GraphNode; edges: GraphEdge[] }) =>
      relayApi.upsertDagNode(node, edges),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: relayKeys.all });
    },
  });
}

export function useGetPairingTicket() {
  return useMutation({
    mutationFn: () => relayApi.getPairingTicket(),
  });
}

export function useConnectPeerTicket() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (ticketJson: string) => relayApi.connectPeerTicket(ticketJson),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: relayKeys.all });
    },
  });
}

export function useResetPreviewDemo() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      resetBrowserPreviewState();
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: relayKeys.all });
    },
  });
}
