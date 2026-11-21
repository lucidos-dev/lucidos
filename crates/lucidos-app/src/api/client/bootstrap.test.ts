import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { API, ApiError } from './_core';
import { fetchBootstrap } from './bootstrap';
import { engineRestarting } from '../../store/store';

const originalFetch = globalThis.fetch;
let mockFetch: ReturnType<typeof vi.fn>;

function answer(response: Response): void {
  mockFetch.mockImplementation(() => Promise.resolve(response));
}

function sentHeaders(): Headers {
  return new Headers((mockFetch.mock.calls[0][1] as RequestInit).headers);
}

beforeEach(() => {
  mockFetch = vi.fn();
  globalThis.fetch = mockFetch as unknown as typeof fetch;
  engineRestarting.value = false;
});

afterEach(() => {
  globalThis.fetch = originalFetch;
});

describe('fetchBootstrap', () => {
  it('answers the snapshot and its ETag, asking with no validator the first time', async () => {
    const snapshot = { users: { data: { users: [], you: null, team: false } } };
    answer(new Response(JSON.stringify(snapshot), { status: 200, headers: { etag: 'W/"abc"' } }));

    expect(await fetchBootstrap(null)).toEqual({ kind: 'snapshot', snapshot, etag: 'W/"abc"' });
    expect(mockFetch.mock.calls[0][0]).toBe(`${API}/bootstrap`);
    expect(sentHeaders().has('if-none-match')).toBe(false);
  });

  it('sends the ETag it holds, and answers not-modified on a 304', async () => {
    answer(new Response(null, { status: 304 }));

    expect(await fetchBootstrap('W/"abc"')).toEqual({ kind: 'not-modified' });
    expect(sentHeaders().get('if-none-match')).toBe('W/"abc"');
  });

  it('answers route-missing on a 404, which is an engine older than the route', async () => {
    answer(new Response('Not Found', { status: 404 }));

    expect(await fetchBootstrap(null)).toEqual({ kind: 'route-missing' });
  });

  it('throws any other failure, so the caller never falls back to the burst', async () => {
    answer(new Response(JSON.stringify({ error: 'pool exhausted' }), { status: 500 }));

    const error = await fetchBootstrap(null).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).httpCode).toBe(500);
  });
});
