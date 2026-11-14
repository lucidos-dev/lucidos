import { afterEach, expect, it, vi } from 'vitest';
import { json, mutatingFetch, mutatingFetchIdempotent, retryTransient, text } from './_core';
import { installRestoreAdmission } from '../../utils/restoreAdmission';

import { trackDeviceRegistration } from '../../utils/deviceRegistration';
import { observeSourceGeneration } from '../../utils/restoreSourceGeneration';

const originalFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = originalFetch;
  observeSourceGeneration(null);
  installRestoreAdmission({ ready: () => true, phase: () => 'admitted', wait: async () => {} });
});

function closedGate() {
  let ready = false;
  let release!: () => void;
  const wait = new Promise<void>((resolve) => { release = resolve; });
  installRestoreAdmission({ ready: () => ready, phase: () => ready ? 'admitted' : 'awaiting-hello', wait: () => wait });
  return () => { ready = true; release(); };
}

it('holds startup reads and compose mutations, while allowing the health probe', async () => {
  const open = closedGate();
  const fetch = vi.fn<typeof globalThis.fetch>(async () => new Response('{}'));
  globalThis.fetch = fetch;
  const threads = json('/api/v1/threads');
  const compose = mutatingFetch('/api/v1/threads/old/compose', { method: 'PUT' });
  await json('/api/v1/health');
  expect(fetch.mock.calls.map((c) => c[0])).toEqual(['/api/v1/health']);
  open();
  await Promise.all([threads, compose]);
  expect(fetch).toHaveBeenCalledTimes(3);
});

it('holds an in-flight read when admission closes before its response arrives', async () => {
  let answer!: (response: Response) => void;
  globalThis.fetch = vi.fn(() => new Promise<Response>((resolve) => { answer = resolve; }));
  const read = json('/api/v1/threads');
  // fetchWithDefaults awaits the engine-ready check before issuing the read.
  await vi.waitFor(() => expect(answer).toBeTypeOf('function'));
  const open = closedGate();
  let applied = false;
  const result = read.then(() => { applied = true; });
  answer(new Response('{}'));
  await Promise.resolve(); await Promise.resolve();
  expect(applied).toBe(false);
  open();
  await result;
  expect(applied).toBe(true);
});

const restored = '00000000-0000-4000-8000-000000000001';

it('rejects compose intent held across a restore before sending it', async () => {
  const open = closedGate();
  const fetch = vi.fn<typeof globalThis.fetch>();
  globalThis.fetch = fetch;
  const request = mutatingFetch('/api/v1/threads/old/compose', { method: 'PUT' });
  const rejected = expect(request).rejects.toMatchObject({ httpCode: 409 });
  observeSourceGeneration(restored);
  open();
  await rejected;
  expect(fetch).not.toHaveBeenCalled();
});

it('does not retry an idempotent mutation in a different generation', async () => {
  const fetch = vi.fn<typeof globalThis.fetch>(async () => {
    observeSourceGeneration(restored);
    throw new TypeError('Load failed');
  });
  globalThis.fetch = fetch;
  await expect(mutatingFetchIdempotent('/api/v1/threads/old/read', { method: 'POST' }))
    .rejects.toMatchObject({ httpCode: 409 });
  expect(fetch).toHaveBeenCalledTimes(1);
});

it.each(['json', 'text'] as const)('rejects a stale %s body consumed across restore', async (format) => {
  let finish!: (value: string) => void;
  const body = new Promise<string>((resolve) => { finish = resolve; });
  const response = new Response('{}');
  vi.spyOn(response, format).mockImplementation(() => body);
  globalThis.fetch = vi.fn(async () => response);
  const request = format === 'json' ? json('/api/v1/threads') : text('/api/v1/files');
  const rejected = expect(request).rejects.toMatchObject({ httpCode: 409 });
  await vi.waitFor(() => expect(response[format]).toHaveBeenCalled());
  observeSourceGeneration(restored);
  finish('{}');
  await rejected;
});

it('does not relabel a transient read retry after restore', async () => {
  const request = vi.fn(async () => {
    observeSourceGeneration(restored);
    throw new TypeError('Load failed');
  });
  await expect(retryTransient(request)).rejects.toMatchObject({ httpCode: 409 });
  expect(request).toHaveBeenCalledTimes(1);
});

it('rejects mutation intent crossing restore while device registration is pending', async () => {
  let register!: () => void;
  const registration = trackDeviceRegistration(new Promise<void>((resolve) => { register = resolve; }));
  const fetch = vi.fn<typeof globalThis.fetch>();
  globalThis.fetch = fetch;
  const request = mutatingFetch('/api/v1/threads/old/compose', { method: 'PUT' });
  const rejected = expect(request).rejects.toMatchObject({ httpCode: 409 });
  observeSourceGeneration(restored);
  register();
  await registration;
  await rejected;
  expect(fetch).not.toHaveBeenCalled();
});

// The explicit original-generation sentinel distinguishes pre-restore pages
// from clients that did not implement the protocol.
it.each([null, restored])('stamps the invocation generation %s on reads and mutations', async (generation) => {
  observeSourceGeneration(generation);
  const fetch = vi.fn<typeof globalThis.fetch>(async () => new Response('{}'));
  globalThis.fetch = fetch;
  await json('/api/v1/threads');
  await mutatingFetch('/api/v1/threads/t/compose', { method: 'PUT',
    headers: new Headers({ 'X-Lucidos-Restore-Generation': 'forged', 'x-example': 'kept' }) });
  for (const [, init] of fetch.mock.calls) {
    expect(new Headers(init?.headers).get('x-lucidos-restore-generation')).toBe(generation ?? 'none');
  }
  expect(new Headers(fetch.mock.calls[1][1]?.headers).get('x-example')).toBe('kept');
});

it('preserves the generation header over a same-generation idempotent retry', async () => {
  observeSourceGeneration(restored);
  const fetch = vi.fn<typeof globalThis.fetch>()
    .mockRejectedValueOnce(new TypeError('Load failed'))
    .mockResolvedValueOnce(new Response('{}'));
  globalThis.fetch = fetch;
  await mutatingFetchIdempotent('/api/v1/threads/t/read', { method: 'POST' });
  expect(fetch).toHaveBeenCalledTimes(2);
  for (const [, init] of fetch.mock.calls) {
    expect(new Headers(init?.headers).get('x-lucidos-restore-generation')).toBe(restored);
  }
});

// A page loading after a restore issues its startup reads before the first
// hello. That admission only succeeds when the generation matches the page's
// durable baseline (a change reloads instead), so those reads belong to it.
it('admits startup reads issued before the first hello at the admitted generation', async () => {
  vi.resetModules();
  const core = await import('./_core');
  const admission = await import('../../utils/restoreAdmission');
  const origin = await import('../../utils/restoreSourceGeneration');
  let ready = false;
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  admission.installRestoreAdmission({ ready: () => ready, phase: () => ready ? 'admitted' : 'awaiting-hello', wait: () => gate });
  const fetch = vi.fn<typeof globalThis.fetch>(async () => new Response('{}'));
  globalThis.fetch = fetch;
  const read = core.json('/api/v1/threads');
  const compose = core.mutatingFetch('/api/v1/threads/t/compose', { method: 'PUT' });
  origin.observeSourceGeneration(restored);
  ready = true;
  release();
  await expect(read).resolves.toEqual({});
  await expect(compose).resolves.toBeInstanceOf(Response);
  for (const [, init] of fetch.mock.calls) {
    expect(new Headers(init?.headers).get('x-lucidos-restore-generation')).toBe(restored);
  }
});
