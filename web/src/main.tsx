import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './app/App';
import './styles/index.css';

/**
 * Entry. MSW intercepts /v1/* only in the `mock` form factor (VITE_TARGET, with
 * DEV defaulting to `mock`) so the SPA runs with no daemon; `web`/`tauri` builds
 * hit a real transport. The worker is started before the first render so no
 * request escapes the mock. VITE_ENABLE_MSW=false stays the explicit escape hatch.
 */
async function enableMocking() {
  const target =
    import.meta.env.VITE_TARGET ?? (import.meta.env.DEV ? 'mock' : 'web');
  const enabled =
    target === 'mock' && import.meta.env.VITE_ENABLE_MSW !== 'false';
  if (!enabled) return;
  const { worker } = await import('./mocks/browser');
  await worker.start({ onUnhandledRequest: 'bypass' });
}

enableMocking().then(() => {
  const root = document.getElementById('root');
  if (!root) throw new Error('root element missing');
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
});
