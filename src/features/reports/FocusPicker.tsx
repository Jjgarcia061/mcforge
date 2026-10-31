import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button, Input, Label, Notice, Select, Textarea } from "../../components/ui";
import { commands, errorMessage, unwrap, type Focus, type FocusPreset } from "../../lib/api";

const NEW = "__new";

/** Picks who the report is written for: a preset or one of the user's own focuses. */
export function FocusPicker({
  presets,
  custom,
  value,
  disabled,
  onChange,
  onSaved,
}: {
  presets: FocusPreset[];
  custom: Focus[];
  value: string;
  disabled: boolean;
  onChange: (id: string) => void;
  /** Called after a save or delete with the focus to select next. */
  onSaved: (select: string) => void;
}) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState<Focus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const own = custom.find((f) => f.id === value);

  async function save(focus: Focus) {
    try {
      const saved = await unwrap(commands.saveFocus(focus));
      setEditing(null);
      onSaved(saved.id);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function remove(id: string) {
    try {
      await unwrap(commands.deleteFocus(id));
      onSaved(presets[0]?.id ?? "");
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  function edit(focus: Focus) {
    setError(null);
    setEditing(focus);
  }

  return (
    <div>
      <label className="block">
        <Label hint={disabled ? t("reports.focusNeedsAi") : undefined}>{t("reports.focusLabel")}</Label>
        <Select
          value={editing && !editing.id ? NEW : value}
          disabled={disabled}
          onChange={(e) => {
            if (e.target.value === NEW) edit({ id: "", name: "", instructions: "" });
            else {
              setEditing(null);
              onChange(e.target.value);
            }
          }}
        >
          <optgroup label={t("reports.focusPresets")}>
            {presets.map((p) => (
              <option key={p.id} value={p.id}>
                {t(`reports.focus.${p.id}.name`)}
              </option>
            ))}
          </optgroup>
          {custom.length > 0 && (
            <optgroup label={t("reports.focusMine")}>
              {custom.map((f) => (
                <option key={f.id} value={f.id}>
                  {f.name}
                </option>
              ))}
            </optgroup>
          )}
          <option value={NEW}>{t("reports.focusNew")}</option>
        </Select>
      </label>

      {editing ? (
        <FocusEditor
          focus={editing}
          error={error}
          onCancel={() => setEditing(null)}
          onSave={(f) => void save(f)}
        />
      ) : (
        <div className="mt-1.5 flex items-start gap-2 text-xs text-zinc-500">
          <p className="line-clamp-2 flex-1">
            {own ? own.instructions : t(`reports.focus.${value}.description`, { defaultValue: "" })}
          </p>
          {own && !disabled && (
            <span className="flex shrink-0 gap-1">
              <Button variant="ghost" className="px-2 py-0.5 text-xs" onClick={() => edit(own)}>
                {t("reports.focusEdit")}
              </Button>
              <Button variant="ghost" className="px-2 py-0.5 text-xs" onClick={() => void remove(own.id)}>
                {t("reports.focusDelete")}
              </Button>
            </span>
          )}
        </div>
      )}
    </div>
  );
}

function FocusEditor({
  focus,
  error,
  onCancel,
  onSave,
}: {
  focus: Focus;
  error: string | null;
  onCancel: () => void;
  onSave: (focus: Focus) => void;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState(focus.name);
  const [instructions, setInstructions] = useState(focus.instructions);
  const ready = name.trim() !== "" && instructions.trim() !== "";
  return (
    <div className="mt-3 space-y-3 rounded-lg border border-zinc-200 p-3 dark:border-zinc-800">
      <label className="block">
        <Label>{t("reports.focusName")}</Label>
        <Input value={name} placeholder={t("reports.focusNamePlaceholder")} onChange={(e) => setName(e.target.value)} />
      </label>
      <label className="block">
        <Label>{t("reports.focusInstructions")}</Label>
        <Textarea
          rows={3}
          value={instructions}
          placeholder={t("reports.focusInstructionsPlaceholder")}
          onChange={(e) => setInstructions(e.target.value)}
        />
      </label>
      {error && <Notice tone="bad">{error}</Notice>}
      <div className="flex gap-2">
        <Button disabled={!ready} onClick={() => onSave({ id: focus.id, name, instructions })}>
          {t("reports.focusSave")}
        </Button>
        <Button variant="ghost" onClick={onCancel}>
          {t("nav.cancel")}
        </Button>
      </div>
    </div>
  );
}
