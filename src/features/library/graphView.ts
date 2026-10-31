// The library map's canvas engine: a d3 force simulation drawn on a 2D canvas,
// with pan, zoom, dragging and hover. React only owns the overlay around it.
import {
  type Force,
  forceCollide,
  forceLink,
  forceManyBody,
  forceSimulation,
  forceX,
  forceY,
  type Simulation,
  type SimulationLinkDatum,
  type SimulationNodeDatum,
} from "d3-force";
import type { Graph } from "../../lib/api";

// Categorical slots in fixed order, stepped for the dark surface the map is drawn on
// (validated: adjacent CVD ΔE ≥ 8.4, all ≥ 3:1 contrast). Topics are also grouped in
// space and named in the legend and tooltip, so color never carries identity alone.
export const TOPIC_COLORS = ["#3987e5", "#d95926", "#199e70", "#c98500", "#d55181", "#008300", "#9085e9", "#e66767"];
export const OTHER = "#6b6a65";
export const SURFACE = "#1a1a19";
export const INK = "#f0efec";
export const MUTED = "#898781";

/**
 * A human brain seen from the side, facing left, as a union of ellipses
 * `[cx, cy, rx, ry]` in units where the cerebrum is two wide.
 */
const BRAIN: [number, number, number, number][] = [
  [0.02, -0.14, 1.0, 0.64], // cerebrum
  [-0.6, 0.02, 0.42, 0.46], // frontal lobe
  [-0.12, 0.26, 0.62, 0.3], // temporal lobe
  [0.6, 0.42, 0.34, 0.21], // cerebellum
  [0.3, 0.6, 0.1, 0.26], // brainstem
];
/** Where the eight largest topics settle inside the brain, like lobes, in the same units. */
const LOBES: [number, number][] = [
  [-0.62, -0.12], // frontal
  [0.32, -0.42], // parietal
  [-0.18, 0.28], // temporal
  [0.78, -0.12], // occipital
  [-0.2, -0.5], // motor strip
  [0.6, 0.42], // cerebellum
  [0.25, 0.08], // deep
  [-0.6, 0.3], // lower frontal
];
const BRAIN_BOUNDS = { minX: -1.02, maxX: 1.02, minY: -0.78, maxY: 0.86 };
/** Area of the outline in those units, sampled once. */
const BRAIN_AREA = (() => {
  let inside = 0;
  for (let i = 0; i < 200; i++) {
    for (let j = 0; j < 200; j++) {
      const x = -1.2 + (i / 200) * 2.4;
      const y = -0.9 + (j / 200) * 1.9;
      if (BRAIN.some(([cx, cy, rx, ry]) => Math.hypot((x - cx) / rx, (y - cy) / ry) <= 1)) inside++;
    }
  }
  return (inside / 40000) * 2.4 * 1.9;
})();
/** Below this many files the map is free-form; from `BRAIN_FULL` on it fills the brain. */
const BRAIN_FROM = 80;
const BRAIN_FULL = 300;
/** Room per file inside the brain, in map units. */
const SPACING = 24;

type SimNode = SimulationNodeDatum & { index: number; r: number; color: string };
type SimLink = SimulationLinkDatum<SimNode> & { strong: boolean; weight: number };

/**
 * Pulls every file that is outside the brain outline (scaled by `scale`) back to
 * its nearest edge. With the files' mutual repulsion, they fill the shape.
 */
function brain(scale: number, strength: number): Force<SimNode, SimLink> {
  let nodes: SimNode[] = [];
  const force = () => {
    for (const n of nodes) {
      const x = (n.x ?? 0) / scale;
      const y = (n.y ?? 0) / scale;
      let nearest = Infinity;
      let tx = 0;
      let ty = 0;
      for (const [cx, cy, rx, ry] of BRAIN) {
        const d = Math.hypot((x - cx) / rx, (y - cy) / ry);
        if (d <= 1) {
          nearest = 0;
          break;
        }
        if (d < nearest) {
          nearest = d;
          tx = cx + (x - cx) / d;
          ty = cy + (y - cy) / d;
        }
      }
      if (nearest === 0) continue;
      // Not scaled by alpha: the outline stays firm while the layout cools down.
      n.vx = (n.vx ?? 0) + (tx * scale - (n.x ?? 0)) * strength;
      n.vy = (n.vy ?? 0) + (ty * scale - (n.y ?? 0)) * strength;
    }
  };
  force.initialize = (all: SimNode[]) => {
    nodes = all;
  };
  return force;
}

/** The color of a topic: the eight largest named topics, gray for the rest. */
export function topicColor(graph: Graph, cluster: number): string {
  const c = graph.clusters[cluster];
  return (c?.label && TOPIC_COLORS[cluster]) || OTHER;
}

