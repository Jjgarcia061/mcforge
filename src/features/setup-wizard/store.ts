import { create } from "zustand";
import type { Catalog, Connector, FieldValue, Permission, Provider } from "../../lib/api";

export const STEPS = ["system", "engine", "useCase", "connections", "permissions", "finish"] as const;
export type Step = (typeof STEPS)[number];

export type Engine = { kind: "local"; model: string } | { kind: "api"; provider: Provider; model: string };

export interface WizardState {
  step: number;
  engine: Engine | null;
  useCase: string | null;
  connectors: string[];
  values: Record<string, Record<string, FieldValue>>;
  permissions: Record<string, Record<string, Permission>>;
}

interface WizardActions {
  goTo: (step: number) => void;
  setEngine: (engine: Engine | null) => void;
  chooseUseCase: (catalog: Catalog, id: string) => void;
  toggleConnector: (id: string) => void;
  setValue: (connector: string, key: string, value: FieldValue) => void;
  setPermission: (connector: string, tool: string, permission: Permission) => void;
}

export const initialState: WizardState = {
  step: 0,
  engine: null,
  useCase: null,
  connectors: [],
  values: {},
  permissions: {},
};

export const useWizard = create<WizardState & WizardActions>()((set) => ({
  ...initialState,
  goTo: (step) => set({ step: Math.max(0, Math.min(STEPS.length - 1, step)) }),
  setEngine: (engine) => set({ engine }),
  chooseUseCase: (catalog, id) =>
    set({ useCase: id, connectors: [...(catalog.useCases.find((u) => u.id === id)?.connectors ?? [])] }),
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

/** Connectors a use case suggests (in its order) and the rest of the catalog. An empty use case suggests everything. */
export function suggestedConnectors(catalog: Catalog, useCase: string | null) {
  const ids = catalog.useCases.find((u) => u.id === useCase)?.connectors ?? [];
  if (ids.length === 0) return { suggested: catalog.connectors, others: [] };
  return {
    suggested: ids.flatMap((id) => catalog.connectors.filter((c) => c.id === id)),
    others: catalog.connectors.filter((c) => !ids.includes(c.id)),
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
      return state.connectors.length > 0;
    case "connections":
      return selectedConnectors(state, catalog).every((c) => missingFields(state, c).length === 0);
    default:
      return true;
  }
}
