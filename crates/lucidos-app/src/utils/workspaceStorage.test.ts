import { describe, it, expect } from 'vitest';
import {
  GLOBAL_KEYS,
  isGlobalKey,
  namespacedKey,
  shouldMigrate,
  migrateUnprefixedKeys,
  installWorkspaceStorage,
} from './workspaceStorage';

/** In-memory Storage matching the browser contract (and the test-setup stub). */
function makeStorage(seed: Record<string, string> = {}): Storage {
  const store: Record<string, string> = { ...seed };
  return {
    getItem: (key: string) => (key in store ? store[key] : null),
    setItem: (key: string, val: string) => { store[key] = String(val); },
    removeItem: (key: string) => { delete store[key]; },
    clear: () => { for (const k of Object.keys(store)) delete store[k]; },
    get length() { return Object.keys(store).length; },
    key: (i: number) => Object.keys(store)[i] ?? null,
  };
}

/** Read a key bypassing any installed override (raw storage). */
function rawKeys(storage: Storage): string[] {
  const out: string[] = [];
  for (let i = 0; i < storage.length; i++) {
    const k = storage.key(i);
    if (k !== null) out.push(k);
  }
  return out;
}

describe('pure helpers', () => {
  it('namespacedKey shape', () => {
    expect(namespacedKey('lucidos-focused-thread', 'alpha')).toBe(
      'ws:alpha:lucidos-focused-thread',
    );
  });

  it('isGlobalKey covers device id, appearance, and sw/build keys', () => {
    expect(isGlobalKey('lucidos-device-id')).toBe(true);
    expect(isGlobalKey('lucidos-theme')).toBe(true);
    expect(isGlobalKey('lucidos-font-family')).toBe(true);
    expect(isGlobalKey('lucidos-ui-scale')).toBe(true);
    expect(isGlobalKey('lucidos-animation-speed-slider')).toBe(true);
    expect(isGlobalKey('lucidos-sw-update-dismissed')).toBe(true);
    expect(isGlobalKey('lucidos-chunk-reload-at')).toBe(true);
    expect(isGlobalKey('lucidos-focused-thread')).toBe(false);
  });

  it('shouldMigrate: workspace keys yes, global/namespaced/foreign no', () => {
    expect(shouldMigrate('lucidos-focused-thread')).toBe(true);
    expect(shouldMigrate('lucidos-nav-history')).toBe(true);
    expect(shouldMigrate('pinned_apps')).toBe(true);
    expect(shouldMigrate('file-preview-open')).toBe(true);
    expect(shouldMigrate('app-window-open')).toBe(true);
    // dynamic keys
    expect(shouldMigrate('lucidos-scroll-thread-abc')).toBe(true);
    expect(shouldMigrate('lucidos-scroll-content-apps')).toBe(true);
    // excluded
    expect(shouldMigrate('lucidos-device-id')).toBe(false);
    expect(shouldMigrate('lucidos-theme')).toBe(false);
    expect(shouldMigrate('ws:alpha:lucidos-focused-thread')).toBe(false);
    expect(shouldMigrate('some-other-app-key')).toBe(false);
  });
});

describe('installWorkspaceStorage — cross-workspace isolation', () => {
  it('prefixes non-global keys with the workspace id', () => {
    const storage = makeStorage();
    installWorkspaceStorage(storage, 'alpha');

    storage.setItem('lucidos-focused-thread', 't1');
    expect(storage.getItem('lucidos-focused-thread')).toBe('t1');
    // physically stored under the namespaced key
    expect(rawKeys(storage)).toContain('ws:alpha:lucidos-focused-thread');
  });

  it('two workspaces never see each other (separate storages, same keys)', () => {
    const a = makeStorage();
    const b = makeStorage();
    installWorkspaceStorage(a, 'alpha');
    installWorkspaceStorage(b, 'beta');

    a.setItem('lucidos-focused-thread', 'from-alpha');
    b.setItem('lucidos-focused-thread', 'from-beta');

    expect(a.getItem('lucidos-focused-thread')).toBe('from-alpha');
    expect(b.getItem('lucidos-focused-thread')).toBe('from-beta');
  });

  it('one origin, two namespaces never collide (shared storage)', () => {
    // Models the gateway: one localStorage, two workspaces. We can't install two
    // overrides on one object, so assert alpha's write lands only in alpha's
    // namespaced slot — beta's slot (which a beta install would read) stays empty.
    const storage = makeStorage();
    installWorkspaceStorage(storage, 'alpha');
    storage.setItem('lucidos-nav-history', 'alpha-nav');

    expect(namespacedKey('lucidos-nav-history', 'alpha')).not.toBe(
      namespacedKey('lucidos-nav-history', 'beta'),
    );
    // Inspect raw storage (the override would re-map a getItem call): alpha's
    // slot holds the value, beta's slot was never written.
    const keys = rawKeys(storage);
    expect(keys).toContain('ws:alpha:lucidos-nav-history');
    expect(keys).not.toContain('ws:beta:lucidos-nav-history');
  });

  it('namespaces dynamic keys without per-callsite changes', () => {
    const storage = makeStorage();
    installWorkspaceStorage(storage, 'alpha');
    storage.setItem('lucidos-scroll-thread-xyz', '42');
    expect(rawKeys(storage)).toContain('ws:alpha:lucidos-scroll-thread-xyz');
  });

  it('removeItem maps to the namespaced key', () => {
    const storage = makeStorage();
    installWorkspaceStorage(storage, 'alpha');
    storage.setItem('lucidos-nav-history', 'x');
    storage.removeItem('lucidos-nav-history');
    expect(storage.getItem('lucidos-nav-history')).toBe(null);
    expect(rawKeys(storage)).not.toContain('ws:alpha:lucidos-nav-history');
  });
});

