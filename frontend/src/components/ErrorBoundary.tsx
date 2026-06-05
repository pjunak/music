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
 *  app. This matters here beyond the usual reasons: `index.html` ships a legacy
 *  `tv-mode.js` that claims `#root` whenever React hasn't populated it within
 *  500 ms. So an uncaught render error doesn't just white-screen — it unmounts
 *  the tree, `#root` goes empty, and the TV fallback silently takes over,
 *  which *looks* like the app "redirected to TV mode". Keeping a fallback
 *  mounted here means `#root` is never empty, so that masquerade can't happen
 *  and the operator gets a real, recoverable error instead. */
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
