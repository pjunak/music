import { Component } from "react";
import type { ErrorInfo, ReactNode } from "react";

interface Props {
  children: ReactNode;
  /** Custom fallback. Defaults to a centered error card with a Reload button. */
  fallback?: (error: Error, reset: () => void) => ReactNode;
}

interface State {
  error: Error | null;
}

/** Catches render-time crashes so a single bad component can't blank the whole
 *  app — the operator gets a real, recoverable error card instead of an empty
 *  `#root`. (Historically an empty `#root` was worse than a blank screen: the
 *  legacy `tv-mode.js` fallback claimed it on a 500 ms timer, so a render crash
 *  *looked* like the app "redirected to TV mode." That vector is now closed at
 *  the source — tv-mode.js ships as `<script nomodule>`, so it never loads on a
 *  module-capable browser and can't race React — but keeping `#root` populated
 *  on a crash is still the right behaviour, so this stays.) */
export class ErrorBoundary extends Component<Props, State> {
  override state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  override componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error("[ErrorBoundary] render crash:", error, info.componentStack);
  }

  reset = (): void => {
    this.setState({ error: null });
  };

  override render(): ReactNode {
    const { error } = this.state;
    if (error !== null) {
      if (this.props.fallback) return this.props.fallback(error, this.reset);
      return (
        <div className="route-status error-boundary" role="alert">
          <h2>Something went wrong</h2>
          <p className="muted">{error.message || "Unexpected error."}</p>
          <button
            type="button"
            className="btn-primary"
            onClick={() => window.location.reload()}
          >
            Reload
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}
