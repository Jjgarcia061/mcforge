import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { Graph } from "../../lib/api";
import { GraphView, INK, MUTED, OTHER, SURFACE, TOPIC_COLORS, type Hover } from "./graphView";

/**
 * The library as an animated map: files are dots sized by their connections and
 * colored by topic. Hovering lights up a file's neighbours, a click selects it,
 * dragging moves the view or a file, and the wheel zooms.
 */
export function LibraryGraph({
  graph,
  selected,
  onSelect,
}: {
  graph: Graph;
  selected: number | null;
  onSelect: (node: number | null) => void;
}) {
  const { t } = useTranslation();
  const canvas = useRef<HTMLCanvasElement>(null);
  const view = useRef<GraphView | null>(null);
  const select = useRef(onSelect);
  const [hover, setHover] = useState<Hover>(null);
  const [topic, setTopic] = useState<number | null>(null);

  useEffect(() => {
    select.current = onSelect;
  }, [onSelect]);

  useEffect(() => {
    const el = canvas.current;
    if (!el) return;
    const v = new GraphView(el, setHover, (n) => select.current(n));
    view.current = v;
    // Wheel events must be non-passive to stop the page from scrolling while zooming.
    const wheel = (e: WheelEvent) => v.wheel(e);
    el.addEventListener("wheel", wheel, { passive: false });
    return () => {
      el.removeEventListener("wheel", wheel);
      v.destroy();
      view.current = null;
    };
  }, []);

  useEffect(() => view.current?.setGraph(graph), [graph]);
  useEffect(() => view.current?.setSelected(selected), [selected]);
  useEffect(() => view.current?.setTopic(topic), [topic]);

  const hovered = hover ? graph.nodes[hover.node] : null;
  const named = graph.clusters.map((c, i) => ({ ...c, i })).filter((c) => c.label && c.i < TOPIC_COLORS.length);
  const others = graph.nodes.length - named.reduce((sum, c) => sum + c.size, 0);

  return (
    <div className="relative overflow-hidden rounded-xl" style={{ background: SURFACE }}>
      <canvas
        ref={canvas}
        className="block h-[460px] w-full touch-none"
        role="img"
        aria-label={t("library.graph.aria", { files: graph.nodes.length, topics: named.length })}
        onPointerDown={(e) => view.current?.pointerDown(e.nativeEvent)}
        onPointerMove={(e) => view.current?.pointerMove(e.nativeEvent)}
        onPointerUp={() => view.current?.pointerUp()}
        onPointerLeave={() => view.current?.pointerLeave()}
      />
      {hovered && hover && (
        <div
          className="pointer-events-none absolute max-w-60 rounded-lg px-2.5 py-1.5 text-xs shadow-lg"
          style={{ left: hover.x + 14, top: hover.y + 14, background: "#2c2c2a", color: INK }}
        >
          <div className="font-medium">{hovered.title}</div>
          <div style={{ color: MUTED }}>
            {graph.clusters[hovered.cluster]?.label || t("library.graph.noTopic")} ·{" "}
            {t("library.graph.connections", { count: hovered.degree })}
          </div>
        </div>
      )}
      <div className="absolute top-3 left-3 flex max-w-[75%] flex-wrap gap-1.5 text-xs">
        {named.map((c) => (
          <button
            key={c.i}
            type="button"
            onClick={() => setTopic(topic === c.i ? null : c.i)}
            className="flex items-center gap-1.5 rounded-full px-2 py-0.5"
            style={{
              background: topic === c.i ? "#383835" : "rgba(26,26,25,0.85)",
              color: INK,
              opacity: topic === null || topic === c.i ? 1 : 0.5,
            }}
          >
            <span className="size-2 rounded-full" style={{ background: TOPIC_COLORS[c.i] }} />
            {c.label}
            <span style={{ color: MUTED }}>{c.size}</span>
          </button>
        ))}
        {others > 0 && named.length > 0 && (
          <span className="flex items-center gap-1.5 rounded-full px-2 py-0.5" style={{ color: MUTED }}>
            <span className="size-2 rounded-full" style={{ background: OTHER }} />
            {t("library.graph.others", { count: others })}
          </span>
        )}
      </div>
      <div className="absolute bottom-2 left-3 flex gap-1">
        <MapButton label="+" title={t("library.graph.zoomIn")} onClick={() => view.current?.zoom(1.3)} />
        <MapButton label="−" title={t("library.graph.zoomOut")} onClick={() => view.current?.zoom(1 / 1.3)} />
        <MapButton label="⤢" title={t("library.graph.fit")} onClick={() => view.current?.reset()} />
      </div>
      <div className="pointer-events-none absolute right-3 bottom-2 text-[11px]" style={{ color: MUTED }}>
        {t("library.graph.hint")}
      </div>
    </div>
  );
}

function MapButton({ label, title, onClick }: { label: string; title: string; onClick: () => void }) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      onClick={onClick}
      className="grid size-7 place-items-center rounded-md text-sm hover:bg-[#383835]"
      style={{ background: "rgba(26,26,25,0.85)", color: INK }}
    >
      {label}
    </button>
  );
}
