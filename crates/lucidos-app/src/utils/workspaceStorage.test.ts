import { describe, it, expect } from 'vitest';
import {
  GLOBAL_KEYS,
  isGlobalKey,
  namespacedKey,
  shouldSeed,
  migrateUnprefixedKeys,
  installWorkspaceStorage,
} from './workspaceStorage';

/**
 * In-memory Storage faithful to the native `Storage` contract — the previous mock
 * was a plain object literal, which let the buggy instance-assignment install
 * "work" in tests while silently no-opping in every real browser.
 *
 * Two faithful properties matter here:
 *   1. The methods live on a dedicated PROTOTYPE (like `Storage.prototype`), so a
 *      correct install must override them there. `Object.getPrototypeOf(storage)`
 *      returns that prototype.
 *   2. The instance models WebIDL `[LegacyOverrideBuiltIns]` + named-property
 *      setter: assigning OR `Object.defineProperty`-ing any property on the
 *      instance is swallowed as a stored item, NOT a real own property — so the
 *      old `storage.getItem = fn` technique fails here exactly as it does on a
 *      native `Storage`. This is the regression guard: reverting to instance
 *      assignment breaks the namespacing tests below.
 */
function makeStorage(seed: Record<string, string> = {}): Storage {
  const store: Record<string, string> = { ...seed };
  const proto = {
    getItem(key: string) { return key in store ? store[key] : null; },
    setItem(key: string, val: string) { store[key] = String(val); },
    removeItem(key: string) { delete store[key]; },
    clear() { for (const k of Object.keys(store)) delete store[k]; },
    key(i: number) { return Object.keys(store)[i] ?? null; },
    get length() { return Object.keys(store).length; },
  };
  const instance = Object.create(proto) as Storage;
  return new Proxy(instance, {
    // Native named-property setter: plain assignment stores an item, it never
    // creates an own property that shadows the prototype method.
    set(_target, prop, value) { store[String(prop)] = String(value); return true; },
    // [[DefineOwnProperty]] routes through the named setter too.
    defineProperty(_target, prop, desc) {
      store[String(prop)] = String((desc as PropertyDescriptor).value);
      return true;
    },
  });
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

  it('isGlobalKey: ONLY the two cross-workspace picker keys stay raw', () => {
    expect(isGlobalKey('lucidos-last-workspace')).toBe(true);
    expect(isGlobalKey('lucidos-last-workspace-count')).toBe(true);
    expect(GLOBAL_KEYS.size).toBe(2);
    // Everything else is now workspace-scoped — including device id + appearance.
    expect(isGlobalKey('lucidos-device-id')).toBe(false);
    expect(isGlobalKey('lucidos-theme')).toBe(false);
    expect(isGlobalKey('lucidos-font-family')).toBe(false);
    expect(isGlobalKey('lucidos-ui-scale')).toBe(false);
    expect(isGlobalKey('lucidos-animation-speed-slider')).toBe(false);
    expect(isGlobalKey('lucidos-notifications-filter')).toBe(false);
    expect(isGlobalKey('lucidos-chunk-reload-at')).toBe(false);
    expect(isGlobalKey('lucidos-focused-thread')).toBe(false);
  });

  it('shouldSeed: only former-global appearance keys, never device id or per-workspace state', () => {
    // Seeded once from the legacy raw value so the user's look carries over.
    expect(shouldSeed('lucidos-theme')).toBe(true);
    expect(shouldSeed('lucidos-font-family')).toBe(true);
    expect(shouldSeed('lucidos-ui-scale')).toBe(true);
    expect(shouldSeed('lucidos-animation-speed-slider')).toBe(true);
    // Device id is NOT seeded — each workspace gets a fresh identity.
    expect(shouldSeed('lucidos-device-id')).toBe(false);
    // Per-workspace state is NOT adopted (contamination fix) — starts clean.
    expect(shouldSeed('lucidos-focused-thread')).toBe(false);
    expect(shouldSeed('lucidos-nav-history')).toBe(false);
    expect(shouldSeed('pinned_apps')).toBe(false);
    expect(shouldSeed('file-preview-open')).toBe(false);
    expect(shouldSeed('lucidos-scroll-thread-abc')).toBe(false);
    // Already-namespaced never re-seeds.
    expect(shouldSeed('ws:alpha:lucidos-theme')).toBe(false);
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

describe('installWorkspaceStorage — device id + appearance are now scoped', () => {
  it('device id is per-workspace (namespaced, not raw)', () => {
    const storage = makeStorage();
    installWorkspaceStorage(storage, 'alpha');
    storage.setItem('lucidos-device-id', 'dev-123');
    expect(rawKeys(storage)).toContain('ws:alpha:lucidos-device-id');
    expect(rawKeys(storage)).not.toContain('lucidos-device-id');
    expect(storage.getItem('lucidos-device-id')).toBe('dev-123');
  });

  it('two workspaces get independent device identities', () => {
    const a = makeStorage();
    const b = makeStorage();
    installWorkspaceStorage(a, 'alpha');
    installWorkspaceStorage(b, 'beta');
    a.setItem('lucidos-device-id', 'id-alpha');
    b.setItem('lucidos-device-id', 'id-beta');
    expect(a.getItem('lucidos-device-id')).toBe('id-alpha');
    expect(b.getItem('lucidos-device-id')).toBe('id-beta');
  });

  it('theme is per-workspace (namespaced)', () => {
    const storage = makeStorage();
    installWorkspaceStorage(storage, 'alpha');
    storage.setItem('lucidos-theme', 'light');
    expect(rawKeys(storage)).toContain('ws:alpha:lucidos-theme');
    expect(rawKeys(storage)).not.toContain('lucidos-theme');
  });

  it('is idempotent — an already-namespaced key is not double-prefixed', () => {
    // Guards the SDK-in-parent-realm footgun: a key that already carries the
    // `ws:<slug>:` prefix (e.g. handed in by the SDK's own _storage helper) must
    // pass through untouched rather than becoming `ws:slug:ws:slug:key`.
    const storage = makeStorage();
    installWorkspaceStorage(storage, 'alpha');
    storage.setItem('ws:alpha:lucidos-theme', 'light');
    expect(rawKeys(storage)).toContain('ws:alpha:lucidos-theme');
    expect(rawKeys(storage)).not.toContain('ws:alpha:ws:alpha:lucidos-theme');
    expect(storage.getItem('ws:alpha:lucidos-theme')).toBe('light');
  });

  it('ONLY the picker keys stay raw', () => {
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

describe('one-time migration (seed appearance, never adopt, fresh device id)', () => {
  it('seeds appearance keys from legacy raw values, leaving the raw original', () => {
    const storage = makeStorage({
      'lucidos-theme': 'light',
      'lucidos-font-family': 'fira-code',
    });
    installWorkspaceStorage(storage, 'alpha');

    // appearance carried over into this workspace
    expect(storage.getItem('lucidos-theme')).toBe('light');
    expect(storage.getItem('lucidos-font-family')).toBe('fira-code');
    const keys = rawKeys(storage);
    expect(keys).toContain('ws:alpha:lucidos-theme');
    // legacy raw left as a harmless orphan so a second workspace can seed too
    expect(keys).toContain('lucidos-theme');
  });

  it('does NOT adopt cross-contaminated per-workspace keys (the bug fix)', () => {
    const storage = makeStorage({
      'lucidos-nav-history': 'phantom-overlay-from-another-workspace',
      'pinned_apps': '["foreign"]',
      'lucidos-focused-thread': 't-foreign',
    });
    installWorkspaceStorage(storage, 'alpha');

    // none of the foreign per-workspace state bleeds in — clean slate
    expect(storage.getItem('lucidos-nav-history')).toBe(null);
    expect(storage.getItem('pinned_apps')).toBe(null);
    expect(storage.getItem('lucidos-focused-thread')).toBe(null);
    expect(rawKeys(storage)).not.toContain('ws:alpha:lucidos-nav-history');
  });

  it('does NOT seed device id — each workspace generates a fresh one', () => {
    const storage = makeStorage({ 'lucidos-device-id': 'legacy-shared-id' });
    installWorkspaceStorage(storage, 'alpha');
    // the legacy shared id is not adopted into the workspace namespace
    expect(storage.getItem('lucidos-device-id')).toBe(null);
    expect(rawKeys(storage)).not.toContain('ws:alpha:lucidos-device-id');
  });

  it('is idempotent — second run seeds nothing new', () => {
    const storage = makeStorage({ 'lucidos-theme': 'light' });
    migrateUnprefixedKeys(storage, 'alpha');
    // user changes the namespaced theme; a re-run must not clobber it back to legacy
    storage.setItem('ws:alpha:lucidos-theme', 'dark');
    migrateUnprefixedKeys(storage, 'alpha');
    expect(storage.getItem('ws:alpha:lucidos-theme' as string)).toBe('dark');
  });

  it('never clobbers an already-set namespaced appearance value', () => {
    const storage = makeStorage({
      'ws:alpha:lucidos-theme': 'dark',
      'lucidos-theme': 'light',
    });
    migrateUnprefixedKeys(storage, 'alpha');
    expect(storage.getItem('ws:alpha:lucidos-theme' as string)).toBe('dark');
  });

  it('sets the namespaced marker so it runs once', () => {
    const storage = makeStorage();
    migrateUnprefixedKeys(storage, 'alpha');
    expect(storage.getItem(namespacedKey('__migrated', 'alpha'))).toBe('1');
  });
});
