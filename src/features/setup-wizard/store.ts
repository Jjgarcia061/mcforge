import { create } from "zustand";
import type { Catalog, Connector, Engine, FieldValue, Language, Needs, Permission, Priority, Setup } from "../../lib/api";

// The interview (`useCase`) comes before the engine, so the model recommendation knows the answers.
export const STEPS = ["system", "useCase", "engine", "connections", "permissions", "finish"] as const;
export type Step = (typeof STEPS)[number];

export interface WizardState {
  step: number;
  engine: Engine | null;
  needs: Needs;
  connectors: string[];
  values: Record<string, Record<string, FieldValue>>;
  permissions: Record<string, Record<string, Permission>>;
}

interface WizardActions {
  goTo: (step: number) => void;
  setEngine: (engine: Engine | null) => void;
  toggleUseCase: (catalog: Catalog, id: string) => void;
  setLanguage: (language: Language) => void;
  setPriority: (priority: Priority) => void;
  toggleConnector: (id: string) => void;
  setValue: (connector: string, key: string, value: FieldValue) => void;
  setPermission: (connector: string, tool: string, permission: Permission) => void;
}

export const initialState: WizardState = {
  step: 0,
  engine: null,
  needs: { useCases: [], language: "both", priority: "balanced" },
  connectors: [],
  values: {},
  permissions: {},
};

export const useWizard = create<WizardState & WizardActions>()((set) => ({
  ...initialState,
  goTo: (step) => set({ step: Math.max(0, Math.min(STEPS.length - 1, step)) }),
  setEngine: (engine) => set({ engine }),
  toggleUseCase: (catalog, id) =>
    set((s) => {
      const on = !s.needs.useCases.includes(id);
      const useCases = on ? [...s.needs.useCases, id] : s.needs.useCases.filter((u) => u !== id);
      const own = catalog.useCases.find((u) => u.id === id)?.connectors ?? [];
      // Turning a task off drops its connections, unless another chosen task needs them too.
      const stillNeeded = new Set(
        catalog.useCases.filter((u) => useCases.includes(u.id)).flatMap((u) => u.connectors),
      );
      const connectors = on
        ? [...s.connectors, ...own.filter((c) => !s.connectors.includes(c))]
        : s.connectors.filter((c) => !own.includes(c) || stillNeeded.has(c));
      return { needs: { ...s.needs, useCases }, connectors };
    }),
  setLanguage: (language) => set((s) => ({ needs: { ...s.needs, language } })),
  setPriority: (priority) => set((s) => ({ needs: { ...s.needs, priority } })),
  toggleConnector: (id) =>
    set((s) => ({
      connectors: s.connectors.includes(id) ? s.connectors.filter((c) => c !== id) : [...s.connectors, id],
    })),
  setValue: (connector, key, value) =>
    set((s) => ({ values: { ...s.values, [connector]: { ...s.values[connector], [key]: value } } })),
  setPermission: (connector, tool, permission) =>
    set((s) => ({
      permissions: { ...s.permissions, [connector]: { ...s.permissions[connector], [tool]: permission } },
    })),
}));

/** The wizard's answers as a setup to save, once an engine is chosen. */
export function toSetup(state: WizardState): Setup | null {
  if (!state.engine) return null;
  const { engine, needs, connectors, values, permissions } = state;
  return { engine, needs, connectors, values, permissions };
}

/** Wizard answers prefilled from a saved setup, for changing it. */
export function fromSetup(setup: Setup): WizardState {
  return { ...initialState, ...setup, step: STEPS.indexOf("engine") };
}

/** Connectors the chosen tasks suggest (in catalog order) and the rest of the catalog. */
export function suggestedConnectors(catalog: Catalog, useCases: string[]) {
  const ids = new Set(catalog.useCases.filter((u) => useCases.includes(u.id)).flatMap((u) => u.connectors));
  if (ids.size === 0) return { suggested: [], others: catalog.connectors };
  return {
    suggested: catalog.connectors.filter((c) => ids.has(c.id)),
    others: catalog.connectors.filter((c) => !ids.has(c.id)),
  };
}

export function selectedConnectors(state: WizardState, catalog: Catalog): Connector[] {
  return catalog.connectors.filter((c) => state.connectors.includes(c.id));
}

/** The user's choice for a tool, falling back to the safe default derived from its risk. */
export function permissionOf(state: WizardState, connector: Connector, tool: string): Permission {
  return (
    state.permissions[connector.id]?.[tool] ??
    connector.tools.find((t) => t.name === tool)?.defaultPermission ??
    "ask"
  );
}

export function missingFields(state: WizardState, connector: Connector): string[] {
  return connector.fields
    .filter((f) => f.required)
    .filter((f) => {
      const v = state.values[connector.id]?.[f.key];
      return Array.isArray(v) ? v.length === 0 : !v?.trim();
    })
    .map((f) => f.key);
}

/** Whether the wizard may move past the given step. */
export function canContinue(state: WizardState, catalog: Catalog): boolean {
  switch (STEPS[state.step]) {
    case "engine":
      return state.engine !== null;
    case "useCase":
      return state.needs.useCases.length > 0;
    case "connections":
      return selectedConnectors(state, catalog).every((c) => missingFields(state, c).length === 0);
    default:
      return true;
  }
}
