import type { Edge, Node } from "@xyflow/react";
import type { DagSnapshot_Serialize, GraphEdge, GraphNode } from "../bindings";

export interface RelayFlowNodeData extends Record<string, unknown> {
  node: GraphNode;
  selected?: boolean;
}

export type RelayFlowNode = Node<RelayFlowNodeData>;

const COLUMN_X = {
  decision: 40,
  artifact: 360,
  evidence: 680,
} as const;

const ROW_GAP = {
  decision: 170,
  artifact: 175,
  evidence: 195,
} as const;

/**
 * Clean left-to-right workflow layout:
 * Notes -> Code Versions -> Run Results
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
    const startY = 48 + Math.max(0, (3 - list.length) * 28);
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
      strokeDasharray = "5 4";
    } else if (isSupersedes) {
      stroke = "#4F46E5";
      animated = true;
    } else if (edge.kind === "produced_from") {
      stroke = "#0D9488";
    } else if (edge.kind === "informs") {
      stroke = "#94A3B8";
      strokeDasharray = "4 4";
    }

    return {
      id: edge.id,
      source: edge.from,
      target: edge.to,
      type: isSupersedes ? "smoothstep" : "bezier",
      animated,
      style: {
        stroke,
        strokeWidth: isTargetStale || isSupersedes ? 2.1 : 1.6,
        strokeDasharray,
      },
    };
  });

  return { nodes: flowNodes, edges: flowEdges };
}
