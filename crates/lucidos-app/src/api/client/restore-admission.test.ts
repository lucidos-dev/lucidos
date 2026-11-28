import { afterEach, expect, it, vi } from 'vitest';
import { json, mutatingFetch, mutatingFetchIdempotent, retryTransient, text } from './_core';
import { consumeProvisionalReads, installRestoreAdmission } from '../../utils/restoreAdmission';

import { trackDeviceRegistration } from '../../utils/deviceRegistration';
import { observeSourceGeneration } from '../../utils/restoreSourceGeneration';

const originalFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = originalFetch;
  observeSourceGeneration(null);
  installRestoreAdmission({ ready: () => true, phase: () => 'admitted', wait: async () => {} });
});

function closedGate(provisional = false) {
  let ready = false;
  let release!: () => void;
  const wait = new Promise<void>((resolve) => { release = resolve; });
  installRestoreAdmission({ ready: () => ready, phase: () => ready ? 'admitted' : 'generation', wait: () => wait,
    provisional: () => provisional });
  return () => { ready = true; release(); };
}

const target = '00000000-0000-4000-8000-0000000000aa';

it('answers a never-admitted page\'s thread and event reads while every other request waits', async () => {
  const open = closedGate(true);
  const fetch = vi.fn<typeof globalThis.fetch>(async () => new Response('{}'));
  globalThis.fetch = fetch;
  const held = [
    json('/api/v1/threads'),
    json('/api/v1/threads?focused=x'),
    json('/api/v1/threads/archived-count'),
    json(`/api/v1/threads/${target}/children`),
    json(`/api/v1/threads/${target}`, { method: 'POST' }),
    mutatingFetch(`/api/v1/threads/${target}`, { method: 'PUT' }),
    mutatingFetch(`/api/v1/threads/${target}/compose`, { method: 'PUT' }),
    mutatingFetch(`/api/v1/threads/${target}`, { method: 'DELETE' }),
  ];
  await expect(json(`/dev/api/v1/threads/${target}`)).resolves.toEqual({});
  await expect(json(`/api/v1/threads/${target}/events?limit=50`)).resolves.toEqual({});
  expect(fetch.mock.calls.map((c) => c[0])).toEqual([`/dev/api/v1/threads/${target}`, `/api/v1/threads/${target}/events?limit=50`]);
  expect(consumeProvisionalReads()).toBe(true);
  expect(consumeProvisionalReads()).toBe(false);
  open();
  await Promise.all(held);
  expect(fetch).toHaveBeenCalledTimes(10);
});

it('starts a mutation\'s deadline after admission, so a send held by a storage stall still goes out', async () => {
  vi.useFakeTimers();
  try {
    const open = closedGate();
    const fetch = vi.fn<typeof globalThis.fetch>(async () => new Response('{}'));
    globalThis.fetch = fetch;
    const sends = [
      mutatingFetch('/api/v1/chat/stream', { method: 'POST', timeoutMs: 1_000 }),
      mutatingFetchIdempotent('/api/v1/threads', { method: 'POST', timeoutMs: 1_000 }),
    ];
    await vi.advanceTimersByTimeAsync(60_000);
    expect(fetch).not.toHaveBeenCalled();
    open();
    await Promise.all(sends);
    expect(fetch).toHaveBeenCalledTimes(2);
    for (const [, init] of fetch.mock.calls) {
      expect(init?.signal?.aborted).toBe(false);
      expect(init).not.toHaveProperty('timeoutMs');
    }
  } finally {
    vi.useRealTimers();
  }
});

it('re-stamps a mutation\'s fired deadline as a timeout, as WebKit reports it as a cancel', async () => {
  globalThis.fetch = vi.fn((_url: RequestInfo | URL, init?: RequestInit) => new Promise<Response>((_resolve, reject) => {
    init?.signal?.addEventListener('abort', () => reject(new DOMException('Fetch is aborted', 'AbortError')));
  }));
  await expect(mutatingFetch('/api/v1/chat/stream', { method: 'POST', timeoutMs: 5 }))
    .rejects.toMatchObject({ name: 'TimeoutError' });
});

it('holds the same thread read on a page that was admitted before', async () => {
  const open = closedGate(false);
  const fetch = vi.fn<typeof globalThis.fetch>(async () => new Response('{}'));
  globalThis.fetch = fetch;
  const read = json(`/api/v1/threads/${target}`);
  await Promise.resolve(); await Promise.resolve();
  expect(fetch).not.toHaveBeenCalled();
  open();
  await read;
  expect(consumeProvisionalReads()).toBe(false);
});

it('does not refuse a provisional read whose admission landed on a generation it could not prove', async () => {
  const open = closedGate(true);
  let answer!: (response: Response) => void;
  globalThis.fetch = vi.fn(() => new Promise<Response>((resolve) => { answer = resolve; }));
  const read = json(`/api/v1/threads/${target}`);
  await vi.waitFor(() => expect(answer).toBeTypeOf('function'));
  observeSourceGeneration(restored, false);
  open();
  answer(new Response('{"thread_id":"t"}'));
  await expect(read).resolves.toEqual({ thread_id: 't' });
});

it('does not refuse the same read wrapped in a transient retry', async () => {
  const open = closedGate(true);
  let answer!: (response: Response) => void;
  globalThis.fetch = vi.fn(() => new Promise<Response>((resolve) => { answer = resolve; }));
  const read = retryTransient(() => json(`/api/v1/threads/${target}/events`));
  await vi.waitFor(() => expect(answer).toBeTypeOf('function'));
  observeSourceGeneration(restored, false);
  open();
  answer(new Response('{"events":[]}'));
  await expect(read).resolves.toEqual({ events: [] });
});

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
