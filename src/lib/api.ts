// Single import point for the core API. Re-exports the generated bindings under
// their output-side names and turns typed command results into plain promises.
import type { AppError } from "./bindings";

export { commands } from "./bindings";
export { Channel } from "@tauri-apps/api/core";
export type {
  AppError,
  ApiModel_Serialize as ApiModel,
  Catalog_Serialize as Catalog,
  Category,
  ColumnKind,
  Connector_Serialize as Connector,
  Engine,
  Field_Serialize as Field,
  FieldValue,
  Focus,
  FocusPreset,
  Fit,
  Hardware,
  NumberFormat,
  OllamaStatus,
  Permission,
  Provider,
  PullProgress_Deserialize as PullProgress,
  Recommendation_Serialize as Recommendation,
  ReportEntry_Serialize as ReportEntry,
  ReportOutcome_Serialize as ReportOutcome,
  ReportRequest_Deserialize as ReportRequest,
  ReportStage,
  ReportWarning,
  Risk,
  SavedFocuses_Serialize as SavedFocuses,
  Setup_Serialize as Setup,
  SourceColumn,
  SourceSummary,
  SystemReport_Serialize as SystemReport,
  TemplateInfo,
  TestReport,
  Tool_Serialize as Tool,
  UseCase,
} from "./bindings";

export class CommandError extends Error {
  constructor(readonly detail: AppError) {
    super(detail.message);
  }
}

type CommandResult<T> = { status: "ok"; data: T } | { status: "error"; error: AppError };

export async function unwrap<T>(result: Promise<CommandResult<T>>): Promise<T> {
  const r = await result;
  if (r.status === "error") throw new CommandError(r.error);
  return r.data;
}

// The core sends f64 values, which the bindings type as `number | null`.
export const gb = (n: number | null) => `${(n ?? 0).toFixed(1)} GB`;

export const errorMessage = (e: unknown) => (e instanceof Error ? e.message : String(e));
