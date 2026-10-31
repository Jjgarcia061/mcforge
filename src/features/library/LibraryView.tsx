import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Badge, Button, Card, Notice, Progress, Spinner, Textarea } from "../../components/ui";
import {
  Channel,
  commands,
  errorMessage,
  unwrap,
  type Answer,
  type Graph,
  type LibraryOverview,
  type LibraryProgress,
  type PullProgress,
  type UpdateSummary,
} from "../../lib/api";
import { useAppData } from "../../lib/data";
import { useSession } from "../../lib/session";
import { LibraryGraph } from "./LibraryGraph";
import { topicColor } from "./graphView";

const EMPTY: Graph = { nodes: [], edges: [], clusters: [] };
const CITATION = /\[(\d+(?:\s*,\s*\d+)*)\]/g;

const loadLibrary = () => Promise.all([unwrap(commands.libraryStatus()), unwrap(commands.libraryGraph())]);

export function LibraryView() {
  const { t, i18n } = useTranslation();
  const { catalog } = useAppData();
  const engine = useSession((s) => s.setup?.engine);
  const language = i18n.resolvedLanguage ?? "en";
  const [overview, setOverview] = useState<LibraryOverview | null>(null);
  const [graph, setGraph] = useState<Graph>(EMPTY);
  const [progress, setProgress] = useState<LibraryProgress | null>(null);
  const [summary, setSummary] = useState<UpdateSummary | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  // undefined: not downloading; null: downloading, size unknown yet.
  const [download, setDownload] = useState<number | null | undefined>(undefined);
  const [question, setQuestion] = useState("");
  const [answer, setAnswer] = useState<Answer | null>(null);
  const [asking, setAsking] = useState(false);
  const [askError, setAskError] = useState<string | null>(null);
  // Set by Cancel, so the failure that follows is not shown as an error.
  const cancelling = useRef(false);

  const status = overview?.status;
  const folders = status?.folders ?? [];
  const updating = progress !== null;
  const searchModel = catalog.models.embedding[0];

  async function refresh() {
    const [o, g] = await loadLibrary();
    setOverview(o);
    setGraph(g);
    setSelected(null);
  }

  useEffect(() => {
    loadLibrary().then(
      ([o, g]) => {
        setOverview(o);
        setGraph(g);
      },
      (e: unknown) => setError(errorMessage(e)),
    );
  }, []);

  async function setFolders(list: string[]) {
    try {
      setOverview(await unwrap(commands.setLibraryFolders(list)));
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function addFolders() {
    const picked = await open({ directory: true, multiple: true });
    if (picked) await setFolders([...folders, ...(Array.isArray(picked) ? picked : [picked])]);
  }

  async function update() {
    let finished = false;
    const channel = new Channel<LibraryProgress>();
    channel.onmessage = (p) => !finished && setProgress(p);
    cancelling.current = false;
    setSummary(null);
    setError(null);
    setProgress({ phase: "scanning" });
    try {
      setSummary(await unwrap(commands.updateLibrary(channel)));
    } catch (e) {
      if (!cancelling.current) setError(errorMessage(e));
    } finally {
      finished = true;
      setProgress(null);
      await refresh().catch(() => {});
    }
  }

  function cancel() {
    cancelling.current = true;
    void commands.cancelLibraryUpdate();
  }

  async function downloadModel() {
    if (!searchModel) return;
    const channel = new Channel<PullProgress>();
    channel.onmessage = (p) => setDownload(p.total && p.completed ? p.completed / p.total : null);
    setDownload(null);
    try {
      await unwrap(commands.pullModel(searchModel.id, channel));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setDownload(undefined);
      await refresh().catch(() => {});
    }
  }

  async function ask() {
    setAsking(true);
    setAnswer(null);
    setAskError(null);
    try {
      setAnswer(await unwrap(commands.askLibrary(question.trim(), language)));
    } catch (e) {
      setAskError(errorMessage(e));
    } finally {
      setAsking(false);
    }
  }

  async function openFile(path: string, reveal: boolean) {
    try {
      await unwrap(commands.openFile(path, reveal));
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  /** Selects a file on the map, as when a citation is clicked. */
  function show(path: string) {
    const i = graph.nodes.findIndex((n) => n.path === path);
    setSelected(i >= 0 ? i : null);
  }

  const pending = status?.pending;
  const changes = pending ? pending.added + pending.changed + pending.removed : 0;
  const unembedded = overview?.searchModel === "ready" ? (pending?.unembedded ?? 0) : 0;
  const needsUpdate = folders.length > 0 && (changes > 0 || unembedded > 0 || !status?.updated);
  const date = new Intl.DateTimeFormat(language, { dateStyle: "medium", timeStyle: "short" });

  return (
    <div className="mx-auto max-w-5xl space-y-5">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">{t("library.title")}</h1>
        <p className="mt-1 text-zinc-500">{t("library.subtitle")}</p>
      </div>

      {error && <Notice tone="bad">{error}</Notice>}

      <Card>
        <h2 className="mb-3 font-medium">{t("library.folders")}</h2>
        {folders.length === 0 ? (
          <p className="mb-3 text-sm text-zinc-500">{t("library.foldersHint")}</p>
        ) : (
          <ul className="mb-3 space-y-1.5">
            {folders.map((f) => (
              <li key={f} className="flex items-center gap-2 text-sm">
                <span className="min-w-0 flex-1 truncate font-mono text-xs" title={f}>
                  {f}
                </span>
                <Button
                  variant="ghost"
                  className="px-2 py-1 text-xs"
                  disabled={updating}
                  onClick={() => void setFolders(folders.filter((x) => x !== f))}
                >
                  {t("connections.remove")}
                </Button>
              </li>
            ))}
          </ul>
        )}
        <Button variant={folders.length ? "secondary" : "primary"} disabled={updating} onClick={() => void addFolders()}>
          {t("connections.addFolder")}
        </Button>
      </Card>

      {folders.length > 0 && status && (
        <Card>
          <div className="flex flex-wrap items-center gap-3">
            <div className="min-w-0 flex-1 text-sm">
              <div className="font-medium">
                {t("library.stats", { count: status.files, chunks: status.chunks.toLocaleString(language) })}
                {status.failed > 0 && (
                  <span className="font-normal text-zinc-500"> · {t("library.failed", { count: status.failed })}</span>
                )}
              </div>
              <div className="text-xs text-zinc-500">
                {status.updated
                  ? t("library.updated", { date: date.format(new Date(status.updated)) })
                  : t("library.neverUpdated")}
                {changes > 0 &&
                  ` · ${t("library.pending", { added: pending?.added, changed: pending?.changed, removed: pending?.removed })}`}
                {unembedded > 0 && ` · ${t("library.unembedded", { count: unembedded })}`}
              </div>
            </div>
            {updating ? (
              <Button variant="secondary" onClick={cancel}>
                {t("reports.cancel")}
              </Button>
            ) : (
              <Button variant={needsUpdate ? "primary" : "secondary"} onClick={() => void update()}>
                {t("library.update")}
              </Button>
            )}
          </div>

          {progress && (
            <div className="mt-4 space-y-1.5">
              <div className="flex justify-between gap-3 text-xs text-zinc-500">
                <span className="truncate">
                  {t(`library.phase.${progress.phase}`)}
                  {progress.phase === "reading" && ` · ${progress.file}`}
                </span>
                {(progress.phase === "reading" || progress.phase === "embedding") && (
                  <span className="shrink-0 tabular-nums">
                    {progress.done} / {progress.total}
                  </span>
                )}
              </div>
              <Progress
                value={
                  (progress.phase === "reading" || progress.phase === "embedding") && progress.total > 0
                    ? progress.done / progress.total
                    : null
                }
              />
            </div>
          )}

          {summary && (
            <div className="mt-4 space-y-2">
              <Notice tone="good">
                {t("library.done", { added: summary.added, changed: summary.changed, removed: summary.removed })}
              </Notice>
              {summary.failed.length > 0 && (
                <details className="text-xs text-zinc-500">
                  <summary className="cursor-pointer">{t("library.failedFiles", { count: summary.failed.length })}</summary>
                  <ul className="mt-1.5 space-y-1">
                    {summary.failed.map((f) => (
                      <li key={f.path}>
                        <span className="font-mono">{f.path.split(/[\\/]/).pop()}</span>: {f.error}
                      </li>
                    ))}
                  </ul>
                </details>
              )}
              {summary.embedError && <Notice tone="warn">{t("library.embedError", { detail: summary.embedError })}</Notice>}
            </div>
          )}

          <div className="mt-4 border-t border-zinc-100 pt-3 text-sm dark:border-zinc-800">
            {overview?.searchModel === "ready" ? (
              <Badge tone="good">{t("library.model.ready")}</Badge>
            ) : overview?.searchModel === "missing" ? (
              <div className="space-y-2">
                <p className="text-zinc-600 dark:text-zinc-400">
                  {t("library.model.missing", { name: searchModel?.name, size: searchModel?.sizeGb?.toFixed(1) })}
                </p>
                {download !== undefined ? (
                  <Progress value={download} />
                ) : (
                  <Button variant="secondary" onClick={() => void downloadModel()}>
                    {t("library.model.download")}
                  </Button>
                )}
              </div>
            ) : (
              <p className="text-zinc-500">{t("library.model.offline")}</p>
            )}
          </div>
        </Card>
      )}

      {graph.nodes.length > 0 && (
        <div className="space-y-3">
          <LibraryGraph graph={graph} selected={selected} onSelect={setSelected} />
          {selected !== null && graph.nodes[selected] && (
            <NodeDetails graph={graph} node={selected} onSelect={setSelected} onOpen={(p, r) => void openFile(p, r)} />
          )}
        </div>
      )}

      {status && status.files > 0 && (
        <Card>
          <h2 className="mb-3 font-medium">{t("library.ask")}</h2>
          <Textarea
            rows={2}
            value={question}
            disabled={asking}
            placeholder={t("library.askPlaceholder")}
            onChange={(e) => setQuestion(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && question.trim()) void ask();
            }}
          />
          <div className="mt-3 flex flex-wrap items-center gap-3">
            <Button disabled={!engine || asking || !question.trim()} onClick={() => void ask()}>
              {asking && <Spinner />}
              {t(asking ? "library.asking" : "library.askButton")}
            </Button>
            {(!engine || engine.kind === "api") && (
              <span className="text-xs text-zinc-500">
                {engine
                  ? t("library.apiNote", { provider: engine.provider === "anthropic" ? "Anthropic" : "OpenAI" })
                  : t("library.noEngine")}
              </span>
            )}
          </div>
          {askError && (
            <div className="mt-3">
              <Notice tone="bad">{askError}</Notice>
            </div>
          )}
          {answer && <AnswerView answer={answer} onShow={show} onOpen={(p, r) => void openFile(p, r)} />}
        </Card>
      )}
    </div>
  );
}

/** The selected file: its topic, the files it is joined to, and its actions. */
function NodeDetails({
  graph,
  node,
  onSelect,
  onOpen,
}: {
  graph: Graph;
  node: number;
  onSelect: (node: number) => void;
  onOpen: (path: string, reveal: boolean) => void;
}) {
  const { t } = useTranslation();
  const n = graph.nodes[node];
  if (!n) return null;
  const related = graph.edges
    .filter((e) => e.source === node || e.target === node)
    .map((e) => ({ other: e.source === node ? e.target : e.source, link: e.kind === "link", weight: e.weight ?? 0 }))
    .sort((a, b) => Number(b.link) - Number(a.link) || b.weight - a.weight);
  const topic = graph.clusters[n.cluster]?.label;
  return (
    <Card>
      <div className="flex flex-wrap items-start gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2 font-medium">
            <span className="size-2.5 shrink-0 rounded-full" style={{ background: topicColor(graph, n.cluster) }} />
            <span className="truncate">{n.title}</span>
            <Badge>{n.kind}</Badge>
          </div>
          <div className="mt-0.5 truncate font-mono text-xs text-zinc-500" title={n.path}>
            {n.path}
          </div>
          {topic && <div className="mt-1 text-xs text-zinc-500">{t("library.topic", { topic })}</div>}
        </div>
        <Button variant="secondary" onClick={() => onOpen(n.path, false)}>
          {t("library.open")}
        </Button>
        <Button variant="ghost" onClick={() => onOpen(n.path, true)}>
          {t("reports.viewer.folder")}
        </Button>
      </div>
      {related.length > 0 && (
        <div className="mt-3 flex flex-wrap gap-1.5">
          <span className="text-xs text-zinc-500">{t("library.related")}</span>
          {related.map((r) => (
            <button
              key={r.other}
              type="button"
              onClick={() => onSelect(r.other)}
              className="rounded-full border border-zinc-200 px-2 py-0.5 text-xs hover:border-orange-400 dark:border-zinc-700"
            >
              {r.link && "↔ "}
              {graph.nodes[r.other]?.title}
            </button>
          ))}
        </div>
      )}
    </Card>
  );
}

/** An answer with its `[n]` citations as buttons, and the passages they point to. */
function AnswerView({
  answer,
  onShow,
  onOpen,
}: {
  answer: Answer;
  onShow: (path: string) => void;
  onOpen: (path: string, reveal: boolean) => void;
}) {
  const { t } = useTranslation();
  if (!answer.text) return <p className="mt-4 text-sm text-zinc-500">{t("library.nothingFound")}</p>;
  const sources = new Map(answer.sources.map((s) => [s.n, s]));
  const parts: (string | number)[] = [];
  let last = 0;
  for (const m of answer.text.matchAll(CITATION)) {
    parts.push(answer.text.slice(last, m.index));
    for (const n of (m[1] ?? "").split(",").map((x) => Number(x.trim()))) {
      // A number the model made up stays out of the text.
      if (sources.has(n)) parts.push(n);
    }
    last = (m.index ?? 0) + m[0].length;
  }
  parts.push(answer.text.slice(last));

  return (
    <div className="mt-4 space-y-4">
      <p className="text-sm leading-relaxed whitespace-pre-wrap">
        {parts.map((p, i) =>
          typeof p === "number" ? (
            <button
              key={i}
              type="button"
              onClick={() => onShow(sources.get(p)?.path ?? "")}
              className="mx-0.5 rounded bg-orange-100 px-1 align-baseline text-xs font-medium text-orange-800 hover:bg-orange-200 dark:bg-orange-950 dark:text-orange-300"
              title={sources.get(p)?.title}
            >
              {p}
            </button>
          ) : (
            p
          ),
        )}
      </p>
      {answer.sources.length > 0 && (
        <ol className="space-y-2">
          {answer.sources.map((s) => (
            <li key={s.n} className="rounded-lg border border-zinc-200 p-3 text-sm dark:border-zinc-800">
              <div className="flex items-center gap-2">
                <span className="rounded bg-orange-100 px-1 text-xs font-medium text-orange-800 dark:bg-orange-950 dark:text-orange-300">
                  {s.n}
                </span>
                <button type="button" className="min-w-0 flex-1 truncate text-left font-medium hover:underline" onClick={() => onShow(s.path)}>
                  {s.title}
                </button>
                <Button variant="ghost" className="px-2 py-0.5 text-xs" onClick={() => onOpen(s.path, false)}>
                  {t("library.open")}
                </Button>
              </div>
              <p className="mt-1.5 text-xs text-zinc-500">{s.excerpt}</p>
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}
