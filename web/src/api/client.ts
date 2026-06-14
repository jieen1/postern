/**
 * Typed control-plane fetch client.
 *
 * The SPA only ever speaks the `/v1/*` HTTP/JSON contract (transport-agnostic:
 * in dev MSW intercepts; in prod a local bridge proxies to control.sock). This
 * module centralizes:
 *  - error normalization into `ApiError` (fail-closed: any non-2xx throws),
 *  - 409 optimistic-lock conflicts into a distinguishable `ConflictError`,
 *  - pagination params (default 20, clamped to [1,200]),
 *  - id discipline (responses are read as text→JSON without reviving id
 *    strings to numbers; we never `JSON.parse` ids into Number).
 */

import {
  PAGE_DEFAULT_SIZE,
  PAGE_MAX_SIZE,
  PAGE_MIN_SIZE,
  type ApiErrorBody,
  type PageQuery,
} from './types';
import { selectTransport } from './transport';

export const API_BASE = '/v1';

/** Any non-2xx control-plane response surfaces as this (fail-closed). */
export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  constructor(status: number, code: string, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
  }
}

/** Optimistic-lock conflict (HTTP 409) — caller prompts "refresh & retry". */
export class ConflictError extends ApiError {
  constructor(code: string, message: string) {
    super(409, code, message);
    this.name = 'ConflictError';
  }
}

function isErrorBody(value: unknown): value is ApiErrorBody {
  return (
    typeof value === 'object' &&
    value !== null &&
    'error' in value &&
    typeof (value as { error: unknown }).error === 'object'
  );
}

function parseBody(text: string): unknown {
  if (text.length === 0) return undefined;
  // Parsed with the default reviver — id fields are already JSON strings in the
  // contract, so they stay strings; we never coerce them to Number.
  return JSON.parse(text) as unknown;
}

async function request<T>(
  method: string,
  path: string,
  body?: unknown,
): Promise<T> {
  const r = await selectTransport().send(method, path, body);

  if (!r.ok) {
    let parsed: unknown;
    try {
      parsed = parseBody(r.text);
    } catch {
      parsed = undefined;
    }
    const code = isErrorBody(parsed) ? parsed.error.code : `http_${r.status}`;
    const message = isErrorBody(parsed)
      ? parsed.error.message
      : `请求失败 (${r.status})`;
    if (r.status === 409) throw new ConflictError(code, message);
    throw new ApiError(r.status, code, message);
  }

  return parseBody(r.text) as T;
}

/** Clamp pagination to the legal range (mirrors postern-core PageQuery::clamp). */
export function clampPage(q: Partial<PageQuery>): PageQuery {
  const page_no = Math.max(1, Math.trunc(q.page_no ?? 1));
  const rawSize = Math.trunc(q.page_size ?? PAGE_DEFAULT_SIZE);
  const page_size = Math.min(PAGE_MAX_SIZE, Math.max(PAGE_MIN_SIZE, rawSize));
  return { page_no, page_size };
}

/** Build a query string from page params + arbitrary string filters. */
export function buildQuery(
  page: Partial<PageQuery>,
  filters: Record<string, string | undefined> = {},
): string {
  const clamped = clampPage(page);
  const params = new URLSearchParams();
  params.set('page_no', String(clamped.page_no));
  params.set('page_size', String(clamped.page_size));
  for (const [k, v] of Object.entries(filters)) {
    if (v !== undefined && v !== '') params.set(k, v);
  }
  return params.toString();
}

export const http = {
  get: <T>(path: string) => request<T>('GET', path),
  post: <T>(path: string, body?: unknown) => request<T>('POST', path, body),
};
