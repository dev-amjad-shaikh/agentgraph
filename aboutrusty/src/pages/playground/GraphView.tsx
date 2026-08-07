import type { GraphEdgeDef, GraphNodeDef, RouteEdge, ScenarioDef } from "./engine";

interface GraphViewProps {
  def: ScenarioDef;
  /** Nodes executing right now (pulse in rust). */
  activeNodes: string[];
  /** The scheduled next-node set. */
  nextNodes: string[];
  /** Routes fired by the last route phase. */
  firedRoutes: RouteEdge[];
  /** Bumped on every route phase to retrigger the edge animation. */
  routeAnimKey: number;
}

const NODE_W = 76;
const NODE_H = 32;
const END_W = 52;

function halfW(n: GraphNodeDef): number {
  return (n.kind === "end" ? END_W : NODE_W) / 2;
}

function edgePath(from: GraphNodeDef, to: GraphNodeDef): string {
  const hh = NODE_H / 2;
  if (from.y === to.y) {
    if (from.x < to.x) {
      return `M ${from.x + halfW(from)} ${from.y} L ${to.x - halfW(to)} ${to.y}`;
    }
    // Reverse direction on the same row (the ReAct loopback): arc underneath.
    const my = from.y + 62;
    return `M ${from.x - 10} ${from.y + hh} C ${from.x - 24} ${my}, ${to.x + 24} ${my}, ${to.x + 10} ${to.y + hh}`;
  }
  // Downward edge (to __end__): bottom center to top center.
  return `M ${from.x} ${from.y + hh} L ${to.x} ${to.y - hh + 4}`;
}

function labelPos(from: GraphNodeDef, to: GraphNodeDef): { x: number; y: number } {
  if (from.y === to.y && from.x > to.x) {
    return { x: (from.x + to.x) / 2, y: from.y + 56 };
  }
  return { x: (from.x + to.x) / 2, y: (from.y + to.y) / 2 - 6 };
}

/**
 * The compiled graph, drawn as plain SVG — no graph library. The active node
 * pulses in the primary rust color; when routing fires (Command::goto /
 * Route), a packet travels the chosen edge.
 */
export function GraphView({
  def,
  activeNodes,
  nextNodes,
  firedRoutes,
  routeAnimKey,
}: GraphViewProps) {
  const nodeById = new Map(def.nodes.map((n) => [n.id, n]));
  const fired = new Set(firedRoutes.map((r) => `${r.from}->${r.to}`));

  const renderEdge = (e: GraphEdgeDef) => {
    const from = nodeById.get(e.from);
    const to = nodeById.get(e.to);
    if (!from || !to) return null;
    const d = edgePath(from, to);
    const isFired = fired.has(`${e.from}->${e.to}`);
    const lp = labelPos(from, to);
    return (
      <g key={`${e.from}->${e.to}`}>
        <path
          d={d}
          fill="none"
          className={
            isFired
              ? "stroke-primary"
              : e.kind === "conditional"
                ? "stroke-muted-foreground/50"
                : "stroke-muted-foreground/70"
          }
          strokeWidth={isFired ? 2.2 : 1.4}
          strokeDasharray={e.kind === "conditional" ? "5 4" : undefined}
          markerEnd="url(#rusty-arrow)"
        />
        {e.label && (
          <text
            x={lp.x}
            y={lp.y}
            textAnchor="middle"
            className="fill-muted-foreground font-code"
            fontSize={9}
          >
            {e.label}
          </text>
        )}
        {isFired && (
          <circle key={routeAnimKey} r={4} className="fill-primary">
            <animateMotion dur="0.7s" path={d} fill="freeze" />
            <animate
              attributeName="opacity"
              from="1"
              to="0"
              begin="0.6s"
              dur="0.25s"
              fill="freeze"
            />
          </circle>
        )}
      </g>
    );
  };

  const renderNode = (n: GraphNodeDef) => {
    const isActive = activeNodes.includes(n.id);
    const isNext = !isActive && nextNodes.includes(n.id);
    const w = halfW(n) * 2;
    return (
      <g key={n.id} className={isActive ? "animate-pulse" : undefined}>
        <rect
          x={n.x - w / 2}
          y={n.y - NODE_H / 2}
          width={w}
          height={NODE_H}
          rx={n.kind === "end" ? NODE_H / 2 : 8}
          className={
            isActive
              ? "fill-primary stroke-primary"
              : n.kind === "end"
                ? "fill-muted stroke-muted-foreground/50"
                : isNext
                  ? "fill-accent stroke-primary/60"
                  : "fill-card stroke-border"
          }
          strokeWidth={isActive ? 2 : isNext ? 1.6 : 1.2}
          strokeDasharray={n.kind === "end" ? "4 3" : undefined}
        />
        <text
          x={n.x}
          y={n.y + 3.5}
          textAnchor="middle"
          fontSize={11}
          className={
            isActive
              ? "fill-primary-foreground font-code font-semibold"
              : "fill-foreground font-code"
          }
        >
          {n.label}
        </text>
        {isActive && (
          <text
            x={n.x}
            y={n.y - NODE_H / 2 - 6}
            textAnchor="middle"
            fontSize={8.5}
            className="fill-primary font-code"
          >
            running
          </text>
        )}
        {isNext && (
          <text
            x={n.x}
            y={n.y - NODE_H / 2 - 6}
            textAnchor="middle"
            fontSize={8.5}
            className="fill-muted-foreground font-code"
          >
            next
          </text>
        )}
      </g>
    );
  };

  return (
    <svg
      viewBox="0 0 340 210"
      className="h-auto w-full"
      role="img"
      aria-label={`Graph topology for ${def.graphName}`}
    >
      <defs>
        <marker
          id="rusty-arrow"
          viewBox="0 0 10 10"
          refX="9"
          refY="5"
          markerWidth="7"
          markerHeight="7"
          orient="auto-start-reverse"
        >
          <path d="M 0 1 L 9 5 L 0 9 z" className="fill-muted-foreground/70" />
        </marker>
      </defs>
      {def.edges.map(renderEdge)}
      {def.nodes.map(renderNode)}
    </svg>
  );
}
