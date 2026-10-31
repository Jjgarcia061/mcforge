import { useTranslation } from "react-i18next";
import { Button, Card, Notice } from "../../../components/ui";
import type { Permission } from "../../../lib/api";
import { useAppData } from "../../../lib/data";
import { permissionOf, selectedConnectors, useWizard } from "../store";

export function FinishStep() {
  const { t } = useTranslation();
  const { catalog } = useAppData();
  const state = useWizard();
  const connectors = selectedConnectors(state, catalog);
  const { engine } = state;

  const counts: Record<Permission, number> = { allow: 0, ask: 0, deny: 0 };
  for (const c of connectors) for (const tool of c.tools) counts[permissionOf(state, c, tool.name)]++;

  return (
    <div className="space-y-4">
      <Card>
        <dl className="grid grid-cols-[max-content_1fr] gap-x-8 gap-y-3 text-sm">
          <dt className="text-zinc-500">{t("finish.engine")}</dt>
          <dd className="font-medium">
            {engine?.kind === "local" && t("finish.local", { model: engine.model })}
            {engine?.kind === "api" &&
              t("finish.api", { provider: t(`engine.provider.${engine.provider}`), model: engine.model })}
          </dd>
          <dt className="text-zinc-500">{t("finish.connections")}</dt>
          <dd>
            <div className="font-medium">{connectors.map((c) => c.name).join(", ")}</div>
            <div className="text-zinc-500">{t("finish.rules", counts)}</div>
          </dd>
        </dl>
      </Card>
      <Notice>{t("finish.comingSoon")}</Notice>
      <div className="flex justify-between">
        <Button variant="ghost" onClick={() => state.goTo(state.step - 1)}>
          {t("nav.back")}
        </Button>
        <Button disabled>{t("finish.start")}</Button>
      </div>
    </div>
  );
}
