import { use } from "react";
import { commands, type Catalog, type SystemReport } from "./api";
import { useSession } from "./session";

export interface AppData {
  catalog: Catalog;
  report: SystemReport;
}

let pending: Promise<AppData> | undefined;

/** Catalog and hardware report, loaded once per session along with the saved setup. Suspends until ready. */
export function useAppData(): AppData {
  pending ??= Promise.all([commands.getCatalog(), commands.systemReport(), commands.loadSetup()]).then(
    ([catalog, report, setup]) => {
      useSession.setState({ setup });
      return { catalog, report };
    },
  );
  return use(pending);
}
