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
import {
  AlertCircle,
  Eye,
  EyeOff,
  FilePlus2,
  FolderGit2,
  Maximize2,
  Play,
  Plus,
  X,
} from "lucide-react";
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
  hasRepo: boolean;
  isGitRepo: boolean;
  hasManifest: boolean;
  isGeneratingManifest: boolean;
  onToggleIncludeStale: () => void;
  onSelectNode: (nodeId: string | null) => void;
  onOpenRecordDecision: () => void;
  onOpenDispatchTask: () => void;
  onOpenRepoSettings: () => void;
  manifestError: string | null;
  onDismissManifestError?: () => void;
  onGenerateManifest: () => void;
}

function DagCanvasInner({
  snapshot,
  selectedNodeId,
  includeStale,
  hasRepo,
  isGitRepo,
  hasManifest,
  isGeneratingManifest,
  onToggleIncludeStale,
  onSelectNode,
  onOpenRecordDecision,
  onOpenDispatchTask,
  onOpenRepoSettings,
  manifestError,
  onDismissManifestError,
  onGenerateManifest,
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
  const needsManifestBanner = hasRepo && isGitRepo && !hasManifest && nodes.length > 0;
  const showDock = hasRepo || nodes.length > 0 || staleCount > 0;

  return (
    <div
      ref={containerRef}
      className="relative h-full w-full flex-1 overflow-hidden bg-[var(--relay-canvas)]"
    >
      {(staleCount > 0 || needsManifestBanner || manifestError) && (
        <div className="pointer-events-none absolute top-4 right-4 left-4 z-20 flex flex-col items-center gap-2">
          {staleCount > 0 && (
            <div className="pointer-events-auto inline-flex items-center gap-2 rounded-lg border border-destructive/30 bg-card/95 px-3 py-1.5 text-xs font-medium text-destructive shadow-sm backdrop-blur-md">
              <AlertCircle className="size-3.5 shrink-0" />
              <span className="relay-text-trim leading-none">
                {t("canvas.outdatedBanner", { count: staleCount })}
              </span>
            </div>
          )}
          {manifestError && (
            <div
              role="alert"
              className="pointer-events-auto inline-flex items-center gap-2 rounded-lg border border-destructive/30 bg-card/95 px-3 py-1.5 text-xs font-medium text-destructive shadow-sm backdrop-blur-md"
            >
              <AlertCircle className="size-3.5 shrink-0" />
              <span>{manifestError}</span>
              {onDismissManifestError && (
                <Button
                  type="button"
                  size="icon-xs"
                  variant="ghost"
                  aria-label={t("detail.close")}
                  onClick={onDismissManifestError}
                >
                  <X />
                </Button>
              )}
            </div>
          )}
          {needsManifestBanner && (
            <div className="pointer-events-auto inline-flex items-center gap-2.5 rounded-lg border border-border bg-card/95 px-3 py-1.5 text-xs text-foreground shadow-sm backdrop-blur-md">
              <FilePlus2 className="size-3.5 shrink-0 text-primary" />
              <span>{t("canvas.missingManifestBanner")}</span>
              <Button
                type="button"
                size="xs"
                disabled={isGeneratingManifest}
                onClick={onGenerateManifest}
              >
                <span className="relay-text-trim leading-none">
                  {t("canvas.generateManifestBtn")}
                </span>
              </Button>
            </div>
          )}
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

      {nodes.length === 0 && staleCount === 0 && (
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center p-8">
          <div className="pointer-events-auto max-w-md rounded-lg border border-border bg-card p-6 text-center shadow-sm">
            {!hasRepo ? (
              <>
                <h3 className="text-sm font-semibold text-foreground">
                  {t("canvas.welcomeTitle")}
                </h3>
                <p className="mt-1.5 text-xs leading-relaxed text-muted-foreground">
                  {t("canvas.welcomeDesc")}
                </p>
                <div className="mt-4 flex flex-wrap justify-center gap-2">
                  <Button size="default" onClick={onOpenRepoSettings}>
                    <FolderGit2 data-icon="inline-start" />
                    <span className="relay-text-trim leading-none">{t("canvas.openRepoCta")}</span>
                  </Button>
                </div>
              </>
            ) : isGitRepo && !hasManifest ? (
              <>
                <h3 className="text-sm font-semibold text-foreground">
                  {t("canvas.missingManifestTitle")}
                </h3>
                <p className="mt-1.5 text-xs leading-relaxed text-muted-foreground">
                  {t("canvas.missingManifestDesc")}
                </p>
                <div className="mt-4 flex flex-wrap justify-center gap-2">
                  <Button
                    size="default"
                    disabled={isGeneratingManifest}
                    onClick={onGenerateManifest}
                  >
                    <FilePlus2 data-icon="inline-start" />
                    <span className="relay-text-trim leading-none">
                      {t("canvas.generateManifestBtn")}
                    </span>
                  </Button>
                  <Button variant="outline" size="default" onClick={onOpenRepoSettings}>
                    <FolderGit2 data-icon="inline-start" />
                    <span className="relay-text-trim leading-none">
                      {t("canvas.openRepoSettingsCta")}
                    </span>
                  </Button>
                </div>
              </>
            ) : (
              <>
                <h3 className="text-sm font-semibold text-foreground">{t("canvas.emptyTitle")}</h3>
                <p className="mt-1.5 text-xs leading-relaxed text-muted-foreground">
                  {t("canvas.emptyDesc")}
                </p>
                <div className="mt-4 flex justify-center gap-2">
                  <Button size="default" onClick={onOpenDispatchTask}>
                    <Play data-icon="inline-start" />
                    <span className="relay-text-trim leading-none">{t("canvas.emptyCta")}</span>
                  </Button>
                  <Button variant="secondary" size="default" onClick={onOpenRecordDecision}>
                    <Plus data-icon="inline-start" />
                    <span className="relay-text-trim leading-none">{t("canvas.dock.addNote")}</span>
                  </Button>
                </div>
              </>
            )}
          </div>
        </div>
      )}

      {showDock && (
        <div className="pointer-events-none absolute right-4 bottom-4 left-4 z-20 flex justify-center">
          <div className="pointer-events-auto flex items-center gap-1 rounded-lg border border-border bg-card/95 p-1 shadow-md backdrop-blur-md">
            <Button variant="secondary" size="default" onClick={onOpenRecordDecision}>
              <Plus data-icon="inline-start" />
              <span className="relay-text-trim leading-none">{t("canvas.dock.addNote")}</span>
            </Button>
            <Button variant="default" size="default" onClick={onOpenDispatchTask}>
              <Play data-icon="inline-start" />
              <span className="relay-text-trim leading-none">{t("canvas.dock.runTask")}</span>
            </Button>

            {staleCount > 0 && (
              <>
                <Separator
                  orientation="vertical"
                  className="mx-0.5 my-auto h-4 data-vertical:self-center"
                />
                <Button variant="ghost" size="default" onClick={onToggleIncludeStale}>
                  {includeStale ? (
                    <Eye data-icon="inline-start" />
                  ) : (
                    <EyeOff data-icon="inline-start" />
                  )}
                  <span className="relay-text-trim leading-none">
                    {includeStale ? t("canvas.dock.showOutdated") : t("canvas.dock.hideOutdated")}
                  </span>
                </Button>
              </>
            )}

            <Button
              variant="ghost"
              size="icon"
              aria-label={t("canvas.dock.fitView")}
              onClick={() => void fitView({ padding: 0.18, maxZoom: 1.05, duration: 280 })}
            >
              <Maximize2 />
            </Button>
          </div>
        </div>
      )}
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
