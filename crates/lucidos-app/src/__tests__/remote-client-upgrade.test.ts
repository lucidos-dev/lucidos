import { describe, expect, it, vi } from 'vitest';
import upgrade from '../../public/remote-client-upgrade.js?raw';

function run(registrations: object[], entries: string[], fail = false, concurrent = false) {
  const deleted: string[] = [];
  const document = { cookie: '' };
  const location = { origin: 'https://gateway.example', protocol: 'https:', search: '', hash: '', replace: vi.fn() };
  const window: { lucidosRemoteClientReady?: Promise<void> } = {};
  const cache = {
    match: async () => undefined,
    keys: async () => entries.map(url => ({ url })),
    delete: async (request: { url: string }) => { deleted.push(request.url); return !concurrent; },
  };
  let enumerations = 0;
  const navigator = { serviceWorker: { getRegistrations: async () => concurrent && enumerations++ > 0 ? [] : registrations } };
  const caches = { keys: async () => ['shared-cache'], open: async () => { if (fail) throw new Error('cache unavailable'); return cache; } };
  new Function('window', 'navigator', 'caches', 'document', 'location', upgrade)(window, navigator, caches, document, location);
  return { ready: window.lucidosRemoteClientReady!, deleted, document, location };
}

describe('trusted remote client upgrade', () => {
  it('preserves only A-owned workers whose script and scope agree', async () => {
    const trusted = vi.fn();
    const foreign = vi.fn(async () => true);
    const r = run([
      {scope: 'https://gateway.example/work@peer/', active: {scriptURL: 'https://gateway.example/~/sw.js?workspace=work%40peer'}, unregister: trusted},
      {scope: 'https://gateway.example/other@peer/', active: {scriptURL: 'https://gateway.example/~/sw.js?workspace=work%40peer'}, unregister: foreign},
    ], []);
    await r.ready;
    expect(trusted).not.toHaveBeenCalled();
    expect(foreign).toHaveBeenCalledOnce();
  });
  it('accepts a registration and entry already removed by another trusted tab', async () => {
    const r = run([{ scope: 'https://gateway.example/work@peer/', unregister: async () => false }], ['https://gateway.example/work@peer/'], false, true);
    await r.ready;
    expect(r.document.cookie).toContain('lucidos-remote-client=1');
  });
  it('retires only remote registrations and remote entries in shared caches before marking ready', async () => {
    const remote = vi.fn(async () => true);
    const local = vi.fn();
    const r = run([
      { scope: 'https://gateway.example/work@peer/', unregister: remote },
      { scope: 'https://gateway.example/work/', unregister: local },
      { scope: 'https://gateway.example/~/', unregister: local },
    ], ['https://gateway.example/work@peer/', 'https://gateway.example/work@peer/assets/old.js', 'https://gateway.example/work/data/keep.png']);
    expect(r.document.cookie).toBe('');
    await r.ready;
    expect(remote).toHaveBeenCalledOnce();
    expect(local).not.toHaveBeenCalled();
    expect(r.deleted).toEqual(['https://gateway.example/work@peer/', 'https://gateway.example/work@peer/assets/old.js']);
    expect(r.document.cookie).toContain('lucidos-remote-client=1');
  });

  it('retires a remote workspace whose valid slug holds consecutive hyphens', async () => {
    const remote = vi.fn(async () => true);
    const r = run([{ scope: 'https://gateway.example/a--b@peer/', unregister: remote }], ['https://gateway.example/a--b@peer/assets/old.js']);
    await r.ready;
    expect(remote).toHaveBeenCalledOnce();
    expect(r.deleted).toEqual(['https://gateway.example/a--b@peer/assets/old.js']);
  });

  it('does not mark completion when retirement fails', async () => {
    const r = run([], [], true);
    await expect(r.ready).rejects.toThrow('cache unavailable');
    expect(r.document.cookie).toBe('');
  });

  it('does not mark completion while a remote registration remains', async () => {
    const r = run([{ scope: 'https://gateway.example/work@peer/', unregister: async () => false }], []);
    await expect(r.ready).rejects.toThrow('Could not retire');
    expect(r.document.cookie).toBe('');
  });
});
