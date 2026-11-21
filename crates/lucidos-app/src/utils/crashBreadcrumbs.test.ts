import { describe, expect, it } from 'vitest';
import { BreadcrumbStore, recoveredStartup } from './crashBreadcrumbs';

function storage() {
  const values = new Map<string, string>();
  return { getItem: (k: string) => values.get(k) ?? null, setItem: (k: string, v: string) => { values.set(k, v); } };
}
describe('crash breadcrumbs', () => {
  it('persists frame counters and unique host image pixels', () => {
    const disk = storage();
    const store = new BreadcrumbStore(disk, 'a');
    const counts = { frameVideoLoaded: 2, frameVideoPixels: 400, frameImagePixels: 100, uniqueImagePixels: 200 };
    store.add('sample', { ...counts, path: 'private' });
    store.flush();
    expect(store.read()?.samples[0].data).toEqual(counts);
  });
  it('rotates samples and events and survives a new instance', () => {
    const disk = storage();
    const store = new BreadcrumbStore(disk, 'a');
    for (let n = 0; n < 30; n++) store.add('sample', { nodes: n });
    for (let n = 0; n < 30; n++) store.add('media-mount', { count: n });
    store.flush();
    const prior = new BreadcrumbStore(disk, 'a').read();
    expect(prior?.samples).toHaveLength(6);
    expect(prior?.events).toHaveLength(12);
    expect(new BreadcrumbStore(disk, 'b').read()).toBeNull();
  });
  it('ignores corrupt, untrusted and oversized storage', () => {
    const disk = storage();
    const store = new BreadcrumbStore(disk, 'a');
    for (const bad of ['{', '{}', JSON.stringify({ version: 1, samples: ['secret'], events: [] }), 'x'.repeat(5000)]) {
      disk.setItem('lucidos-crash:a', bad);
      expect(store.read()).toBeNull();
    }
  });
  it('rejects stale, future, versioned and secret-bearing records', () => {
    const disk = storage();
    const valid = { version: 1, at: Date.now(), samples: [], events: [] };
    for (const changes of [
      { version: 2 }, { at: Date.now() - 86_400_001 }, { at: Date.now() + 120_000 },
      { events: [{ at: Date.now(), kind: 'stall', data: { appIds: ['app?secret'] } }] },
    ]) {
      disk.setItem('lucidos-crash:a', JSON.stringify({ ...valid, ...changes }));
      expect(new BreadcrumbStore(disk, 'a').read()).toBeNull();
    }
    disk.setItem('lucidos-crash:a', JSON.stringify({ ...valid, secret: 'private' }));
    expect(JSON.stringify(new BreadcrumbStore(disk, 'a').read())).not.toContain('private');
  });
  it('never throws on quota or disabled storage', () => {
    const store = new BreadcrumbStore({ getItem() { throw Error('disabled'); }, setItem() { throw Error('quota'); } }, 'a');
    expect(store.read()).toBeNull();
    expect(() => { store.add('sample', { nodes: 1 }); store.flush(); }).not.toThrow();
  });
  it('keeps a byte budget and refuses arbitrary strings and fields', () => {
    const disk = storage();
    const store = new BreadcrumbStore(disk, 'a');
    for (let n = 0; n < 30; n++) store.add('sample', { nodes: n, secret: 'token', url: 'https://secret', pixels: Infinity });
    store.flush();
    const raw = disk.getItem('lucidos-crash:a')!;
    expect(new TextEncoder().encode(raw).length).toBeLessThanOrEqual(2300);
    expect(raw).not.toContain('secret');
    expect(raw).not.toContain('Infinity');
  });
  it('keeps the newest dense samples under storage and startup byte pressure', () => {
    const disk = storage();
    const store = new BreadcrumbStore(disk, 'a');
    for (let n = 0; n < 15; n++) {
      store.add('sample', { nodes: n, appIds: Array.from({ length: 8 }, (_, id) => `${id}${'a'.repeat(47)}`) });
    }
    store.flush();
    const prior = store.read()!;
    expect(prior.samples.length).toBeLessThan(6);
    expect(prior.samples.slice(-1)[0]?.data.nodes).toBe(14);
    expect(new TextEncoder().encode(disk.getItem('lucidos-crash:a')!).length).toBeLessThanOrEqual(2300);
    const result = recoveredStartup({ kind: 'likely_crash', landing_search: '界'.repeat(1000) }, new BreadcrumbStore(disk, 'a'));
    const recovered = result.breadcrumb as typeof prior;
    expect(recovered.samples.length).toBeLessThan(prior.samples.length);
    expect(recovered.samples.slice(-1)[0]?.data.nodes).toBe(14);
    expect(new TextEncoder().encode(JSON.stringify(result)).length).toBeLessThanOrEqual(3900);
  });
});

