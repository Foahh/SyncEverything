import { Button } from "@heroui/react";
import {
  Background,
  BackgroundVariant,
  Controls,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  type NodeTypes,
} from "@xyflow/react";
import { AlertCircle, Eye, EyeOff, Maximize2, Play, Plus } from "lucide-react";
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
}

function DagCanvasInner({
  snapshot,
  selectedNodeId,
  includeStale,
  onToggleIncludeStale,
  onSelectNode,
  onOpenRecordDecision,
  onOpenDispatchTask,
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
        void fitView({ padding: 0.28, maxZoom: 1.05, duration: 300 });
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
        void fitView({ padding: 0.28, maxZoom: 1.05, duration: 220 });
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
      {/* Subtle single-line notice ONLY when outdated results exist */}
      {staleCount > 0 && (
        <div className="pointer-events-none absolute top-4 left-4 right-4 z-20 flex justify-center">
          <div className="pointer-events-auto inline-flex items-center gap-2 rounded-full border border-red-500/30 bg-[var(--relay-surface)]/95 px-3.5 py-1.5 text-[12px] font-medium text-red-700 shadow-sm backdrop-blur-md dark:text-red-300">
            <AlertCircle className="h-3.5 w-3.5 shrink-0 text-red-500" />
            <span>{t("canvas.outdatedBanner", { count: staleCount })}</span>
          </div>
        </div>
      )}

      {/* Calm Interactive Workflow Map */}
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        onNodeClick={(_, node) => onSelectNode(node.id)}
        onPaneClick={() => onSelectNode(null)}
        fitView
        fitViewOptions={{ padding: 0.28, maxZoom: 1.05 }}
        minZoom={0.5}
        maxZoom={1.5}
        proOptions={{ hideAttribution: true }}
      >
        <Background
          variant={BackgroundVariant.Dots}
          gap={24}
          size={1.15}
          color="var(--relay-grid)"
        />
        <Controls position="bottom-left" showInteractive={false} />
      </ReactFlow>

      {/* Clean First-Run Empty State */}
      {nodes.length === 0 && (
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center p-8">
          <div className="pointer-events-auto max-w-sm rounded-2xl border border-[var(--relay-border)] bg-[var(--relay-surface)] p-6 text-center shadow-sm">
            <h3 className="text-[15px] font-semibold text-[var(--relay-text)]">
              {t("canvas.emptyTitle")}
            </h3>
            <p className="mt-1.5 text-[12.5px] leading-relaxed text-[var(--relay-text-muted)]">
              {t("canvas.emptyDesc")}
            </p>
            <div className="mt-4 flex justify-center gap-2">
              <Button size="sm" variant="primary" onPress={onOpenDispatchTask}>
                <Play className="h-3.5 w-3.5" />
                {t("canvas.emptyCta")}
              </Button>
              <Button size="sm" variant="secondary" onPress={onOpenRecordDecision}>
                <Plus className="h-3.5 w-3.5" />
                {t("canvas.dock.addNote")}
              </Button>
            </div>
          </div>
        </div>
      )}

      {/* Minimal Floating Bottom Toolbar */}
      <div className="pointer-events-none absolute bottom-4 left-4 right-4 z-20 flex justify-center">
        <div className="pointer-events-auto flex items-center gap-1 rounded-2xl border border-[var(--relay-border)] bg-[var(--relay-surface)]/95 p-1.5 shadow-lg backdrop-blur-md">
          <Button
            size="sm"
            variant="secondary"
            className="h-8 whitespace-nowrap px-3 text-[12px]"
            onPress={onOpenRecordDecision}
          >
            <Plus className="h-3.5 w-3.5" />
            {t("canvas.dock.addNote")}
          </Button>
          <Button
            size="sm"
            variant="primary"
            className="h-8 whitespace-nowrap px-3 text-[12px] font-medium"
            onPress={onOpenDispatchTask}
          >
            <Play className="h-3.5 w-3.5" />
            {t("canvas.dock.runTask")}
          </Button>

          {staleCount > 0 && (
            <>
              <div className="mx-0.5 h-4 w-px bg-[var(--relay-border)]" />
              <Button
                size="sm"
                variant="ghost"
                className="h-8 whitespace-nowrap px-2.5 text-[11.5px]"
                onPress={onToggleIncludeStale}
              >
                {includeStale ? (
                  <Eye className="h-3.5 w-3.5" />
                ) : (
                  <EyeOff className="h-3.5 w-3.5" />
                )}
                {includeStale ? t("canvas.dock.showOutdated") : t("canvas.dock.hideOutdated")}
              </Button>
            </>
          )}

          <Button
            size="sm"
            variant="ghost"
            isIconOnly
            className="h-8 w-8 min-w-8"
            aria-label={t("canvas.dock.fitView")}
            onPress={() => void fitView({ padding: 0.28, maxZoom: 1.05, duration: 280 })}
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
