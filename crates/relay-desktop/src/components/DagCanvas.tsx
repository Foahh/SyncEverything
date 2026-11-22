import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
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
        void fitView({ padding: 0.18, maxZoom: 1.05, duration: 300 });
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
        void fitView({ padding: 0.18, maxZoom: 1.05, duration: 220 });
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
      className="relative h-full w-full flex-1 overflow-hidden bg-[var(--relay-canvas)]"
    >
      {staleCount > 0 && (
        <div className="pointer-events-none absolute top-4 right-4 left-4 z-20 flex justify-center">
          <div className="pointer-events-auto inline-flex items-center gap-2 rounded-full border border-destructive/30 bg-card/95 px-3.5 py-1.5 text-xs font-medium text-destructive shadow-sm backdrop-blur-md">
            <AlertCircle className="size-3.5 shrink-0" />
            <span>{t("canvas.outdatedBanner", { count: staleCount })}</span>
          </div>
        </div>
      )}

      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        onNodeClick={(_, node) => onSelectNode(node.id)}
        onPaneClick={() => onSelectNode(null)}
        fitView
        fitViewOptions={{ padding: 0.18, maxZoom: 1.05 }}
        minZoom={0.25}
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

      {nodes.length === 0 && (
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center p-8">
          <div className="pointer-events-auto max-w-sm rounded-2xl border border-border bg-card p-6 text-center shadow-sm">
            <h3 className="text-sm font-semibold text-foreground">{t("canvas.emptyTitle")}</h3>
            <p className="mt-1.5 text-xs leading-relaxed text-muted-foreground">
              {t("canvas.emptyDesc")}
            </p>
            <div className="mt-4 flex justify-center gap-2">
              <Button size="lg" onClick={onOpenDispatchTask}>
                <Play data-icon="inline-start" />
                {t("canvas.emptyCta")}
              </Button>
              <Button variant="secondary" size="lg" onClick={onOpenRecordDecision}>
                <Plus data-icon="inline-start" />
                {t("canvas.dock.addNote")}
              </Button>
            </div>
          </div>
        </div>
      )}

      <div className="pointer-events-none absolute right-4 bottom-4 left-4 z-20 flex justify-center">
        <div className="pointer-events-auto flex items-center gap-1 rounded-xl border border-border bg-card/95 p-1.5 shadow-lg backdrop-blur-md">
          <Button variant="secondary" size="lg" onClick={onOpenRecordDecision}>
            <Plus data-icon="inline-start" />
            {t("canvas.dock.addNote")}
          </Button>
          <Button variant="default" size="lg" onClick={onOpenDispatchTask}>
            <Play data-icon="inline-start" />
            {t("canvas.dock.runTask")}
          </Button>

          {staleCount > 0 && (
            <>
              <Separator orientation="vertical" className="mx-0.5 h-4" />
              <Button variant="ghost" size="lg" onClick={onToggleIncludeStale}>
                {includeStale ? (
                  <Eye data-icon="inline-start" />
                ) : (
                  <EyeOff data-icon="inline-start" />
                )}
                {includeStale ? t("canvas.dock.showOutdated") : t("canvas.dock.hideOutdated")}
              </Button>
            </>
          )}

          <Button
            variant="ghost"
            size="icon-lg"
            aria-label={t("canvas.dock.fitView")}
            onClick={() => void fitView({ padding: 0.18, maxZoom: 1.05, duration: 280 })}
          >
            <Maximize2 />
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
