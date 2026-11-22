import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import type {
  ApprovalDecision,
  GraphEdge,
  GraphNode,
  RuntimeMode,
  TaskSpec_Deserialize,
} from "../bindings";
import { relayApi, resetBrowserPreviewState } from "./relayApi";

export const relayKeys = {
  all: ["relay"] as const,
  status: () => [...relayKeys.all, "status"] as const,
  dag: (includeStale: boolean) => [...relayKeys.all, "dag", includeStale] as const,
  tasks: () => [...relayKeys.all, "tasks"] as const,
  handoffs: () => [...relayKeys.all, "handoffs"] as const,
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
