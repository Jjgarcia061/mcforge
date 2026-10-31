import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Badge, Button, Card, Label, Notice, Select, Spinner, Textarea } from "../../components/ui";
import {
  Channel,
  commands,
  errorMessage,
  unwrap,
  type NumberFormat,
  type ReportEntry,
  type ReportOutcome,
  type ReportStage,
  type ReportWarning,
  type SourceSummary,
  type TemplateInfo,
} from "../../lib/api";
import { useAppData } from "../../lib/data";
import { modelName } from "../../lib/engine";
import { useSession } from "../../lib/session";
import { RecentReports } from "./RecentReports";

const STAGES: ReportStage[] = ["reading", "planning", "querying", "writing", "rendering"];
const AI_STAGES: ReportStage[] = ["planning", "writing"];
const SHOWN_COLUMNS = 14;
const DELIMITERS: Record<string, string> = { ",": "comma", ";": "semicolon", tab: "tab", "|": "pipe" };
const EXTENSIONS = ["csv", "tsv", "txt", "xlsx", "xlsm", "xlsb", "xls", "ods"];

/** The separators this system writes numbers with, so the report reads naturally. */
function numberFormat(locale?: string): NumberFormat {
  const parts = new Intl.NumberFormat(locale).formatToParts(1234567.5);
  return {
    decimal: parts.find((p) => p.type === "decimal")?.value ?? ".",
    group: parts.find((p) => p.type === "group")?.value ?? ",",
  };
}

type Run = { stage: ReportStage } | { outcome: ReportOutcome } | { error: string };

