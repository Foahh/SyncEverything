import { Alert, Button } from "@heroui/react";
import {
  Background,
  BackgroundVariant,
  Controls,
  MiniMap,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  type NodeTypes,
} from "@xyflow/react";
import { Eye, EyeOff, Maximize2, Play, Plus, ShieldAlert, Zap } from "lucide-react";
import { useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import type { DagSnapshot_Serialize } from "../bindings";
import { buildRelayFlowGraph } from "../lib/dagLayout";
import { ArtifactNode } from "./dag/ArtifactNode";
import { DecisionNode } from "./dag/DecisionNode";
import { EvidenceNode } from "./dag/EvidenceNode";

const nodeTypes: NodeTypes = {
  decision: DecisionNode,
  artifact: ArtifactNode,
  evidence: EvidenceNode,
};

interface DagCanvasProps {
  snapshot: DagSnapshot_Serialize | undefined;
  selectedNodeId: string | null;
  includeStale: boolean;
  onToggleIncludeStale: () => void;
  onSelectNode: (nodeId: string | null) => void;
  onOpenRecordDecision: () => void;
  onOpenDispatchTask: () => void;
  onTriggerUpstreamEdit: () => void;
  isSimulatingEdit: boolean;
}

function DagCanvasInner({
  snapshot,
  selectedNodeId,
  includeStale,
  onToggleIncludeStale,
  onSelectNode,
  onOpenRecordDecision,
  onOpenDispatchTask,
  onTriggerUpstreamEdit,
  isSimulatingEdit,
}: DagCanvasProps) {
  const { t } = useTranslation();
  const { fitView } = useReactFlow();
  const containerRef = useRef<HTMLDivElement | null>(null);

  const { nodes, edges } = useMemo(() => {
    if (!snapshot) return { nodes: [], edges: [] };
    return buildRelayFlowGraph(snapshot, selectedNodeId);
  }, [snapshot, selectedNodeId]);

  useEffect(() => {
    if (nodes.length > 0) {
      const id = window.setTimeout(() => {
        void fitView({ padding: 0.22, duration: 350 });
      }, 60);
      return () => window.clearTimeout(id);
    }
  }, [nodes.length, fitView]);

  useEffect(() => {
    const el = containerRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    let timer: number | undefined;
    const observer = new ResizeObserver(() => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        void fitView({ padding: 0.22, duration: 220 });
      }, 120);
    });
    observer.observe(el);
    return () => {
      window.clearTimeout(timer);
      observer.disconnect();
    };
  }, [fitView]);

  const staleCount = snapshot?.stale_count ?? 0;

  return (
    <div
      ref={containerRef}
      className="relative flex-1 h-full w-full overflow-hidden bg-[var(--relay-canvas)]"
    >
      {/* Top Overlay Stack: Title Badge + Non-Overlapping STALE Guard Banner */}
      <div className="pointer-events-none absolute top-3.5 left-4 right-4 z-20 flex flex-col gap-2">
        <div className="flex items-center justify-between gap-2">
          <div className="rounded-lg border border-[var(--relay-border)] bg-[var(--relay-surface)]/92 px-2.5 py-1 text-[11px] font-medium text-[var(--relay-text-muted)] shadow-xs backdrop-blur-sm">
            {t("dag.title")}
          </div>
        </div>

        {staleCount > 0 && (
          <div className="pointer-events-auto max-w-2xl rounded-xl shadow-lg">
            <Alert
              status="danger"
              className="border border-red-500/35 bg-[var(--relay-surface)]/95 py-2.5 backdrop-blur-md"
            >
              <Alert.Indicator>
                <ShieldAlert className="h-4 w-4 text-red-600 dark:text-red-400" />
              </Alert.Indicator>
              <Alert.Content>
                <Alert.Description className="text-[12px] font-medium text-red-700 dark:text-red-300">
                  {t("dag.staleGuardBanner", { count: staleCount })}
                </Alert.Description>
              </Alert.Content>
            </Alert>
          </div>
        )}
      </div>

      {/* Main Interactive React Flow Canvas */}
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        onNodeClick={(_, node) => onSelectNode(node.id)}
        onPaneClick={() => onSelectNode(null)}
        fitView
        fitViewOptions={{ padding: 0.24 }}
        minZoom={0.45}
        maxZoom={1.6}
        proOptions={{ hideAttribution: true }}
      >
        <Background
          variant={BackgroundVariant.Dots}
          gap={22}
          size={1.25}
          color="var(--relay-grid)"
        />
        <Controls position="bottom-left" showInteractive={false} />
        <MiniMap
          position="bottom-right"
          style={{ width: 144, height: 92 }}
          pannable
          zoomable
          nodeColor={(n) => {
            const graphNode = (n.data as { node?: { freshness?: { status?: string } } })?.node;
            if (graphNode?.freshness?.status === "stale") return "#dc2626";
            if (n.type === "decision") return "#6366f1";
            if (n.type === "artifact") return "#0284c7";
            return "#0d9488";
          }}
        />
      </ReactFlow>

      {/* Empty State */}
      {nodes.length === 0 && (
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center p-8">
          <div className="pointer-events-auto max-w-sm rounded-2xl border border-[var(--relay-border)] bg-[var(--relay-surface)] p-6 text-center shadow-sm">
            <h3 className="text-sm font-semibold text-[var(--relay-text)]">
              {t("dag.emptyTitle")}
            </h3>
            <p className="mt-1.5 text-xs leading-relaxed text-[var(--relay-text-muted)]">
              {t("dag.emptyDesc")}
            </p>
            <div className="mt-4 flex justify-center gap-2">
              <Button size="sm" variant="primary" onPress={onOpenRecordDecision}>
                <Plus className="h-3.5 w-3.5" />
                {t("dag.dock.recordDecision")}
              </Button>
            </div>
          </div>
        </div>
      )}

      {/* Floating Bottom Action Dock (Responsive, zero overlap with Controls / MiniMap) */}
      <div className="pointer-events-none absolute bottom-3.5 left-3 right-3 z-20 flex justify-center">
        <div className="pointer-events-auto flex max-w-full flex-wrap items-center justify-center gap-1 rounded-2xl border border-[var(--relay-border)] bg-[var(--relay-surface)]/95 p-1.5 shadow-xl backdrop-blur-md">
          <Button
            size="sm"
            variant="secondary"
            className="h-8 whitespace-nowrap px-2.5 text-[11.5px]"
            onPress={onOpenRecordDecision}
          >
            <Plus className="h-3.5 w-3.5" />
            {t("dag.dock.recordDecision")}
          </Button>
          <Button
            size="sm"
            variant="primary"
            className="h-8 whitespace-nowrap px-2.5 text-[11.5px] font-semibold"
            onPress={onOpenDispatchTask}
          >
            <Play className="h-3.5 w-3.5" />
            {t("dag.dock.dispatchTask")}
          </Button>
          <div className="mx-0.5 hidden h-5 w-px bg-[var(--relay-border)] sm:block" />
          <Button
            size="sm"
            variant="danger-soft"
            className="h-8 whitespace-nowrap px-2.5 text-[11.5px]"
            isDisabled={isSimulatingEdit}
            onPress={onTriggerUpstreamEdit}
          >
            <Zap className="h-3.5 w-3.5" />
            {t("dag.dock.simulateEdit")}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="h-8 whitespace-nowrap px-2 text-[11.5px]"
            onPress={onToggleIncludeStale}
          >
            {includeStale ? <Eye className="h-3.5 w-3.5" /> : <EyeOff className="h-3.5 w-3.5" />}
            {includeStale ? t("dag.dock.showStale") : t("dag.dock.hideStale")}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            isIconOnly
            className="h-8 w-8 min-w-8"
            aria-label={t("dag.dock.fitView")}
            onPress={() => void fitView({ padding: 0.22, duration: 300 })}
          >
            <Maximize2 className="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>
    </div>
  );
}

export function DagCanvas(props: DagCanvasProps) {
  return (
    <ReactFlowProvider>
      <DagCanvasInner {...props} />
    </ReactFlowProvider>
  );
}
