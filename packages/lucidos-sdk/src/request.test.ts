/**
 * The escape hatch sends what it checked, and refuses the rest.
 *
 * `_fetch` is mocked, so these cases are about the classification and the path
 * that reaches the transport. Whether the transport bridges or goes direct is
 * `_fetch`'s own decision, covered by its tests.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { SdkError } from './_fetch';

const sent: Array<{ path: string; init?: RequestInit }> = [];

vi.mock('./_fetch', async () => {
  const actual = await vi.importActual<typeof import('./_fetch')>('./_fetch');
  return {
    ...actual,
    request: (path: string, init?: RequestInit) => {
      sent.push({ path, init });
      return Promise.resolve({ ok: true });
    },
  };
});

beforeEach(() => {
  sent.length = 0;
});

describe('lucidos.request', () => {
  it('sends an allowed call, with its query intact', async () => {
    const { request } = await import('./request');
    await request('/env-vars');
    await request('/notification/read?id=abc', { method: 'POST' });
    expect(sent.map(s => s.path)).toEqual(['/env-vars', '/notification/read?id=abc']);
  });

  it('carries the method and body through untouched', async () => {
    const { request } = await import('./request');
    const body = JSON.stringify({ event_type: 'ReportOpened', payload: { summary: 'x' } });
    await request('/events/emit', { method: 'POST', body });
    expect(sent[0].init?.method).toBe('POST');
    expect(sent[0].init?.body).toBe(body);
  });

  it('refuses a route an app may not reach, and sends nothing', async () => {
    const { request } = await import('./request');
    await expect(request('/credential-value')).rejects.toBeInstanceOf(SdkError);
    await expect(request('/chat/stream', { method: 'POST' })).rejects.toThrow(/may not call/);
    expect(sent).toEqual([]);
  });

  it('refuses a method the route does not open', async () => {
    const { request } = await import('./request');
    await expect(request('/models', { method: 'DELETE' })).rejects.toThrow(/DELETE \/models/);
    expect(sent).toEqual([]);
  });

  it('refuses a traversal on what it resolves to, not on how it is spelled', async () => {
    const { request } = await import('./request');
    await expect(request('/data/%2e%2e/credential-value')).rejects.toThrow(/may not call/);
    expect(sent).toEqual([]);
  });

  it('sends the resolved path, so nothing renormalises after the check', async () => {
    const { request } = await import('./request');
    await request('/data/./artifacts/x.md');
    expect(sent[0].path).toBe('/data/artifacts/x.md');
  });

  it('refuses a path that is not a rooted suffix', async () => {
    const { request } = await import('./request');
    await expect(request('env-vars')).rejects.toThrow(/not a path under/);
    await expect(request('//example.com/env-vars')).rejects.toThrow(/not a path under/);
    expect(sent).toEqual([]);
  });

  it('throws on a suffix that is not a string', async () => {
    const { request } = await import('./request');
    // Synchronous, like `preferences.set` and `data.edit`: a caller passing the
    // wrong type has a bug, and the stack should point at their call.
    expect(() => (request as unknown as (s: unknown) => unknown)(7)).toThrow(TypeError);
  });
});
