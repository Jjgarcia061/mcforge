import { useTranslation } from "react-i18next";
import { Badge, Card, Notice, Segmented } from "../../../components/ui";
import type { Permission, Risk } from "../../../lib/api";
import { useAppData } from "../../../lib/data";
import { permissionOf, selectedConnectors, useWizard } from "../store";

const PERMISSIONS: Permission[] = ["allow", "ask", "deny"];
const RISK_TONE: Record<Risk, "neutral" | "warn" | "bad" | "brand"> = {
  read: "neutral",
  write: "warn",
  network: "brand",
  send: "warn",
  destructive: "bad",
};

export function PermissionsStep() {
  const { t } = useTranslation();
  const { catalog } = useAppData();
  const state = useWizard();
  const options = PERMISSIONS.map((p) => ({ value: p, label: t(`permissions.${p}`) }));

  return (
    <div className="space-y-4">
      <Notice tone="brand">{t("permissions.intro")}</Notice>
      {selectedConnectors(state, catalog).map((c) => (
        <Card key={c.id}>
          <h2 className="mb-3 font-medium">{c.name}</h2>
          <ul className="divide-y divide-zinc-100 dark:divide-zinc-800">
            {c.tools.map((tool) => (
              <li key={tool.name} className="flex items-center justify-between gap-4 py-2">
                <span className="flex min-w-0 items-center gap-2">
                  <code className="truncate text-sm">{tool.name}</code>
                  <Badge tone={RISK_TONE[tool.risk]}>{t(`permissions.risk.${tool.risk}`)}</Badge>
                </span>
                <Segmented
                  size="sm"
                  value={permissionOf(state, c, tool.name)}
                  onChange={(p) => state.setPermission(c.id, tool.name, p)}
                  options={options}
                />
              </li>
            ))}
          </ul>
        </Card>
      ))}
    </div>
  );
}
