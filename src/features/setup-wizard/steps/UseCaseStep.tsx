import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Badge, Card } from "../../../components/ui";
import type { Connector } from "../../../lib/api";
import { useAppData } from "../../../lib/data";
import { withViewTransition } from "../../../lib/transition";
import { suggestedConnectors, useWizard } from "../store";

export function UseCaseStep() {
  const { t } = useTranslation();
  const { catalog } = useAppData();
  const { useCase, connectors, chooseUseCase, toggleConnector } = useWizard();
  const [showMore, setShowMore] = useState(false);
  const { suggested, others } = suggestedConnectors(catalog, useCase);
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
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-3">
        {catalog.useCases.map((u) => (
          <Card
            key={u.id}
            selected={u.id === useCase}
            onClick={() =>
              withViewTransition(() => {
                chooseUseCase(catalog, u.id);
                setShowMore(false);
              })
            }
          >
            <div className="font-medium">{t(`useCases.${u.id}.title`)}</div>
            <div className="mt-1 text-sm text-zinc-500">{t(`useCases.${u.id}.description`)}</div>
          </Card>
        ))}
      </div>

      {useCase && (
        <section className="space-y-2">
          <h2 className="text-sm font-medium text-zinc-500">
            {t("useCase.suggested", { useCase: t(`useCases.${useCase}.title`) })}
          </h2>
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
      )}
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
