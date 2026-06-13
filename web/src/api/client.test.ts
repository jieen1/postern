import { describe, expect, it } from 'vitest';
import { http as msw, HttpResponse } from 'msw';
import { server } from '../mocks/server';
import { http, ApiError, ConflictError, clampPage, buildQuery } from './client';

describe('client pagination + error handling', () => {
  it('clampPage mirrors the backend clamp (default 20, [1,200])', () => {
    expect(clampPage({})).toEqual({ page_no: 1, page_size: 20 });
    expect(clampPage({ page_no: 0, page_size: 9999 })).toEqual({ page_no: 1, page_size: 200 });
    expect(clampPage({ page_no: 3, page_size: 1 })).toEqual({ page_no: 3, page_size: 1 });
  });

  it('buildQuery always emits clamped page params and drops empty filters', () => {
    const qs = buildQuery({ page_no: 0, page_size: 5000 }, { kind: 'request', decision: '' });
    const params = new URLSearchParams(qs);
    expect(params.get('page_no')).toBe('1');
    expect(params.get('page_size')).toBe('200');
    expect(params.get('kind')).toBe('request');
    expect(params.has('decision')).toBe(false);
  });

  it('maps a 409 to ConflictError (optimistic-lock refresh prompt)', async () => {
    server.use(
      msw.post('/v1/roles', () =>
        HttpResponse.json(
          { error: { code: 'version_conflict', message: '他人已改' } },
          { status: 409 },
        ),
      ),
    );
    await expect(http.post('/roles', {})).rejects.toBeInstanceOf(ConflictError);
  });

  it('maps a non-2xx to a fail-closed ApiError carrying the verbatim message', async () => {
    server.use(
      msw.get('/v1/roles', () =>
        HttpResponse.json({ error: { code: 'boom', message: '炸了' } }, { status: 500 }),
      ),
    );
    await expect(http.get('/roles')).rejects.toMatchObject({
      name: 'ApiError',
      status: 500,
      code: 'boom',
      message: '炸了',
    });
  });

  it('does not coerce snowflake id strings to numbers on parse', async () => {
    const BIG = '7300000000000000123';
    server.use(msw.get('/v1/probe', () => HttpResponse.json({ id: BIG })));
    const body = await http.get<{ id: string }>('/probe');
    expect(body.id).toBe(BIG);
    expect(typeof body.id).toBe('string');
  });
});

describe('ApiError shape', () => {
  it('is an Error subclass with status/code', () => {
    const e = new ApiError(404, 'not_found', 'x');
    expect(e).toBeInstanceOf(Error);
    expect(e.status).toBe(404);
    expect(e.code).toBe('not_found');
  });
});
