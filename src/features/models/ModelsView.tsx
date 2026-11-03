import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Badge, Button, Card, Notice } from "../../components/ui";
import { commands, errorMessage, gb, unwrap, type LocalModels, type ModelAdvice } from "../../lib/api";
import { useAppData } from "../../lib/data";
import { useSession } from "../../lib/session";
import { KitCard } from "./KitCard";

const FIT_TONE = { smooth: "good", usable: "warn", tooBig: "bad" } as const;

/** The models downloaded on this computer: which one is in use, switch, delete. */
export function ModelsView() {
  const { t, i18n } = useTranslation();
  const { catalog } = useAppData();
  const setup = useSession((s) => s.setup);
  const [local, setLocal] = useState<LocalModels | null>(null);
  const [advice, setAdvice] = useState<ModelAdvice | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<string | null>(null);

  const current = setup?.engine.kind === "local" ? setup.engine.model : undefined;
  const search = catalog.models.embedding[0]?.id;
  const needs = setup?.needs;

  const refresh = useCallback(() => {
    if (!needs) return;
    Promise.all([unwrap(commands.localModels()), unwrap(commands.recommendModels(needs))]).then(
      ([l, a]) => {
        setLocal(l);
        setAdvice(a);
        setError(null);
      },
      (e: unknown) => setError(errorMessage(e)),
    );
  }, [needs]);
  useEffect(() => refresh(), [refresh]);

  async function use(model: string) {
    if (!setup) return;
    const next = { ...setup, engine: { kind: "local" as const, model } };
    try {
      await unwrap(commands.saveSetup(next));
      useSession.setState({ setup: next });
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function remove(model: string) {
    setConfirming(null);
    try {
      await unwrap(commands.deleteModel(model));
      refresh();
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  const same = (a: string, b?: string) => a === b || a === `${b}:latest`;
  const used = local?.models.reduce((sum, m) => sum + (m.sizeGb ?? 0), 0) ?? 0;
  const date = new Intl.DateTimeFormat(i18n.resolvedLanguage, { dateStyle: "medium" });

  return (
    <div className="mx-auto max-w-3xl space-y-5">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">{t("models.title")}</h1>
        <p className="mt-1 text-zinc-500">{t("models.subtitle")}</p>
      </div>

      {setup?.engine.kind === "api" && <Notice>{t("models.apiNote")}</Notice>}
      {error && <Notice tone="bad">{error}</Notice>}

      {advice && (
        <KitCard
          advice={advice}
          current={current}
          canDownload={local !== null}
          onReady={(model) => void use(model)}
          onDownloaded={refresh}
        />
      )}

      <Card>
        <div className="mb-3 flex flex-wrap items-baseline justify-between gap-2">
          <h2 className="font-medium">{t("models.installed")}</h2>
          {local && (
            <span className="text-xs text-zinc-500">
              {t("models.used", { gb: gb(used) })}
              {local.freeGb !== null && ` · ${t("kit.free", { gb: gb(local.freeGb) })}`}
            </span>
          )}
        </div>
        {local === null && !error && <p className="text-sm text-zinc-500">{t("models.loading")}</p>}
        {local?.models.length === 0 && <p className="text-sm text-zinc-500">{t("models.empty")}</p>}
        <ul className="divide-y divide-zinc-100 dark:divide-zinc-800">
          {local?.models.map((m) => {
            const inUse = same(m.name, current);
            const isSearch = same(m.name, search);
            const known = advice?.ranked.find((r) => same(m.name, r.model.id));
            return (
              <li key={m.name} className="py-3">
                <div className="flex flex-wrap items-center gap-2">
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="font-medium">{known?.model.name ?? m.name}</span>
                      {inUse && <Badge tone="brand">{t("models.inUse")}</Badge>}
                      {isSearch && <Badge tone="good">{t("models.search")}</Badge>}
                      {known && <Badge tone={FIT_TONE[known.fit]}>{t(`engine.fit.${known.fit}`)}</Badge>}
                    </div>
                    <div className="mt-0.5 text-xs text-zinc-500">
                      <span className="font-mono">{m.name}</span> · {gb(m.sizeGb)}
                      {m.modified && ` · ${date.format(new Date(m.modified))}`}
                    </div>
                  </div>
                  {!inUse && !isSearch && (
                    <Button variant="secondary" className="px-3 py-1.5" onClick={() => void use(m.name)}>
                      {t("models.use")}
                    </Button>
                  )}
                  <Button
                    variant="ghost"
                    className="px-3 py-1.5"
                    disabled={inUse || isSearch}
                    title={inUse || isSearch ? t("models.cannotDelete") : undefined}
                    onClick={() => setConfirming(m.name)}
                  >
                    {t("models.delete")}
                  </Button>
                </div>
                {confirming === m.name && (
                  <div className="mt-2">
                    <Notice tone="warn">
                      <div>{t("models.confirmDelete", { model: m.name, gb: gb(m.sizeGb) })}</div>
                      <div className="mt-2 flex gap-2">
                        <Button className="px-3 py-1.5" onClick={() => void remove(m.name)}>
                          {t("models.delete")}
                        </Button>
                        <Button variant="ghost" className="px-3 py-1.5" onClick={() => setConfirming(null)}>
                          {t("nav.cancel")}
                        </Button>
                      </div>
                    </Notice>
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      </Card>
    </div>
  );
}
