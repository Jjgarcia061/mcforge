import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/ui";
import { useAppData } from "../../lib/data";
import { modelName } from "../../lib/engine";
import { useSession } from "../../lib/session";
import { ChatView } from "../chat/ChatView";
import { LibraryView } from "../library/LibraryView";
import { ReportsView } from "../reports/ReportsView";
import { fromSetup, useWizard } from "../setup-wizard/store";

const VIEWS = ["chat", "reports", "library"] as const;
type View = (typeof VIEWS)[number];

export function Workspace() {
  const { t } = useTranslation();
  const { catalog } = useAppData();
  const setup = useSession((s) => s.setup);
  const [view, setView] = useState<View>("chat");

  function editSetup() {
    if (!setup) return;
    useWizard.setState(fromSetup(setup));
    useSession.setState({ editing: true });
  }

  return (
    <div className="flex h-screen">
      <aside className="flex w-64 shrink-0 flex-col border-r border-zinc-200 bg-white p-6 dark:border-zinc-800 dark:bg-zinc-900">
        <div className="mb-8">
          <div className="text-xl font-bold tracking-tight">
            MC<span className="text-orange-600">Forge</span>
          </div>
          <div className="text-xs text-zinc-500">{t("app.tagline")}</div>
        </div>
        <nav className="space-y-1 text-sm">
          {VIEWS.map((v) => (
            <button
              key={v}
              type="button"
              onClick={() => setView(v)}
              className={
                v === view
                  ? "block w-full rounded-lg bg-orange-50 px-3 py-2 text-left font-medium text-orange-700 dark:bg-orange-950/50 dark:text-orange-300"
                  : "block w-full rounded-lg px-3 py-2 text-left text-zinc-600 hover:bg-zinc-100 dark:text-zinc-300 dark:hover:bg-zinc-800"
              }
            >
              {t(`workspace.${v}`)}
            </button>
          ))}
        </nav>
        {setup && (
          <div className="mt-auto space-y-3 border-t border-zinc-200 pt-4 text-sm dark:border-zinc-800">
            <div>
              <div className="text-xs text-zinc-500">{t("workspace.engine")}</div>
              <div className="truncate font-medium">{modelName(catalog, setup.engine)}</div>
            </div>
            <Button variant="secondary" className="w-full" onClick={editSetup}>
              {t("workspace.editSetup")}
            </Button>
          </div>
        )}
      </aside>
      {/* Each view sets its own width: forms stay narrow, the report viewer uses the whole area. */}
      <main className="min-w-0 flex-1 overflow-y-auto px-10 py-8">
        {view === "chat" ? <ChatView /> : view === "reports" ? <ReportsView /> : <LibraryView />}
      </main>
    </div>
  );
}
