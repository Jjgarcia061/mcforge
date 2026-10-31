import { openUrl } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Badge, Button, Card, Input, Label, Notice, Progress, Segmented, Select } from "../../../components/ui";
import {
  Channel,
  commands,
  errorMessage,
  gb,
  unwrap,
  type Catalog,
  type Fit,
  type OllamaStatus,
  type Provider,
  type PullProgress,
  type Recommendation,
} from "../../../lib/api";
import { useAppData } from "../../../lib/data";
import { useWizard } from "../store";

const FIT_TONE: Record<Fit, "good" | "warn" | "bad"> = { smooth: "good", usable: "warn", tooBig: "bad" };
const PROVIDERS: Provider[] = ["anthropic", "openai"];
// OpenAI lists every model; keep the ones that can chat and call tools.
const OPENAI_NON_CHAT = /audio|realtime|tts|transcribe|image|search|embedding|moderation|dall-e|whisper|babbage|davinci/;

export function EngineStep() {
  const { t } = useTranslation();
  const { recommendations } = useAppData().report;
  const engine = useWizard((s) => s.engine);
  const [tab, setTab] = useState<"local" | "api">(
    engine?.kind ?? (recommendations.some((r) => r.recommended) ? "local" : "api"),
  );

  return (
    <div className="space-y-5">
      <Segmented
        value={tab}
        onChange={setTab}
        options={[
          { value: "local", label: t("engine.local") },
          { value: "api", label: t("engine.api") },
        ]}
      />
      {tab === "local" ? <LocalModels /> : <ApiProviders />}
    </div>
  );
}

function LocalModels() {
  const { t } = useTranslation();
  const { recommendations } = useAppData().report;
  const [status, setStatus] = useState<OllamaStatus | null>(null);
  const [pulling, setPulling] = useState<{ model: string; progress: number | null } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(
    () =>
      unwrap(commands.ollamaStatus()).then(
        (s) => setStatus(s),
        (e) => setError(errorMessage(e)),
      ),
    [],
  );
  useEffect(() => void refresh(), [refresh]);

  async function download(model: string) {
    setError(null);
    setPulling({ model, progress: null });
    const channel = new Channel<PullProgress>();
    channel.onmessage = (p) =>
      setPulling({ model, progress: p.total && p.completed ? p.completed / p.total : null });
    try {
      await unwrap(commands.pullModel(model, channel));
      await refresh();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setPulling(null);
    }
  }

  const installed = status?.state === "running" ? status.models : [];
  const others = installed.filter((m) => !recommendations.some((r) => r.model.id === m));

  return (
    <div className="space-y-4">
      <OllamaBanner status={status} onRetry={refresh} />
      {error && <Notice tone="bad">{error}</Notice>}
      <div className="space-y-2">
        {recommendations.map((r) => (
          <ModelCard
            key={r.model.id}
            rec={r}
            installed={installed.includes(r.model.id)}
            canDownload={status?.state === "running" && pulling === null}
            progress={pulling?.model === r.model.id ? pulling.progress : undefined}
            onDownload={() => void download(r.model.id)}
          />
        ))}
      </div>
      <p className="text-xs text-zinc-500">{t("engine.weakTools")}</p>
      {others.length > 0 && (
        <div className="space-y-2 pt-2">
          <div>
            <div className="text-sm font-medium">{t("engine.otherInstalled")}</div>
            <p className="text-xs text-zinc-500">{t("engine.otherInstalledHint")}</p>
          </div>
          {others.map((m) => (
            <InstalledModel key={m} model={m} />
          ))}
        </div>
      )}
    </div>
  );
}

/** A model already in Ollama that the catalog does not know, so it has no fit or tool rating. */
function InstalledModel({ model }: { model: string }) {
  const { t } = useTranslation();
  const { engine, setEngine } = useWizard();
  const selected = engine?.kind === "local" && engine.model === model;
  return (
    <Card selected={selected}>
      <div className="flex items-center justify-between gap-4">
        <span className="truncate font-mono text-sm">{model}</span>
        <div className="flex shrink-0 gap-2">
          <Badge tone="good">{t("engine.installed")}</Badge>
          <Button variant={selected ? "primary" : "secondary"} onClick={() => setEngine({ kind: "local", model })}>
            {t(selected ? "engine.selected" : "engine.use")}
          </Button>
        </div>
      </div>
    </Card>
  );
}

function OllamaBanner({ status, onRetry }: { status: OllamaStatus | null; onRetry: () => void }) {
  const { t } = useTranslation();
  if (!status) return <Notice>{t("engine.ollama.checking")}</Notice>;
  if (status.state === "running") return <Notice tone="good">{t("engine.ollama.running", { version: status.version })}</Notice>;
  return (
    <Notice tone="warn">
      <div className="flex items-center justify-between gap-4">
        <span>{t(status.state === "stopped" ? "engine.ollama.stopped" : "engine.ollama.notInstalled")}</span>
        <span className="flex shrink-0 gap-2">
          {status.state === "notInstalled" && (
            <Button variant="secondary" onClick={() => void openUrl("https://ollama.com/download")}>
              {t("engine.ollama.install")}
            </Button>
          )}
          <Button variant="secondary" onClick={onRetry}>
            {t("engine.ollama.retry")}
          </Button>
        </span>
      </div>
    </Notice>
  );
}

