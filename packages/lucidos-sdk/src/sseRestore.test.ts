import { afterEach, expect, it, vi } from 'vitest';

const transport = vi.hoisted(() => ({ handlers: null as null | {
  onOpen(): void; onFrame(data: string): void;
} }));
vi.mock('./eventStream', () => ({
  eventStreamTargets: () => ({}),
  openEventStream: (_targets: unknown, handlers: typeof transport.handlers) => {
    transport.handlers = handlers;
    return { close: vi.fn() };
  },
}));
vi.mock('./_bridge', async (importOriginal) => ({
  ...await importOriginal<typeof import('./_bridge')>(), isBridged: () => false,
}));
afterEach(() => { vi.unstubAllGlobals(); vi.resetModules(); });

it('boots standalone HTTP from onOpen only after a restored hello', async () => {
  const generation = '11111111-1111-4111-8111-111111111111';
  const fetch = vi.fn(async (_url: unknown, _init?: RequestInit) => new Response('{}'));
  vi.stubGlobal('fetch', fetch);
  const { sse } = await import('./sse');
  const { request } = await import('./_fetch');
  let loaded: Promise<unknown> | undefined;
  sse.onOpen(() => { loaded = request('/triggers'); });
  transport.handlers!.onOpen();
  expect(fetch).not.toHaveBeenCalled();
  transport.handlers!.onFrame(JSON.stringify({ type: 'StreamHello', data: { restore_generation: generation } }));
  await loaded;
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(new Headers(fetch.mock.calls[0]?.[1]?.headers).get('x-lucidos-restore-generation')).toBe(generation);
  transport.handlers!.onOpen();
  transport.handlers!.onFrame(JSON.stringify({ type: 'StreamHello', data: {
    restore_generation: '22222222-2222-4222-8222-222222222222',
  } }));
  expect(fetch).toHaveBeenCalledTimes(1);
  await expect(request('/triggers')).rejects.toThrow(/restored/i);
  sse.disconnect();
});
