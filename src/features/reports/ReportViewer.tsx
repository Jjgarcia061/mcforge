import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button, Notice, Spinner } from "../../components/ui";
import { commands, errorMessage, unwrap } from "../../lib/api";

export type ViewedReport = { path: string; title: string };

// The report's own print button needs a script, and none runs in the viewer.
const HIDE_PRINT_BUTTON = "<style>.print{display:none}</style>";

/** A generated report shown inside the app, with the same actions as the file. */
export function ReportViewer({
  report,
  onBack,
  onOpen,
}: {
  report: ViewedReport;
  onBack: () => void;
  onOpen: (path: string, reveal: boolean) => void;
}) {
  const { t } = useTranslation();
  const [html, setHtml] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const frame = useRef<HTMLIFrameElement>(null);

  useEffect(() => {
    let live = true;
    unwrap(commands.readReport(report.path)).then(
      (text) => live && setHtml(text.replace("</head>", `${HIDE_PRINT_BUTTON}</head>`)),
      (e: unknown) => live && setError(errorMessage(e)),
    );
    return () => {
      live = false;
    };
  }, [report.path]);

  return (
    <div className="flex h-[calc(100vh-4rem)] flex-col gap-3">
      <div className="flex items-center gap-2">
        <Button variant="ghost" className="px-3" onClick={onBack}>
          ← {t("reports.viewer.back")}
        </Button>
        <h1 className="min-w-0 flex-1 truncate font-medium" title={report.title}>
          {report.title}
        </h1>
        <Button variant="secondary" disabled={!html} onClick={() => frame.current?.contentWindow?.print()}>
          {t("reports.viewer.print")}
        </Button>
        <Button variant="secondary" onClick={() => onOpen(report.path, false)}>
          {t("reports.viewer.browser")}
        </Button>
        <Button variant="ghost" onClick={() => onOpen(report.path, true)}>
          {t("reports.viewer.folder")}
        </Button>
      </div>
      {error ? (
        <Notice tone="bad">{error}</Notice>
      ) : html === null ? (
        <div className="grid flex-1 place-items-center text-zinc-500">
          <Spinner />
        </div>
      ) : (
        // Same-origin so the toolbar can print it; no scripts run inside.
        <iframe
          ref={frame}
          title={report.title}
          srcDoc={html}
          sandbox="allow-same-origin allow-modals"
          className="w-full flex-1 rounded-xl border border-zinc-200 bg-white dark:border-zinc-800"
        />
      )}
    </div>
  );
}
