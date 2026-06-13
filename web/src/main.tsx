import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './app/App';
import './styles/index.css';

/**
 * Entry. In dev (and unless VITE_ENABLE_MSW=false) MSW intercepts /v1/* so the
 * SPA runs with no daemon. The worker is started before the first render so no
 * request escapes the mock.
 */
async function enableMocking() {
  const enabled =
    import.meta.env.DEV && import.meta.env.VITE_ENABLE_MSW !== 'false';
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
