import { Component, Suspense, type ReactNode } from "react";
import { Wizard } from "./features/setup-wizard/Wizard";
import i18n from "./i18n";

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

export function App() {
  return (
    <ErrorBoundary>
      <Suspense fallback={<div className="h-screen" />}>
        <Wizard />
      </Suspense>
    </ErrorBoundary>
  );
}
