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
 *  `compat-mode.js` fallback (then called tv-mode.js) claimed it on a 500 ms
 *  paint timer, so a render crash *looked* like the app "redirected to
 *  compatibility mode." That vector is closed: the index.html boot watchdog
 *  only loads compat-mode.js when the bundle fails to
 *  *execute* (sets `window.__SPA_BOOTED__`) — a render crash boots fine and
 *  sets the flag, so the watchdog stands down and this boundary handles it.
 *  Keeping `#root` populated on a crash is what makes that true, so this stays.) */
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
