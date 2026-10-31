import { Component, Suspense, type ReactNode } from "react";
import { Wizard } from "./features/setup-wizard/Wizard";
import { Workspace } from "./features/workspace/Workspace";
import i18n from "./i18n";
import { useAppData } from "./lib/data";
import { useSession } from "./lib/session";

class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="grid h-screen place-items-center p-8 text-center">
        <div>
          <h1 className="text-lg font-semibold">{i18n.t("error.title")}</h1>
          <p className="mt-2 text-sm text-zinc-500">{error.message}</p>
        </div>
      </div>
    );
  }
}

/** The wizard on first run or while changing the setup; the workspace otherwise. */
function Screen() {
  useAppData();
  const { setup, editing } = useSession();
  return setup && !editing ? <Workspace /> : <Wizard />;
}

export function App() {
  return (
    <ErrorBoundary>
      <Suspense fallback={<div className="h-screen" />}>
        <Screen />
      </Suspense>
    </ErrorBoundary>
  );
}
