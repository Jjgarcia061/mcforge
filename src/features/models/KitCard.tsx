import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Badge, Button, Card, Notice, Progress } from "../../components/ui";
import { Channel, commands, errorMessage, gb, unwrap, type ModelAdvice, type PullProgress } from "../../lib/api";

/**
 * The recommended kit: one model for everything the user chose, plus the
 * library's search model when needed. Downloads what is missing in one go.
 */
export function KitCard({
  advice,
  current,
  canDownload,
  onReady,
  onDownloaded,
}: {
  advice: ModelAdvice;
  /** The model the setup uses now, if local. */
  current?: string;
  /** Ollama is running. */
  canDownload: boolean;
  onReady: (model: string) => void;
  onDownloaded: () => void;
}) {
  const { t } = useTranslation();
  const [progress, setProgress] = useState<{ name: string; value: number | null } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { kit } = advice;
  const main = advice.ranked.find((r) => r.model.id === kit.main);

  if (!main) {
    return <Notice tone="warn">{t("kit.noneRuns")}</Notice>;
  }

  const missing = [
    ...(main.installed ? [] : [{ id: main.model.id, name: main.model.name, size: main.model.sizeGb ?? 0 }]),
    ...(kit.search && !kit.searchInstalled ? [{ id: kit.search.id, name: kit.search.name, size: kit.search.sizeGb ?? 0 }] : []),
  ];
  const inUse = current === main.model.id;

  async function downloadAndUse() {
    setError(null);
    const total = missing.reduce((sum, m) => sum + m.size, 0);
    let done = 0;
    try {
      for (const m of missing) {
        setProgress({ name: m.name, value: total ? done / total : null });
        const channel = new Channel<PullProgress>();
        channel.onmessage = (p) => {
          const part = p.total && p.completed ? p.completed / p.total : 0;
          setProgress({ name: m.name, value: total ? (done + part * m.size) / total : null });
        };
        await unwrap(commands.pullModel(m.id, channel));
        done += m.size;
      }
      onDownloaded();
      onReady(main!.model.id);
    } catch (e) {
      setError(errorMessage(e));
      onDownloaded();
    } finally {
      setProgress(null);
    }
  }

  return (
    <Card selected={inUse}>
      <div className="mb-3 text-xs font-medium tracking-wide text-orange-600 uppercase">{t("kit.title")}</div>
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-lg font-semibold">{main.model.name}</span>
        <span className="text-sm text-zinc-500">· {gb(main.model.sizeGb)}</span>
        {main.installed && <Badge tone="good">{t("reason.installed")}</Badge>}
      </div>
      <div className="mt-2 flex flex-wrap gap-1.5">
        {main.reasons
          .filter((r) => r !== "installed")
          .map((r) => (
            <Badge key={r}>{t(`reason.${r}`)}</Badge>
          ))}
      </div>
      {kit.search && (
        <div className="mt-3 text-sm text-zinc-600 dark:text-zinc-400">
          + {t("kit.search", { name: kit.search.name, size: gb(kit.search.sizeGb) })}
          {kit.searchInstalled && <span className="text-zinc-500"> · {t("reason.installed")}</span>}
        </div>
      )}
      {kit.weak && (
        <div className="mt-3">
          <Notice tone="warn">{t("kit.weak")}</Notice>
        </div>
      )}

      <div className="mt-4 flex flex-wrap items-center gap-3">
        {inUse && missing.length === 0 ? (
          <Badge tone="brand">{t("kit.inUse")}</Badge>
        ) : (
          <Button
            disabled={progress !== null || (missing.length > 0 && (!canDownload || !kit.enoughSpace))}
            onClick={() => (missing.length ? void downloadAndUse() : onReady(main.model.id))}
          >
            {missing.length ? t("kit.downloadAndUse", { gb: gb(kit.downloadGb) }) : t("kit.use")}
          </Button>
        )}
        <span className="text-xs text-zinc-500">
          {missing.length ? t("kit.toDownload", { gb: gb(kit.downloadGb) }) : t("kit.allInstalled")}
          {kit.freeGb !== null && ` · ${t("kit.free", { gb: gb(kit.freeGb) })}`}
        </span>
      </div>
      {!kit.enoughSpace && missing.length > 0 && (
        <div className="mt-3">
          <Notice tone="bad">{t("kit.noSpace", { need: gb(kit.downloadGb), free: gb(kit.freeGb) })}</Notice>
        </div>
      )}
      {progress && (
        <div className="mt-3 space-y-1.5">
          <div className="text-xs text-zinc-500">{t("kit.downloading", { model: progress.name })}</div>
          <Progress value={progress.value} />
        </div>
      )}
      {error && (
        <div className="mt-3">
          <Notice tone="bad">{error}</Notice>
        </div>
      )}
    </Card>
  );
}
