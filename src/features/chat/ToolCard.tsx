import { useTranslation } from "react-i18next";
import { Badge, Button, Spinner } from "../../components/ui";
import type { Approval, ToolView } from "../../lib/api";

export type ReportLink = { path: string; title: string };

const KNOWN = [
  "library_search",
  "database_list_tables",
  "database_describe_table",
  "database_query",
  "database_execute",
  "reports_list_templates",
  "reports_render_report",
];

/** The part of a tool's input worth showing: the SQL, the search, the file. */
function inputPreview(name: string, input: unknown): { code?: string; text?: string } {
  const value = (input ?? {}) as Record<string, unknown>;
  const str = (key: string) => (typeof value[key] === "string" ? (value[key] as string) : undefined);
  if (name.startsWith("database_") && str("sql")) return { code: str("sql") };
  if (name === "library_search") return { text: str("query") && `«${str("query")}»` };
  if (name === "database_describe_table") return { text: str("table") };
  if (name === "reports_render_report") return { text: [str("file"), str("question")].filter(Boolean).join(" · ") };
  const json = JSON.stringify(value);
  return json === "{}" ? {} : { code: json };
}

/**
 * One tool use in a conversation: running, waiting for the user's permission,
 * or done with what it found.
 */
export function ToolCard({
  name,
  input,
  view,
  waiting,
  onAnswer,
  onOpen,
  onReport,
}: {
  name: string;
  input: unknown;
  view?: ToolView;
  waiting?: boolean;
  onAnswer: (approval: Approval) => void;
  onOpen: (path: string) => void;
  onReport: (report: ReportLink) => void;
}) {
  const { t } = useTranslation();
  const sources = view?.sources ?? [];
  const table = view?.table;
  const label = KNOWN.includes(name) ? t(`chat.tool.${name}`) : name;
  const preview = inputPreview(name, input);
  const s = view?.summary;
  const summary = !s
    ? null
    : s.kind === "error"
      ? s.message
      : s.kind === "skipped"
        ? null
        : t(`chat.summary.${s.kind}`, { ...s, count: "count" in s ? s.count : "rows" in s ? s.rows : s.sections });

  return (
    <div className="my-2 rounded-lg border border-zinc-200 bg-zinc-50 text-sm dark:border-zinc-800 dark:bg-zinc-900/60">
      <div className="flex flex-wrap items-center gap-2 px-3 py-2">
        {!view && !waiting && <Spinner />}
        <span className="font-medium">{label}</span>
        {preview.text && <span className="min-w-0 truncate text-zinc-500">{preview.text}</span>}
        {view?.decision === "blocked" && <Badge tone="bad">{t("chat.decision.blocked")}</Badge>}
        {view?.decision === "rejected" && <Badge tone="warn">{t("chat.decision.rejected")}</Badge>}
        {view?.decision === "approved" && <Badge tone="good">{t("chat.decision.approved")}</Badge>}
        {summary && (
          <span className={s?.kind === "error" ? "text-red-600 dark:text-red-400" : "text-zinc-500"}>· {summary}</span>
        )}
      </div>

      {waiting && (
        <div className="space-y-2 border-t border-zinc-200 px-3 py-2.5 dark:border-zinc-800">
          <div className="text-xs text-zinc-600 dark:text-zinc-400">{t("chat.approval.ask")}</div>
          {preview.code && <Code text={preview.code} />}
          <div className="flex flex-wrap gap-2">
            <Button className="px-3 py-1.5" onClick={() => onAnswer("once")}>
              {t("chat.approval.once")}
            </Button>
            <Button variant="secondary" className="px-3 py-1.5" onClick={() => onAnswer("always")}>
              {t("chat.approval.always")}
            </Button>
            <Button variant="ghost" className="px-3 py-1.5" onClick={() => onAnswer("reject")}>
              {t("chat.approval.reject")}
            </Button>
          </div>
        </div>
      )}

      {view?.report && (
        <div className="border-t border-zinc-200 px-3 py-2.5 dark:border-zinc-800">
          <Button className="px-3 py-1.5" onClick={() => view.report && onReport(view.report)}>
            {t("chat.viewReport", { title: view.report.title })}
          </Button>
        </div>
      )}

      {view && !waiting && (preview.code || table || sources.length > 0) && (
        <details className="border-t border-zinc-200 px-3 py-2 dark:border-zinc-800">
          <summary className="cursor-pointer text-xs text-zinc-500">{t("chat.details")}</summary>
          <div className="mt-2 space-y-2">
            {preview.code && <Code text={preview.code} />}
            {sources.length > 0 && (
              <ul className="space-y-1">
                {sources.map((src) => (
                  <li key={src.path}>
                    <button type="button" className="text-left text-xs underline" onClick={() => onOpen(src.path)}>
                      {src.title}
                    </button>
                  </li>
                ))}
              </ul>
            )}
            {table && table.rows.length > 0 && (
              <div className="max-h-64 overflow-auto">
                <table className="min-w-full border-collapse text-xs">
                  <thead>
                    <tr>
                      {table.columns.map((c) => (
                        <th key={c} className="sticky top-0 border-b border-zinc-200 bg-zinc-50 px-2 py-1 text-left font-medium dark:border-zinc-700 dark:bg-zinc-900">
                          {c}
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {table.rows.map((row, i) => (
                      <tr key={i}>
                        {row.map((cell, j) => (
                          <td key={j} className="border-b border-zinc-100 px-2 py-1 dark:border-zinc-800">
                            {cell}
                          </td>
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        </details>
      )}
    </div>
  );
}

function Code({ text }: { text: string }) {
  return (
    <pre className="overflow-x-auto rounded-md bg-zinc-100 p-2 font-mono text-xs whitespace-pre-wrap dark:bg-zinc-800">
      {text}
    </pre>
  );
}
