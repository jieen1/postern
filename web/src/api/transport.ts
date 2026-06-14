/**
 * Transport seam — the single replaceable point where a control-plane request
 * is actually dispatched.
 *
 * Everything above this file (endpoints → hooks → pages) speaks the same
 * `/v1/*` HTTP/JSON contract regardless of form factor; `client.ts` owns error
 * normalization and id-safe body parsing. This module owns only "send the bytes
 * and hand back a raw response", so the same UI can run in two shells:
 *
 *  - **web / mock** (`httpTransport`): a real `fetch` to `${API_BASE}${path}`.
 *    In dev/test MSW intercepts exactly this fetch, so one implementation serves
 *    both the standalone web build and every mocked test.
 *  - **tauri** (`tauriTransport`): hands the request to the Tauri Rust side via
 *    the `control_request` command. The Tauri shell does not exist yet, so this
 *    implementation must never introduce a build/compile-time dependency on
 *    `@tauri-apps/*`; `invoke` is resolved off the runtime `window` globals and
 *    is only ever selected when `VITE_TARGET=tauri`.
 *
 * `selectTransport()` picks the implementation from `VITE_TARGET`
 * (default: DEV→`'mock'`, PROD→`'web'`).
 */

import { API_BASE } from './client';

/** A transport's raw, un-normalized reply. `client.ts` turns this into T or throws. */
export interface RawResponse {
  status: number;
  ok: boolean;
  text: string;
}

/**
 * The replaceable dispatch seam. `path` is the `/v1`-relative path (identical to
 * what `http.get(path)` receives today); the implementation prefixes `API_BASE`.
 */
export interface Transport {
  send(method: string, path: string, body?: unknown): Promise<RawResponse>;
}

/**
 * Web/mock transport: a plain `fetch`. MSW intercepts this exact request in
 * dev/test, so it backs both the standalone web build and the mocked suites.
 */
export const httpTransport: Transport = {
  async send(method, path, body) {
    const res = await fetch(`${API_BASE}${path}`, {
      method,
      headers:
        body === undefined ? undefined : { 'content-type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    return { status: res.status, ok: res.ok, text: await res.text() };
  },
};

/** Shape of the Tauri `invoke` we resolve off the runtime window globals. */
type TauriInvoke = (
  cmd: string,
  args?: Record<string, unknown>,
) => Promise<unknown>;

/**
 * Resolve Tauri's `invoke` from the runtime globals the shell injects, without
 * any static import of `@tauri-apps/*`. Returns undefined when not running
 * inside a Tauri webview (i.e. every web/mock build), so this file never causes
 * a compile-time or load-time failure outside Tauri.
 */
function resolveTauriInvoke(): TauriInvoke | undefined {
  const w = globalThis as {
    __TAURI__?: { core?: { invoke?: TauriInvoke }; invoke?: TauriInvoke };
    __TAURI_INTERNALS__?: { invoke?: TauriInvoke };
  };
  return (
    w.__TAURI__?.core?.invoke ??
    w.__TAURI__?.invoke ??
    w.__TAURI_INTERNALS__?.invoke
  );
}

/**
 * Tauri transport: forwards the request to the Rust side via the
 * `control_request` command (the Rust handler is implemented in the Tauri-shell
 * wave; this is only the front-end half). The Rust side returns a
 * `{status, ok, text}` envelope mirroring `RawResponse`.
 */
export const tauriTransport: Transport = {
  async send(method, path, body) {
    const invoke = resolveTauriInvoke();
    if (!invoke) {
      throw new Error(
        'tauriTransport selected but Tauri invoke is unavailable (not running inside a Tauri webview)',
      );
    }
    const raw = await invoke('control_request', {
      method,
      path: `${API_BASE}${path}`,
      body,
    });
    return raw as RawResponse;
  },
};

/**
 * Choose the transport from `VITE_TARGET`:
 *  - `'tauri'` → `tauriTransport`
 *  - `'web'` / `'mock'` → `httpTransport` (they differ only in whether MSW runs)
 *  - unset → DEV defaults to `'mock'`, PROD to `'web'` (both `httpTransport`)
 */
export function selectTransport(): Transport {
  const target =
    import.meta.env.VITE_TARGET ?? (import.meta.env.DEV ? 'mock' : 'web');
  return target === 'tauri' ? tauriTransport : httpTransport;
}