function ModelCard({
  rec,
  installed,
  canDownload,
  progress,
  onDownload,
}: {
  rec: Recommendation;
  installed: boolean;
  canDownload: boolean;
  progress: number | null | undefined;
  onDownload: () => void;
}) {
  const { t } = useTranslation();
  const { engine, setEngine } = useWizard();
  const selected = engine?.kind === "local" && engine.model === rec.model.id;
  const runnable = rec.fit !== "tooBig";

  return (
    <Card selected={selected} className={runnable ? undefined : "opacity-60"}>
      <div className="flex items-center justify-between gap-4">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium">{rec.model.name}</span>
            {rec.recommended && <Badge tone="brand">{t("engine.recommended")}</Badge>}
            <Badge tone={FIT_TONE[rec.fit]}>{t(`engine.fit.${rec.fit}`)}</Badge>
            <Badge tone={rec.weakToolUse ? "warn" : "neutral"}>{t(`engine.tools.${rec.model.tools}`)}</Badge>
          </div>
          <div className="mt-1 text-xs text-zinc-500">
            {rec.model.id} · {gb(rec.model.sizeGb)} · {t("engine.memoryNeeded", { gb: gb(rec.memoryNeededGb) })}
          </div>
        </div>
        {runnable && (
          <div className="flex shrink-0 gap-2">
            {installed ? (
              <Badge tone="good">{t("engine.installed")}</Badge>
            ) : (
              progress === undefined && (
                <Button variant="secondary" disabled={!canDownload} onClick={onDownload}>
                  {t("engine.download")}
                </Button>
              )
            )}
            <Button
              variant={selected ? "primary" : "secondary"}
              onClick={() => setEngine({ kind: "local", model: rec.model.id })}
            >
              {t(selected ? "engine.selected" : "engine.use")}
            </Button>
          </div>
        )}
      </div>
      {progress !== undefined && (
        <div className="mt-3">
          <Progress value={progress} />
        </div>
      )}
    </Card>
  );
}

function ApiProviders() {
  const { t } = useTranslation();
  const engine = useWizard((s) => s.engine);
  const [provider, setProvider] = useState<Provider>(engine?.kind === "api" ? engine.provider : "anthropic");

  return (
    <div className="space-y-4">
      <Segmented
        size="sm"
        value={provider}
        onChange={setProvider}
        options={PROVIDERS.map((p) => ({ value: p, label: t(`engine.provider.${p}`) }))}
      />
      {/* Keyed so each provider starts from a clean form. */}
      <ProviderPanel key={provider} provider={provider} />
    </div>
  );
}

/** Catalog defaults first (recommended on top), then any other chat model the key can use. */
export function modelOptions(catalog: Catalog, provider: Provider, available: string[]) {
  const pinned = catalog.models.api
    .filter((m) => m.provider === provider)
    .sort((a, b) => Number(b.recommended) - Number(a.recommended));
  const pinnedIds = new Set(pinned.map((m) => m.id));
  const rest = available
    .filter((id) => !pinnedIds.has(id))
    .filter((id) => provider !== "openai" || (/^(gpt|o\d|chatgpt)/.test(id) && !OPENAI_NON_CHAT.test(id)));
  return [...pinned.map((m) => ({ id: m.id, label: m.name })), ...rest.map((id) => ({ id, label: id }))];
}

function ProviderPanel({ provider }: { provider: Provider }) {
  const { t } = useTranslation();
  const { catalog } = useAppData();
  const { engine, setEngine } = useWizard();
  const [models, setModels] = useState<string[] | null>(null);
  const [replacing, setReplacing] = useState(false);
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const options = models ? modelOptions(catalog, provider, models) : [];
  const current = engine?.kind === "api" && engine.provider === provider ? engine.model : undefined;

  const accept = useCallback(
    (available: string[]) => {
      setModels(available);
      const first = modelOptions(catalog, provider, available)[0];
      const chosen = useWizard.getState().engine;
      if (first && (chosen?.kind !== "api" || chosen.provider !== provider)) {
        setEngine({ kind: "api", provider, model: first.id });
      }
    },
    [catalog, provider, setEngine],
  );

  useEffect(() => {
    let live = true;
    unwrap(commands.savedProviderModels(provider)).then(
      (saved) => live && saved && accept(saved),
      () => {}, // A stale or revoked key simply means asking for a new one.
    );
    return () => {
      live = false;
    };
  }, [provider, accept]);

  async function connect() {
    setBusy(true);
    setError(null);
    try {
      accept(await unwrap(commands.connectProvider(provider, key)));
      setKey("");
      setReplacing(false);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  if (models && !replacing) {
    return (
      <Card>
        <div className="mb-4 flex items-center justify-between gap-4">
          <Badge tone="good">{t("engine.keySaved")}</Badge>
          <Button variant="ghost" onClick={() => setReplacing(true)}>
            {t("engine.replaceKey")}
          </Button>
        </div>
        <label className="block">
          <Label>{t("engine.model")}</Label>
          <Select value={current ?? ""} onChange={(e) => setEngine({ kind: "api", provider, model: e.target.value })}>
            {options.map((o) => (
              <option key={o.id} value={o.id}>
                {o.label}
              </option>
            ))}
          </Select>
        </label>
      </Card>
    );
  }

  return (
    <Card>
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          void connect();
        }}
      >
        <label className="block">
          <Label>{t("engine.apiKey")}</Label>
          <Input type="password" autoComplete="off" value={key} onChange={(e) => setKey(e.target.value)} />
        </label>
        <p className="text-xs text-zinc-500">{t("engine.keyHint")}</p>
        {error && <Notice tone="bad">{error}</Notice>}
        <Button type="submit" disabled={busy || !key.trim()}>
          {t(busy ? "engine.connecting" : "engine.connect")}
        </Button>
      </form>
    </Card>
  );
}
