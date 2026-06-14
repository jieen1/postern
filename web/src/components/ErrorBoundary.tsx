import { Component, type ReactNode } from 'react';

interface Props {
  children: ReactNode;
}
interface State {
  error: Error | null;
}

/**
 * Route-level error boundary: a single crashing page (e.g. a backend response
 * whose shape the page didn't expect during front/back integration) shows a
 * contained error here instead of blanking the whole app. The shell + nav stay
 * usable; the boundary is keyed by route in App so navigating elsewhere resets
 * it. Never swallows silently — the error is also logged to the console.
 */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error) {
    // eslint-disable-next-line no-console
    console.error('[page error]', error);
  }

  render() {
    if (this.state.error) {
      return (
        <div
          role="alert"
          className="m-6 rounded-card border border-deny/40 bg-surface-2 p-6"
        >
          <h2 className="text-lg font-semibold text-deny">此页加载出错</h2>
          <p className="mt-2 text-sm text-text-muted">
            页面渲染时抛出异常——多见于后端返回的数据形状与前端契约不一致（前后端联调期）。
            外壳与导航仍可用，切到其他页可恢复。
          </p>
          <pre className="mt-3 overflow-auto rounded-badge bg-surface-1 p-3 font-mono text-xs text-text">
            {this.state.error.message}
          </pre>
        </div>
      );
    }
    return this.props.children;
  }
}