describe('startup recovery', () => {
  it('keeps events ahead of samples under a multibyte startup budget', () => {
    const disk = storage();
    const base = { kind: 'likely_crash', landing_search: '界'.repeat(1150) };
    const now = Date.now();
    const event = { at: now - 20, kind: 'media-mount', data: { count: 1, appIds: ['a'.repeat(48)] } };
    const sample = { at: now - 10, kind: 'sample', data: { nodes: 100, appIds: ['b'.repeat(48)] } };
    disk.setItem('lucidos-crash:a', JSON.stringify({ version: 1, at: now, samples: [sample], events: [event, { ...event, at: now }] }));
    const result = recoveredStartup(base, new BreadcrumbStore(disk, 'a'));
    expect(new TextEncoder().encode(JSON.stringify(result)).length).toBeLessThanOrEqual(3900);
    expect(result).toHaveProperty('breadcrumb.samples.length', 0);
    expect(result).toHaveProperty('breadcrumb.events.length', 2);
  });
  it('attaches prior evidence only to crash startup and clears it for the new document', () => {
    const disk = storage();
    const old = new BreadcrumbStore(disk, 'a');
    old.add('sample', { video: 3 }); old.flush();
    const result = recoveredStartup({ kind: 'likely_crash' }, new BreadcrumbStore(disk, 'a'));
    expect(result).toHaveProperty('breadcrumb.samples.0.data.video', 3);
    expect(new TextEncoder().encode(JSON.stringify(result)).length).toBeLessThan(4096);
    expect(new BreadcrumbStore(disk, 'a').read()?.samples).toHaveLength(0);
    old.flush();
    expect(recoveredStartup({ kind: 'cold' }, new BreadcrumbStore(disk, 'a'))).not.toHaveProperty('breadcrumb');
  });
});

it('validates the writer build and retains open admission marks ahead of samples', () => {
  const disk = storage();
  const store = new BreadcrumbStore(disk, 'a', 'abcdef123456');
  store.add('wake', {});
  store.add('admission', { phase: 4, path: '/private', threadId: 'private', text: 'private' });
  store.flush();
  expect(new BreadcrumbStore(disk, 'a').read()).toMatchObject({
    build: 'abcdef123456', events: [expect.anything(), { at: expect.any(Number), kind: 'admission', data: { phase: 4 } }],
  });
  for (let n = 0; n < 30; n++) store.add('sample', { nodes: n, appIds: Array(8).fill('a'.repeat(48)) });
  store.flush();
  expect(store.read()?.events.map(e => e.kind)).toEqual(['wake', 'admission']);
  const result = recoveredStartup({ kind: 'likely_crash', padding: 'x'.repeat(3400) }, new BreadcrumbStore(disk, 'a'));
  expect(result).toHaveProperty('breadcrumb.events.1.kind', 'admission');
  expect(new TextEncoder().encode(JSON.stringify(result)).length).toBeLessThanOrEqual(3900);
  for (const build of ['/private', 'thread-id', 'hello world', 'f'.repeat(65)]) {
    disk.setItem('lucidos-crash:a', JSON.stringify({ version: 1, at: Date.now(), build, samples: [], events: [] }));
    expect(store.read()).toBeNull();
  }
  disk.setItem('lucidos-crash:a', JSON.stringify({
    version: 1, at: Date.now(), samples: [], events: [{ at: Date.now(), kind: 'admission', data: { path: '/private' } }],
  }));
  expect(store.read()).toBeNull();
});

it('uses dev for an unstamped bundle and accepts legacy records without a build', () => {
  const disk = storage();
  const store = new BreadcrumbStore(disk, 'a', '__LUCIDOS_BUILD_ID__');
  store.flush();
  expect(store.read()?.build).toBe('dev');
  disk.setItem('lucidos-crash:a', JSON.stringify({ version: 1, at: Date.now(), samples: [], events: [] }));
  expect(store.read()).not.toBeNull();
});

it('keeps the latest resume state through event churn and byte pressure', () => {
  const disk = storage();
  const store = new BreadcrumbStore(disk, 'a');
  store.add('wake', {});
  store.add('stream', { state: 1 });
  store.add('admission', { phase: 0 });
  store.add('admission', { phase: 4 });
  for (let n = 0; n < 40; n++) {
    store.add('iframe-load', { count: n, appIds: Array(8).fill('a'.repeat(48)) });
    store.add('sample', { nodes: n });
  }
  store.flush();
  const prior = new BreadcrumbStore(disk, 'a').read()!;
  expect(prior.events).toEqual(expect.arrayContaining([
    expect.objectContaining({ kind: 'wake' }),
    expect.objectContaining({ kind: 'stream', data: { state: 1 } }),
    expect.objectContaining({ kind: 'admission', data: { phase: 4 } }),
    expect.objectContaining({ kind: 'iframe-load', data: expect.objectContaining({ count: 39 }) }),
  ]));
  expect(prior.events.length).toBeLessThanOrEqual(12);
  expect(new TextEncoder().encode(disk.getItem('lucidos-crash:a')!).length).toBeLessThanOrEqual(2300);
});

it('keeps the document start and latest hide through event churn, and omits sample zeros', () => {
  const disk = storage();
  const store = new BreadcrumbStore(disk, 'a');
  store.add('hide', { state: 2 });
  store.add('worker', { state: 1 });
  for (let n = 0; n < 20; n++) store.add('media-mount', { count: n });
  store.add('sample', { nodes: 1, video: 0 });
  store.flush();
  expect(store.read()?.start).toBeGreaterThan(0);
  expect(store.read()?.events).toContainEqual(expect.objectContaining({ kind: 'hide', data: { state: 2 } }));
  expect(store.read()?.events).toContainEqual(expect.objectContaining({ kind: 'worker', data: { state: 1 } }));
  expect(store.read()?.samples[0].data).toEqual({ nodes: 1 });
});