export type Hover = { node: number; x: number; y: number } | null;

export class GraphView {
  private graph: Graph = { nodes: [], edges: [], clusters: [] };
  private nodes: SimNode[] = [];
  private links: SimLink[] = [];
  private sim: Simulation<SimNode, SimLink> | null = null;
  private view = { x: 0, y: 0, k: 1 };
  /** Where each file was, so an update only animates what is new. */
  private positions = new Map<string, { x: number; y: number }>();
  /** Once the user pans or zooms, the view stops fitting itself. */
  private touched = false;
  private drag: { node: SimNode | null; px: number; py: number; moved: boolean } | null = null;
  private hover: Hover = null;
  private selected: number | null = null;
  private topic: number | null = null;
  private observer: ResizeObserver;
  /** How much the map is a brain (0–1) and the brain's size, for fitting the view. */
  private shape = { amount: 0, scale: 1 };
  private readonly reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly onHover: (hover: Hover) => void,
    private readonly onSelect: (node: number | null) => void,
  ) {
    this.observer = new ResizeObserver(() => this.resize());
    this.observer.observe(canvas);
  }

  destroy() {
    this.sim?.stop();
    this.observer.disconnect();
  }

  setGraph(graph: Graph) {
    this.sim?.stop();
    this.graph = graph;
    const count = graph.nodes.length;
    // 0 for a small library, 1 for a big one: how much the map takes the shape of a brain.
    const shape = Math.min(1, Math.max(0, (count - BRAIN_FROM) / (BRAIN_FULL - BRAIN_FROM)));
    const size = 1 - 0.3 * shape;
    const scale = SPACING * Math.sqrt(count / BRAIN_AREA);
    this.shape = { amount: shape, scale };
    // Each named topic heads for its lobe as the brain forms; loose files for the middle.
    const anchor = (i: number): [number, number] => {
      const cluster = graph.nodes[i]?.cluster ?? -1;
      const lobe = graph.clusters[cluster]?.label ? LOBES[cluster] : undefined;
      return lobe ?? [0.1, -0.1];
    };
    const pull = (n: SimNode) => {
      const free = graph.nodes[n.index]?.degree ? 0.06 : 0.2;
      // Gently, so the files still spread to fill the whole outline.
      return free + (0.025 - free) * shape;
    };
    this.nodes = graph.nodes.map((n, index) => ({
      index,
      r: (3.5 + Math.sqrt(n.degree) * 2.2) * size,
      color: topicColor(graph, n.cluster),
      ...this.positions.get(n.path),
    }));
    // f32 weights arrive as `number | null` in the bindings.
    this.links = graph.edges.map((e) => ({
      source: e.source,
      target: e.target,
      strong: e.kind === "link",
      weight: e.weight ?? 0,
    }));
    const sim = forceSimulation(this.nodes)
      .force(
        "link",
        forceLink<SimNode, SimLink>(this.links)
          .distance((l) => (l.strong ? 55 : 50 + (1 - l.weight) * 80))
          .strength((l) => (l.strong ? 0.5 : 0.2 + l.weight * 0.4)),
      )
      // A big library packs denser, so the repulsion reaches less far.
      .force("charge", forceManyBody().strength(-160 + 100 * shape).distanceMax(350 - 250 * shape))
      .force("collide", forceCollide<SimNode>((n) => n.r + 3 * size))
      // Files on their own drift away under the repulsion, so they are pulled in harder.
      .force("x", forceX<SimNode>((n) => anchor(n.index)[0] * scale * shape).strength(pull))
      .force("y", forceY<SimNode>((n) => anchor(n.index)[1] * scale * shape).strength(pull))
      .force("brain", shape > 0 ? brain(scale, 0.5 * shape) : null)
      .on("tick", () => this.draw())
      .on("end", () => this.settled());
    this.sim = sim;
    if (this.reduced) {
      sim.stop();
      sim.tick(300);
      this.settled();
    }
  }

  setSelected(node: number | null) {
    this.selected = node;
    this.draw();
  }

  setTopic(topic: number | null) {
    this.topic = topic;
    this.draw();
  }

  private settled() {
    for (const n of this.nodes) {
      const path = this.graph.nodes[n.index]?.path;
      if (path) this.positions.set(path, { x: n.x ?? 0, y: n.y ?? 0 });
    }
    if (!this.touched) this.fit();
    this.draw();
  }

  private resize() {
    const dpr = window.devicePixelRatio || 1;
    const { width, height } = this.canvas.getBoundingClientRect();
    this.canvas.width = Math.round(width * dpr);
    this.canvas.height = Math.round(height * dpr);
    this.draw();
  }

  /** Zooms so the files are in view, below the topic legend; a few strays do not shrink the map. */
  private fit() {
    if (this.nodes.length === 0) return;
    const { width, height } = this.canvas.getBoundingClientRect();
    const legend = 64;
    const span = (values: number[]): [number, number] => {
      const sorted = [...values].sort((a, b) => a - b);
      const cut = Math.floor(sorted.length * 0.05);
      return [sorted[cut] ?? 0, sorted[sorted.length - 1 - cut] ?? 0];
    };
    // A brain is framed whole, not just where the files happen to be.
    const s = this.shape.scale;
    const [[minX, maxX], [minY, maxY]] =
      this.shape.amount >= 0.5
        ? [
            [BRAIN_BOUNDS.minX * s, BRAIN_BOUNDS.maxX * s],
            [BRAIN_BOUNDS.minY * s, BRAIN_BOUNDS.maxY * s],
          ]
        : [span(this.nodes.map((n) => n.x ?? 0)), span(this.nodes.map((n) => n.y ?? 0))];
    const k = Math.min(
      2,
      0.9 * Math.min(width / Math.max(maxX - minX, 1), (height - legend) / Math.max(maxY - minY, 1)),
    );
    this.view = { k, x: (-(minX + maxX) / 2) * k, y: (-(minY + maxY) / 2) * k + legend / 2 };
  }

  private neighbours(i: number): Set<number> {
    const set = new Set([i]);
    for (const e of this.graph.edges) {
      if (e.source === i) set.add(e.target);
      if (e.target === i) set.add(e.source);
    }
    return set;
  }

  draw() {
    const ctx = this.canvas.getContext("2d");
    if (!ctx) return;
    const dpr = window.devicePixelRatio || 1;
    const { width, height } = this.canvas.getBoundingClientRect();
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.fillStyle = SURFACE;
    ctx.fillRect(0, 0, width, height);
    const { x, y, k } = this.view;
    ctx.translate(width / 2 + x, height / 2 + y);
    ctx.scale(k, k);

    // The brain's silhouette, faint, as the library grows into it.
    if (this.shape.amount > 0) {
      const s = this.shape.scale;
      ctx.beginPath();
      for (const [cx, cy, rx, ry] of BRAIN) {
        ctx.moveTo((cx + rx) * s, cy * s);
        ctx.ellipse(cx * s, cy * s, rx * s, ry * s, 0, 0, Math.PI * 2);
      }
      ctx.fillStyle = `rgba(240,239,236,${0.045 * this.shape.amount})`;
      ctx.fill("nonzero");
    }

    const focus = this.hover?.node ?? this.selected;
    const lit = focus !== null ? this.neighbours(focus) : null;
    const dim = (n: SimNode) =>
      lit ? !lit.has(n.index) : this.topic !== null && this.graph.nodes[n.index]?.cluster !== this.topic;
    const focusColor = focus !== null ? (this.nodes[focus]?.color ?? INK) : INK;

    for (const l of this.links) {
      const s = l.source as SimNode;
      const d = l.target as SimNode;
      const active = focus !== null && (s.index === focus || d.index === focus);
      const alpha = dim(s) || dim(d) ? 0.03 : l.strong ? 0.28 : 0.1;
      ctx.strokeStyle = active ? focusColor : `rgba(240,239,236,${alpha})`;
      ctx.lineWidth = (active ? 1.6 : l.strong ? 1.2 : 0.8) / Math.sqrt(k);
      ctx.beginPath();
      ctx.moveTo(s.x ?? 0, s.y ?? 0);
      ctx.lineTo(d.x ?? 0, d.y ?? 0);
      ctx.stroke();
    }

    for (const n of this.nodes) {
      ctx.globalAlpha = dim(n) ? 0.22 : 1;
      ctx.fillStyle = n.color;
      ctx.beginPath();
      ctx.arc(n.x ?? 0, n.y ?? 0, n.r, 0, Math.PI * 2);
      ctx.fill();
      if (n.index === this.selected) {
        // A surface ring, then an ink ring, so the selection reads on any color.
        ctx.lineWidth = 2 / k;
        ctx.strokeStyle = SURFACE;
        ctx.stroke();
        ctx.beginPath();
        ctx.arc(n.x ?? 0, n.y ?? 0, n.r + 3 / k, 0, Math.PI * 2);
        ctx.strokeStyle = INK;
        ctx.stroke();
      }
    }
    ctx.globalAlpha = 1;

    // Names appear only around the file under the cursor (or the selected one),
    // the file's own first; a name that would cover another is left out.
    if (!lit) return;
    ctx.font = `${12 / k}px system-ui, -apple-system, "Segoe UI", sans-serif`;
    ctx.textBaseline = "middle";
    ctx.lineJoin = "round";
    const placed: [number, number, number, number][] = [];
    const order = [...this.nodes].sort((a, b) => (a.index === focus ? -1 : b.index === focus ? 1 : 0));
    for (const n of order) {
      if (!lit.has(n.index)) continue;
      const label = this.graph.nodes[n.index]?.title ?? "";
      const lx = (n.x ?? 0) + n.r + 4 / k;
      const box: [number, number, number, number] = [lx, (n.y ?? 0) - 7 / k, ctx.measureText(label).width, 14 / k];
      const overlaps = placed.some(
        ([x2, y2, w2, h2]) => box[0] < x2 + w2 && x2 < box[0] + box[2] && box[1] < y2 + h2 && y2 < box[1] + box[3],
      );
      if (overlaps) continue;
      placed.push(box);
      ctx.lineWidth = 3 / k;
      ctx.strokeStyle = SURFACE;
      ctx.strokeText(label, lx, n.y ?? 0);
      ctx.fillStyle = INK;
      ctx.fillText(label, lx, n.y ?? 0);
    }
  }

  /** The file under a point given in canvas pixels, with a hit area larger than the dot. */
  private nodeAt(px: number, py: number): SimNode | null {
    const { width, height } = this.canvas.getBoundingClientRect();
    const { x, y, k } = this.view;
    const wx = (px - width / 2 - x) / k;
    const wy = (py - height / 2 - y) / k;
    let best: SimNode | null = null;
    let bestDistance = Infinity;
    for (const n of this.nodes) {
      const d = Math.hypot((n.x ?? 0) - wx, (n.y ?? 0) - wy);
      if (d < n.r + 6 / k && d < bestDistance) {
        best = n;
        bestDistance = d;
      }
    }
    return best;
  }

  private point(e: PointerEvent | WheelEvent) {
    const rect = this.canvas.getBoundingClientRect();
    return { px: e.clientX - rect.left, py: e.clientY - rect.top };
  }

  pointerDown(e: PointerEvent) {
    const { px, py } = this.point(e);
    this.canvas.setPointerCapture(e.pointerId);
    this.drag = { node: this.nodeAt(px, py), px, py, moved: false };
  }

  pointerMove(e: PointerEvent) {
    const { px, py } = this.point(e);
    const d = this.drag;
    if (!d) {
      const n = this.nodeAt(px, py);
      this.hover = n ? { node: n.index, x: px, y: py } : null;
      this.canvas.style.cursor = n ? "pointer" : "grab";
      this.onHover(this.hover);
      this.draw();
      return;
    }
    const dx = px - d.px;
    const dy = py - d.py;
    if (!d.moved && Math.hypot(dx, dy) < 3) return;
    d.moved = true;
    this.touched = true;
    if (d.node) {
      d.node.fx = (d.node.fx ?? d.node.x ?? 0) + dx / this.view.k;
      d.node.fy = (d.node.fy ?? d.node.y ?? 0) + dy / this.view.k;
      this.sim?.alphaTarget(0.2).restart();
    } else {
      this.view = { ...this.view, x: this.view.x + dx, y: this.view.y + dy };
      this.draw();
    }
    d.px = px;
    d.py = py;
  }

  pointerUp() {
    const d = this.drag;
    this.drag = null;
    if (!d) return;
    if (d.node && d.moved) {
      d.node.fx = null;
      d.node.fy = null;
      this.sim?.alphaTarget(0);
    } else if (!d.moved) {
      this.onSelect(d.node ? d.node.index : null);
    }
  }

  pointerLeave() {
    this.hover = null;
    this.onHover(null);
    this.draw();
  }

  /** Pinch, or the wheel with ⌘/Ctrl, zooms around the pointer; a plain wheel scrolls the page. */
  wheel(e: WheelEvent) {
    if (!e.ctrlKey && !e.metaKey) return;
    e.preventDefault();
    const { px, py } = this.point(e);
    const rect = this.canvas.getBoundingClientRect();
    this.zoom(Math.exp(-e.deltaY * 0.01), px - rect.width / 2, py - rect.height / 2);
  }

  /** Zooms by `factor` around a point given relative to the canvas center. */
  zoom(factor: number, cx = 0, cy = 0) {
    this.touched = true;
    const { x, y, k } = this.view;
    const next = Math.min(5, Math.max(0.2, k * factor));
    this.view = { k: next, x: cx - ((cx - x) * next) / k, y: cy - ((cy - y) * next) / k };
    this.draw();
  }

  /** Back to the whole map. */
  reset() {
    this.touched = false;
    this.fit();
    this.draw();
  }
}
