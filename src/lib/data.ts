import { use } from "react";
import { commands, type Catalog, type SystemReport } from "./api";

export interface AppData {
  catalog: Catalog;
  report: SystemReport;
}

let pending: Promise<AppData> | undefined;

/** Catalog and hardware report, loaded once per session. Suspends until ready. */
export function useAppData(): AppData {
  pending ??= Promise.all([commands.getCatalog(), commands.systemReport()]).then(([catalog, report]) => ({
    catalog,
    report,
  }));
  return use(pending);
}