export function ReportsView() {
  const { t, i18n } = useTranslation();
  const { catalog } = useAppData();
  const engine = useSession((s) => s.setup?.engine);
  const language = i18n.resolvedLanguage ?? "en";
  const [templates, setTemplates] = useState<TemplateInfo[]>([]);
  const [template, setTemplate] = useState<string | null>(null);
  const [source, setSource] = useState<{ path: string; summary: SourceSummary } | null>(null);
  const [reading, setReading] = useState(false);
  const [sourceError, setSourceError] = useState<string | null>(null);
  const [question, setQuestion] = useState("");
  const [useAi, setUseAi] = useState(true);
  const [run, setRun] = useState<Run | null>(null);
  const [history, setHistory] = useState<ReportEntry[]>([]);

  const ai = useAi && engine !== undefined;
  const busy = run !== null && "stage" in run;
  const progress = useRef<HTMLDivElement>(null);
  const runKind = run && Object.keys(run)[0];

  // Keep the progress and the result in view: they appear below the form.
  useEffect(() => {
    if (runKind) progress.current?.scrollIntoView({ behavior: "smooth", block: "nearest" });
  }, [runKind]);

  useEffect(() => {
    void commands.listReports().then(setHistory);
  }, []);

  useEffect(() => {
    let live = true;
    void commands.listTemplates(language).then((list) => {
      if (!live) return;
      setTemplates(list);
      setTemplate((current) => current ?? list[0]?.id ?? null);
    });
    return () => {
      live = false;
    };
  }, [language]);

  async function chooseFile() {
    const path = await open({
      multiple: false,
      directory: false,
      filters: [{ name: t("reports.fileTypes"), extensions: EXTENSIONS }],
    });
    if (path) await inspect(path, null);
  }

  /** Reads `path`; for a workbook, `sheet` or else its first sheet with data. */
  async function inspect(path: string, sheet: string | null) {
    setReading(true);
    setSourceError(null);
    setRun(null);
    try {
      setSource({ path, summary: await unwrap(commands.inspectSource(path, sheet)) });
    } catch (e) {
      // A sheet that fails to read leaves the file and its other sheets selectable.
      if (sheet === null) setSource(null);
      setSourceError(errorMessage(e));
    } finally {
      setReading(false);
    }
  }

  async function generate() {
    if (!source || !template) return;
    // Stage messages can trail the final answer; once it is in, they are stale.
    let finished = false;
    const channel = new Channel<ReportStage>();
    channel.onmessage = (stage) => !finished && setRun({ stage });
    setRun({ stage: "reading" });
    try {
      const outcome = await unwrap(
        commands.generateReport(
          {
            file: source.path,
            sheet: source.summary.format.kind === "spreadsheet" ? source.summary.format.sheet : null,
            template,
            question: question.trim() || null,
            useAi: ai,
            language,
            numberFormat: numberFormat(),
          },
          channel,
        ),
      );
      finished = true;
      setRun({ outcome });
      void openReport(outcome.path, false);
      void commands.listReports().then(setHistory);
    } catch (e) {
      finished = true;
      setRun({ error: errorMessage(e) });
    }
  }

  async function openReport(path: string, reveal: boolean) {
    try {
      await unwrap(commands.openReport(path, reveal));
    } catch (e) {
      setRun({ error: errorMessage(e) });
    }
  }

  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">{t("reports.title")}</h1>
        <p className="mt-1 text-zinc-500">{t("reports.subtitle")}</p>
      </div>

      <Card>
        <h2 className="mb-3 font-medium">{t("reports.data")}</h2>
        {source ? (
          <SourcePreview
            summary={source.summary}
            disabled={reading || busy}
            onSheet={(sheet) => void inspect(source.path, sheet)}
          />
        ) : (
          <p className="mb-3 text-sm text-zinc-500">{t("reports.dataHint")}</p>
        )}
        {sourceError && (
          <div className="mb-3">
            <Notice tone="bad">{sourceError}</Notice>
          </div>
        )}
        <Button variant={source ? "secondary" : "primary"} disabled={reading || busy} onClick={() => void chooseFile()}>
          {reading && <Spinner />}
          {t(reading ? "reports.reading" : source ? "reports.changeFile" : "reports.chooseFile")}
        </Button>
      </Card>

      {templates.length > 1 && (
        <Card>
          <h2 className="mb-3 font-medium">{t("reports.template")}</h2>
          <div className="grid gap-2 sm:grid-cols-2">
            {templates.map((tpl) => (
              <Card key={tpl.id} selected={tpl.id === template} onClick={() => setTemplate(tpl.id)}>
                <div className="font-medium">{tpl.name}</div>
                <div className="mt-1 text-xs text-zinc-500">{tpl.description}</div>
              </Card>
            ))}
          </div>
        </Card>
      )}

      <Card>
        <label className="block">
          <Label hint={t("reports.optional")}>{t("reports.question")}</Label>
          <Textarea
            rows={3}
            value={question}
            disabled={busy}
            placeholder={t("reports.questionPlaceholder")}
            onChange={(e) => setQuestion(e.target.value)}
          />
        </label>
        <div className="mt-4 text-sm">
          {engine ? (
            <label className="flex items-start gap-2.5">
              <input
                type="checkbox"
                className="mt-0.5 size-4 accent-orange-600"
                checked={useAi}
                disabled={busy}
                onChange={(e) => setUseAi(e.target.checked)}
              />
              <span>
                {t("reports.useAi", { model: modelName(catalog, engine) })}
                {engine.kind === "local" && <span className="block text-xs text-zinc-500">{t("reports.localHint")}</span>}
              </span>
            </label>
          ) : (
            <p className="text-zinc-500">{t("reports.noEngine")}</p>
          )}
        </div>
      </Card>

      <div className="flex items-center gap-3">
        <Button disabled={!source || !template || busy} onClick={() => void generate()}>
          {busy && <Spinner />}
          {t(busy ? "reports.generating" : "reports.generate")}
        </Button>
        {templates.length === 1 && templates[0] && (
          <span className="text-xs text-zinc-500">{t("reports.usingTemplate", { name: templates[0].name })}</span>
        )}
      </div>

      <div ref={progress} className="scroll-mb-8">
        {run && "stage" in run && <Stages current={run.stage} ai={ai} />}
        {run && "error" in run && (
          <Notice tone="bad">
            <div className="font-medium">{t("reports.failed")}</div>
            <div className="mt-1">{run.error}</div>
          </Notice>
        )}
        {run && "outcome" in run && (
          <Result outcome={run.outcome} onOpen={(reveal) => void openReport(run.outcome.path, reveal)} />
        )}
      </div>

      <RecentReports reports={history} onOpen={(path, reveal) => void openReport(path, reveal)} />
    </div>
  );
}