describe('installWorkspaceStorage — device-global keys stay raw', () => {
  it('device id is written/read at its raw key', () => {
    const storage = makeStorage();
    installWorkspaceStorage(storage, 'alpha');
    storage.setItem('lucidos-device-id', 'dev-123');
    expect(rawKeys(storage)).toContain('lucidos-device-id');
    expect(rawKeys(storage)).not.toContain('ws:alpha:lucidos-device-id');
    expect(storage.getItem('lucidos-device-id')).toBe('dev-123');
  });

  it('every allowlisted key stays raw', () => {
    const storage = makeStorage();
    installWorkspaceStorage(storage, 'alpha');
    for (const k of GLOBAL_KEYS) {
      storage.setItem(k, 'v');
      expect(rawKeys(storage)).toContain(k);
      expect(rawKeys(storage)).not.toContain(namespacedKey(k, 'alpha'));
    }
  });
});

describe('installWorkspaceStorage — null workspace passes through', () => {
  it('does not prefix or migrate when ws is null (picker/legacy/tests)', () => {
    const storage = makeStorage({ 'lucidos-focused-thread': 't1' });
    installWorkspaceStorage(storage, null);
    storage.setItem('lucidos-nav-history', 'x');
    // raw keys unchanged in shape — no ws: prefix, no migration marker
    expect(storage.getItem('lucidos-focused-thread')).toBe('t1');
    expect(rawKeys(storage)).toEqual(
      expect.arrayContaining(['lucidos-focused-thread', 'lucidos-nav-history']),
    );
    expect(rawKeys(storage).some(k => k.startsWith('ws:'))).toBe(false);
  });
});

describe('one-time migration', () => {
  it('moves existing unprefixed workspace keys into the namespace', () => {
    const storage = makeStorage({
      'lucidos-focused-thread': 't1',
      'lucidos-nav-history': 'nav',
      'pinned_apps': '["a"]',
      'lucidos-device-id': 'dev-123', // global — must stay
    });
    installWorkspaceStorage(storage, 'alpha');

    expect(storage.getItem('lucidos-focused-thread')).toBe('t1');
    expect(storage.getItem('lucidos-nav-history')).toBe('nav');
    expect(storage.getItem('pinned_apps')).toBe('["a"]');
    // originals removed
    const keys = rawKeys(storage);
    expect(keys).not.toContain('lucidos-focused-thread');
    expect(keys).toContain('ws:alpha:lucidos-focused-thread');
    // device id untouched at raw key
    expect(keys).toContain('lucidos-device-id');
  });

  it('is idempotent — second run moves nothing', () => {
    const storage = makeStorage({ 'lucidos-nav-history': 'nav' });
    migrateUnprefixedKeys(storage, 'alpha');
    // user writes a fresh raw key after migration (pre-override scenario)
    storage.setItem('lucidos-late-key', 'late');
    migrateUnprefixedKeys(storage, 'alpha');
    // the late raw key survives untouched because the marker short-circuits
    expect(storage.getItem('lucidos-late-key')).toBe('late');
    expect(rawKeys(storage)).toContain('lucidos-late-key');
  });

  it('never clobbers an existing namespaced value (newer wins)', () => {
    const storage = makeStorage({
      'ws:alpha:lucidos-nav-history': 'new',
      'lucidos-nav-history': 'old',
    });
    migrateUnprefixedKeys(storage, 'alpha');
    expect(storage.getItem('ws:alpha:lucidos-nav-history' as string)).toBe('new');
    expect(rawKeys(storage)).not.toContain('lucidos-nav-history');
  });

  it('sets the namespaced marker so it runs once', () => {
    const storage = makeStorage();
    migrateUnprefixedKeys(storage, 'alpha');
    expect(storage.getItem(namespacedKey('__migrated', 'alpha'))).toBe('1');
  });
});
