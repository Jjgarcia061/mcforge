import type { Catalog, Engine } from "./api";

/** How a model is shown: the catalog's name for API models, the Ollama tag for local ones. */
export function modelName(catalog: Catalog, engine: Engine): string {
  if (engine.kind === "local") return engine.model;
  return catalog.models.api.find((m) => m.provider === engine.provider && m.id === engine.model)?.name ?? engine.model;
}