function SourcePreview({
  summary,
  disabled,
  onSheet,
}: {
  summary: SourceSummary;
  disabled: boolean;
  onSheet: (sheet: string) => void;
}) {
  const { t } = useTranslation();
  const hidden = summary.columns.length - SHOWN_COLUMNS;
  const format = summary.format;
  const size = t("reports.fileInfo", { rows: summary.rows.toLocaleString(), columns: summary.columns.length });
  return (
    <div className="mb-4">
      <div className="font-mono text-sm">{summary.file}</div>
      <div className="mt-0.5 text-xs text-zinc-500">
        {size} ·{" "}
        {format.kind === "csv"
          ? t("reports.csvInfo", {
              delimiter: t(`reports.delimiter.${DELIMITERS[format.delimiter] ?? "comma"}`),
              encoding: format.encoding,
            })
          : t("reports.sheetInfo", { sheet: format.sheet })}
      </div>
      {format.kind === "spreadsheet" && format.sheets.length > 1 && (
        <label className="mt-3 flex max-w-sm items-center gap-3 text-sm">
          <span className="shrink-0 text-zinc-500">{t("reports.sheet")}</span>
          <Select value={format.sheet} disabled={disabled} onChange={(e) => onSheet(e.target.value)}>
            {format.sheets.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </Select>
        </label>
      )}
      <div className="mt-3 flex flex-wrap gap-1.5">
        {summary.columns.slice(0, SHOWN_COLUMNS).map((c) => (
          <Badge key={c.name}>
            {c.label}
            <span className="ml-1 font-normal text-zinc-500">{t(`reports.kind.${c.kind}`)}</span>
          </Badge>
        ))}
        {hidden > 0 && <Badge>{t("reports.moreColumns", { count: hidden })}</Badge>}
      </div>
    </div>
  );
}

function Stages({ current, ai }: { current: ReportStage; ai: boolean }) {
  const { t } = useTranslation();
  const stages = STAGES.filter((s) => ai || !AI_STAGES.includes(s));
  const index = stages.indexOf(current);
  return (
    <Card>
      <ol className="space-y-2.5 text-sm">
        {stages.map((s, i) => (
          <li
            key={s}
            className={`flex items-center gap-3 ${i > index ? "text-zinc-400" : i === index ? "font-medium" : "text-zinc-500"}`}
          >
            <span className="grid size-5 place-items-center text-xs text-orange-600">
              {i < index ? "✓" : i === index ? <Spinner /> : "·"}
            </span>
            {t(`reports.stage.${s}`)}
          </li>
        ))}
      </ol>
    </Card>
  );
}

function Result({ outcome, onOpen }: { outcome: ReportOutcome; onOpen: (reveal: boolean) => void }) {
  const { t } = useTranslation();
  return (
    <div className="space-y-3">
      <Notice tone="good">
        <div className="font-medium">{t("reports.done", { title: outcome.title })}</div>
        <div className="mt-0.5 text-xs">
          {t("reports.sections", { count: outcome.sections })} ·{" "}
          {outcome.engine ? t("reports.by", { engine: outcome.engine }) : t("reports.auto")}
        </div>
        <div className="mt-3 flex gap-2">
          <Button onClick={() => onOpen(false)}>{t("reports.open")}</Button>
          <Button variant="secondary" onClick={() => onOpen(true)}>
            {t("reports.reveal")}
          </Button>
        </div>
      </Notice>
      {outcome.warnings.map((w, i) => (
        <Notice key={i} tone="warn">
          {warningText(t, w)}
        </Notice>
      ))}
    </div>
  );
}

function warningText(t: (key: string, options?: Record<string, unknown>) => string, w: ReportWarning): string {
  switch (w.kind) {
    case "planFailed":
      return t("reports.warning.planFailed", { detail: w.detail });
    case "queriesDropped":
      return t("reports.warning.queriesDropped", { count: w.detail });
    case "narrativeFailed":
      return t("reports.warning.narrativeFailed", { detail: w.detail });
  }
}
