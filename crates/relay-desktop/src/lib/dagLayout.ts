import type { Edge, Node } from "@xyflow/react";
import type { DagSnapshot_Serialize, GraphEdge, GraphNode } from "../bindings";

export interface RelayFlowNodeData extends Record<string, unknown> {
  node: GraphNode;
  selected?: boolean;
}

export type RelayFlowNode = Node<RelayFlowNodeData>;

const COLUMN_X = {
  decision: 48,
  artifact: 420,
  evidence: 810,
} as const;

const ROW_GAP = {
  decision: 210,
  artifact: 220,
  evidence: 240,
} as const;

/**
 * Deterministic left-to-right layered DAG layout:
 * Column 0 = Decision | Column 1 = Artifact (Commit/Env) | Column 2 = Evidence (Metrics)
 * Ensures stable spatial muscle-memory during live demos when STALE cascades occur.
 */
export function buildRelayFlowGraph(
  snapshot: DagSnapshot_Serialize,
  selectedNodeId: string | null,
): { nodes: RelayFlowNode[]; edges: Edge[] } {
  const buckets: Record<"decision" | "artifact" | "evidence", GraphNode[]> = {
    decision: [],
    artifact: [],
    evidence: [],
  };

  for (const node of snapshot.nodes) {
    buckets[node.payload.kind].push(node);
  }

  for (const kind of ["decision", "artifact", "evidence"] as const) {
    buckets[kind].sort((a, b) => a.created_at.localeCompare(b.created_at));
  }

  const nodeById = new Map<string, GraphNode>(snapshot.nodes.map((n) => [n.id, n]));
  const flowNodes: RelayFlowNode[] = [];

  for (const kind of ["decision", "artifact", "evidence"] as const) {
    const list = buckets[kind];
    const startY = 48 + Math.max(0, (3 - list.length) * 32);
    list.forEach((node, idx) => {
      flowNodes.push({
        id: node.id,
        type: kind,
        position: {
          x: COLUMN_X[kind],
          y: startY + idx * ROW_GAP[kind],
        },
        selected: node.id === selectedNodeId,
        data: {
          node,
          selected: node.id === selectedNodeId,
        },
      });
    });
  }

  const flowEdges: Edge[] = snapshot.edges.map((edge: GraphEdge) => {
    const targetNode = nodeById.get(edge.to);
    const isTargetStale = targetNode?.freshness.status === "stale";
    const isSupersedes = edge.kind === "supersedes";

    let stroke = "#94A3B8";
    let animated = false;
    let strokeDasharray: string | undefined;

    if (isTargetStale) {
      stroke = "#DC2626";
      animated = true;
      strokeDasharray = "6 4";
    } else if (isSupersedes) {
      stroke = "#4F46E5";
      animated = true;
    } else if (edge.kind === "produced_from") {
      stroke = "#0D9488";
    } else if (edge.kind === "informs") {
      stroke = "#64748B";
      strokeDasharray = "4 4";
    }

    return {
      id: edge.id,
      source: edge.from,
      target: edge.to,
      type: isSupersedes ? "smoothstep" : "bezier",
      animated,
      label: edge.kind,
      labelStyle: {
        fontSize: 10,
        fontFamily: "var(--font-mono)",
        fontWeight: 500,
        fill: isTargetStale ? "#DC2626" : isSupersedes ? "#4F46E5" : "#64748B",
      },
      labelBgStyle: {
        fill: "var(--relay-edge-label-bg)",
        fillOpacity: 0.92,
      },
      labelBgPadding: [6, 3] as [number, number],
      labelBgBorderRadius: 6,
      style: {
        stroke,
        strokeWidth: isTargetStale || isSupersedes ? 2.2 : 1.75,
        strokeDasharray,
      },
    };
  });

  return { nodes: flowNodes, edges: flowEdges };
}
