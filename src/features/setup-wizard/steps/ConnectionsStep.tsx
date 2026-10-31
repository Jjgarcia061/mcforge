import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Badge, Button, Card, Input, Label, Notice, Select } from "../../../components/ui";
import { commands, unwrap, type Connector, type Field, type FieldValue, type TestReport } from "../../../lib/api";
import { useAppData } from "../../../lib/data";
import { missingFields, selectedConnectors, useWizard } from "../store";

const INPUT_TYPE: Partial<Record<Field["kind"], string>> = { secret: "password", number: "number", url: "url" };

export function ConnectionsStep() {
  const { t } = useTranslation();
  const { catalog } = useAppData();
  const configurable = selectedConnectors(useWizard(), catalog).filter((c) => c.fields.length > 0);

  if (configurable.length === 0) return <p className="text-zinc-500">{t("connections.nothingToConfigure")}</p>;

  return (
    <div className="space-y-4">
      {configurable.map((c) => (
        <ConnectorForm key={c.id} connector={c} />
      ))}
    </div>
  );
}

function ConnectorForm({ connector }: { connector: Connector }) {
  const { t } = useTranslation();
  const state = useWizard();
  const values = state.values[connector.id];
  const complete = missingFields(state, connector).length === 0;

  return (
    <Card>
      <h2 className="mb-4 font-medium">{connector.name}</h2>
      <div className="space-y-4">
        {connector.fields.map((f) => (
          <div key={f.key}>
            <Label hint={f.required ? t("connections.required") : undefined}>{f.label}</Label>
            <FieldInput field={f} value={values?.[f.key]} onChange={(v) => state.setValue(connector.id, f.key, v)} />
          </div>
        ))}
      </div>
      {/* Keyed by the values so an old result disappears as soon as anything changes. */}
      {connector.testable && (
        <ConnectionTest key={JSON.stringify(values)} connector={connector} values={values ?? {}} disabled={!complete} />
      )}
    </Card>
  );
}

function ConnectionTest({
  connector,
  values,
  disabled,
}: {
  connector: Connector;
  values: Record<string, FieldValue>;
  disabled: boolean;
}) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ ok: TestReport } | { error: string } | null>(null);

  async function run() {
    setBusy(true);
    try {
      setResult({ ok: await unwrap(commands.testConnection(connector.id, values)) });
    } catch (e) {
      setResult({ error: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="mt-5 space-y-3 border-t border-zinc-100 pt-4 dark:border-zinc-800">
      <Button variant="secondary" disabled={disabled || busy} onClick={() => void run()}>
        {t(busy ? "connections.testing" : "connections.test")}
      </Button>
      {result && "ok" in result && (
        <Notice tone="good">
          <div className="font-medium">
            {t("connections.testOk", { title: result.ok.title, count: result.ok.items.length })}
          </div>
          {result.ok.items.length > 0 && (
            <div className="mt-2 flex flex-wrap gap-1.5">
              {result.ok.items.map((item) => (
                <Badge key={item}>{item}</Badge>
              ))}
            </div>
          )}
        </Notice>
      )}
      {result && "error" in result && <Notice tone="bad">{result.error}</Notice>}
    </div>
  );
}

function FieldInput({
  field,
  value,
  onChange,
}: {
  field: Field;
  value: FieldValue | undefined;
  onChange: (value: FieldValue) => void;
}) {
  const { t } = useTranslation();

  if (field.kind === "paths") {
    const paths = Array.isArray(value) ? value : [];
    return (
      <div className="space-y-2">
        {paths.map((p) => (
          <div key={p} className="flex items-center gap-2 rounded-lg bg-zinc-100 px-3 py-1.5 text-sm dark:bg-zinc-800">
            <span className="min-w-0 flex-1 truncate font-mono text-xs">{p}</span>
            <Button variant="ghost" className="px-2 py-1" onClick={() => onChange(paths.filter((x) => x !== p))}>
              {t("connections.remove")}
            </Button>
          </div>
        ))}
        <Button
          variant="secondary"
          onClick={async () => {
            const picked = await open({ directory: true, multiple: true });
            if (picked) onChange([...new Set([...paths, ...picked])]);
          }}
        >
          {t("connections.addFolder")}
        </Button>
      </div>
    );
  }

  const text = typeof value === "string" ? value : "";

  if (field.kind === "path") {
    return (
      <div className="flex gap-2">
        <Input readOnly value={text} className="font-mono text-xs" />
        <Button
          variant="secondary"
          className="shrink-0"
          onClick={async () => {
            const picked = await open({ directory: true });
            if (picked) onChange(picked);
          }}
        >
          {t("connections.chooseFolder")}
        </Button>
      </div>
    );
  }

  if (field.kind === "select") {
    return (
      <Select value={text} onChange={(e) => onChange(e.target.value)}>
        <option value="" disabled>
          {t("connections.select")}
        </option>
        {field.options.map((o) => (
          <option key={o} value={o}>
            {o}
          </option>
        ))}
      </Select>
    );
  }

  return (
    <Input
      type={INPUT_TYPE[field.kind] ?? "text"}
      autoComplete="off"
      placeholder={field.placeholder ?? undefined}
      value={text}
      onChange={(e) => onChange(e.target.value)}
    />
  );
}
