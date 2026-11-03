import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Badge, Card, Segmented } from "../../../components/ui";
import type { Connector, Language, Priority } from "../../../lib/api";
import { useAppData } from "../../../lib/data";
import { withViewTransition } from "../../../lib/transition";
import { suggestedConnectors, useWizard } from "../store";

const LANGUAGES: Language[] = ["spanish", "english", "both"];
const PRIORITIES: Priority[] = ["fast", "balanced", "quality"];

/** The setup interview: what the user wants to do, in which language, and fast or thorough. */
export function UseCaseStep() {
  const { t } = useTranslation();
  const { catalog } = useAppData();
  const { needs, connectors, toggleUseCase, setLanguage, setPriority, toggleConnector } = useWizard();
  const [showMore, setShowMore] = useState(false);
  const { suggested, others } = suggestedConnectors(catalog, needs.useCases);
  const extrasChosen = others.filter((c) => connectors.includes(c.id)).length;

  const row = (c: Connector) => (
    <ConnectorRow
      key={c.id}
      connector={c}
      checked={connectors.includes(c.id)}
      onToggle={() => toggleConnector(c.id)}
    />
  );

  return (
    <div className="space-y-8">
      <section className="space-y-3">
        <div>
          <h2 className="font-medium">{t("useCase.tasks")}</h2>
          <p className="text-sm text-zinc-500">{t("useCase.tasksHint")}</p>
        </div>
        <div className="grid grid-cols-2 gap-3 lg:grid-cols-3">
          {catalog.useCases.map((u) => (
            <Card
              key={u.id}
              selected={needs.useCases.includes(u.id)}
              onClick={() => withViewTransition(() => toggleUseCase(catalog, u.id))}
            >
              <div className="font-medium">{t(`useCases.${u.id}.title`)}</div>
              <div className="mt-1 text-sm text-zinc-500">{t(`useCases.${u.id}.description`)}</div>
            </Card>
          ))}
        </div>
      </section>

      <section className="grid gap-6 sm:grid-cols-2">
        <div className="space-y-2">
          <h2 className="font-medium">{t("useCase.language")}</h2>
          <Segmented
            size="sm"
            value={needs.language}
            onChange={setLanguage}
            options={LANGUAGES.map((l) => ({ value: l, label: t(`useCase.languages.${l}`) }))}
          />
        </div>
        <div className="space-y-2">
          <h2 className="font-medium">{t("useCase.priority")}</h2>
          <Segmented
            size="sm"
            value={needs.priority}
            onChange={setPriority}
            options={PRIORITIES.map((p) => ({ value: p, label: t(`useCase.priorities.${p}`) }))}
          />
          <p className="text-xs text-zinc-500">{t(`useCase.priorityHints.${needs.priority}`)}</p>
        </div>
      </section>

      <section className="space-y-2">
        {suggested.length > 0 && <h2 className="text-sm font-medium text-zinc-500">{t("useCase.suggested")}</h2>}
        {suggested.map(row)}

        {others.length > 0 && (
          <>
            <button
              type="button"
              style={{ viewTransitionName: "more-connectors" }}
              onClick={() => withViewTransition(() => setShowMore(!showMore))}
              className="flex w-full items-center gap-2 pt-4 text-sm font-medium text-zinc-500 hover:text-zinc-800 dark:hover:text-zinc-200"
            >
              <span className={`inline-block transition-transform ${showMore ? "rotate-90" : ""}`}>›</span>
              {t("useCase.more", { count: others.length })}
              {extrasChosen > 0 && <Badge tone="brand">{t("useCase.extrasChosen", { count: extrasChosen })}</Badge>}
            </button>
            {showMore && others.map(row)}
          </>
        )}
      </section>
    </div>
  );
}

function ConnectorRow({ connector, checked, onToggle }: { connector: Connector; checked: boolean; onToggle: () => void }) {
  const { t } = useTranslation();
  const builtin = connector.server.runtime === "builtin";

  return (
    <label
      style={{ viewTransitionName: `connector-${connector.id}` }}
      className="flex cursor-pointer items-start gap-3 rounded-xl border border-zinc-200 bg-white p-4 dark:border-zinc-800 dark:bg-zinc-900"
    >
      <input type="checkbox" className="mt-1 size-4 accent-orange-600" checked={checked} onChange={onToggle} />
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <span className="font-medium">{connector.name}</span>
          <Badge>{t(`categories.${connector.category}`)}</Badge>
          <Badge tone={builtin ? "brand" : "neutral"}>{t(builtin ? "connector.builtin" : "connector.external")}</Badge>
          {!connector.verified && <Badge tone="warn">{t("connector.unverified")}</Badge>}
        </div>
        <p className="mt-1 text-sm text-zinc-500">{connector.description}</p>
      </div>
    </label>
  );
}
