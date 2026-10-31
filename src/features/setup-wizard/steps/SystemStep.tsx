import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { Card, Notice } from "../../../components/ui";
import { gb, type Hardware } from "../../../lib/api";
import { useAppData } from "../../../lib/data";

function acceleratorLabel(t: TFunction, acc: Hardware["accelerator"]) {
  switch (acc.kind) {
    case "appleSilicon":
      return t("system.appleSilicon");
    case "nvidia":
      return t("system.nvidia", { name: acc.name, vram: gb(acc.vramGb) });
    case "none":
      return t("system.none");
  }
}

export function SystemStep() {
  const { t } = useTranslation();
  const { hardware, recommendations } = useAppData().report;
  const pick = recommendations.find((r) => r.recommended);

  const rows = [
    [t("system.os"), hardware.os],
    [t("system.cpu"), `${hardware.cpu} · ${t("system.cores", { count: hardware.cores })}`],
    [t("system.memory"), gb(hardware.memoryGb)],
    [t("system.accelerator"), acceleratorLabel(t, hardware.accelerator)],
  ];

  return (
    <div className="space-y-4">
      <Card>
        <dl className="grid grid-cols-[max-content_1fr] gap-x-8 gap-y-3 text-sm">
          {rows.map(([k, v]) => (
            <div key={k} className="contents">
              <dt className="text-zinc-500">{k}</dt>
              <dd className="font-medium">{v}</dd>
            </div>
          ))}
        </dl>
      </Card>
      {pick ? (
        <Notice tone="good">
          <strong>{t("system.recommended", { model: pick.model.name })}</strong>
          <br />
          {t("system.recommendedHint")}
        </Notice>
      ) : (
        <Notice tone="warn">
          <strong>{t("system.noneFits")}</strong>
          <br />
          {t("system.noneFitsHint")}
        </Notice>
      )}
    </div>
  );
}
