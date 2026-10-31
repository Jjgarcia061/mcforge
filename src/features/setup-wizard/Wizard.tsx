import type { ComponentType } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/ui";
import { useAppData } from "../../lib/data";
import { useSession } from "../../lib/session";
import { canContinue, STEPS, useWizard, type Step } from "./store";
import { ConnectionsStep } from "./steps/ConnectionsStep";
import { EngineStep } from "./steps/EngineStep";
import { FinishStep } from "./steps/FinishStep";
import { PermissionsStep } from "./steps/PermissionsStep";
import { SystemStep } from "./steps/SystemStep";
import { UseCaseStep } from "./steps/UseCaseStep";

const VIEWS: Record<Step, ComponentType> = {
  system: SystemStep,
  engine: EngineStep,
  useCase: UseCaseStep,
  connections: ConnectionsStep,
  permissions: PermissionsStep,
  finish: FinishStep,
};

export function Wizard() {
  const { t } = useTranslation();
  const { catalog } = useAppData();
  const state = useWizard();
  const step = STEPS[state.step] ?? "system";
  const View = VIEWS[step];
  const last = state.step === STEPS.length - 1;
  const editing = useSession((s) => s.setup !== null);

  return (
    <div className="flex h-screen">
      <aside className="flex w-64 shrink-0 flex-col border-r border-zinc-200 bg-white p-6 dark:border-zinc-800 dark:bg-zinc-900">
        <div className="mb-8">
          <div className="text-xl font-bold tracking-tight">
            MC<span className="text-orange-600">Forge</span>
          </div>
          <div className="text-xs text-zinc-500">{t("app.tagline")}</div>
        </div>
        <ol className="space-y-1">
          {STEPS.map((s, i) => (
            <li key={s}>
              <button
                type="button"
                disabled={i > state.step}
                onClick={() => state.goTo(i)}
                className={`flex w-full items-center gap-3 rounded-lg px-3 py-2 text-left text-sm ${
                  i === state.step
                    ? "bg-orange-50 font-medium text-orange-700 dark:bg-orange-950/50 dark:text-orange-300"
                    : "text-zinc-600 enabled:hover:bg-zinc-100 disabled:opacity-50 dark:text-zinc-400 dark:enabled:hover:bg-zinc-800"
                }`}
              >
                <span
                  className={`grid size-6 place-items-center rounded-full text-xs ${
                    i < state.step ? "bg-orange-600 text-white" : "border border-current"
                  }`}
                >
                  {i < state.step ? "✓" : i + 1}
                </span>
                {t(`steps.${s}.title`)}
              </button>
            </li>
          ))}
        </ol>
        {editing && (
          <Button variant="ghost" className="mt-auto" onClick={() => useSession.setState({ editing: false })}>
            {t("nav.cancel")}
          </Button>
        )}
      </aside>

      <main className="flex min-w-0 flex-1 flex-col">
        <div className="flex-1 overflow-y-auto px-10 py-8">
          <div className="mx-auto max-w-3xl">
            <p className="text-xs font-medium tracking-wide text-zinc-500 uppercase">
              {t("nav.stepOf", { n: state.step + 1, total: STEPS.length })}
            </p>
            <h1 className="mt-1 text-2xl font-semibold tracking-tight">{t(`steps.${step}.title`)}</h1>
            <p className="mt-1 mb-6 text-zinc-500">{t(`steps.${step}.subtitle`)}</p>
            <View />
          </div>
        </div>
        {!last && (
          <footer className="flex justify-between border-t border-zinc-200 px-10 py-4 dark:border-zinc-800">
            <Button variant="ghost" disabled={state.step === 0} onClick={() => state.goTo(state.step - 1)}>
              {t("nav.back")}
            </Button>
            <Button disabled={!canContinue(state, catalog)} onClick={() => state.goTo(state.step + 1)}>
              {t("nav.next")}
            </Button>
          </footer>
        )}
      </main>
    </div>
  );
}
