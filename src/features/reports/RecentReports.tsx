import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button, Card } from "../../components/ui";
import type { ReportEntry } from "../../lib/api";
import type { ViewedReport } from "./ReportViewer";

const SHOWN = 8;

/** Reports generated earlier on this computer, newest first. */
export function RecentReports({
  reports,
  onView,
  onReveal,
}: {
  reports: ReportEntry[];
  onView: (report: ViewedReport) => void;
  onReveal: (path: string) => void;
}) {
  const { t, i18n } = useTranslation();
  const [all, setAll] = useState(false);
  if (reports.length === 0) return null;
  const date = new Intl.DateTimeFormat(i18n.resolvedLanguage, { dateStyle: "medium", timeStyle: "short" });
  const shown = all ? reports : reports.slice(0, SHOWN);

  return (
    <Card>
      <h2 className="mb-2 font-medium">{t("reports.recent.title")}</h2>
      <ul className="divide-y divide-zinc-100 dark:divide-zinc-800">
        {shown.map((r) => (
          <li key={r.path} className="flex items-center gap-2 py-2.5">
            <div className="min-w-0 flex-1">
              <div className="truncate text-sm font-medium" title={r.title}>
                {r.title}
              </div>
              <div className="truncate text-xs text-zinc-500">
                {r.source} · {date.format(new Date(r.created))} · {t("reports.sections", { count: r.sections })}
              </div>
            </div>
            <Button variant="secondary" className="px-3 py-1.5" onClick={() => onView(r)}>
              {t("reports.recent.open")}
            </Button>
            <Button variant="ghost" className="px-3 py-1.5" onClick={() => onReveal(r.path)}>
              {t("reports.recent.reveal")}
            </Button>
          </li>
        ))}
      </ul>
      {reports.length > SHOWN && (
        <Button variant="ghost" className="mt-1 px-3 py-1.5" onClick={() => setAll(!all)}>
          {all ? t("reports.recent.fewer") : t("reports.recent.all", { count: reports.length })}
        </Button>
      )}
    </Card>
  );
}
