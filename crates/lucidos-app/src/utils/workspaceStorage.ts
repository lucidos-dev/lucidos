/**
 * Per-workspace localStorage namespacing (gateway topology, ADR 0014).
 *
 * The gateway serves EVERY workspace from one origin (`https://localhost:5251`
 * in dev), distinguishing them only by the `/<slug>/` path prefix. `localStorage`
 * is scoped per ORIGIN (scheme + host + port) and ignores the path — so without
 * this wrapper all workspaces share one bucket and clobber each other's
 * content-pane navigation, focused thread, drawer filters, scroll positions,
 * split ratios, restart state, etc.
 *
 * `installWorkspaceStorage` overrides `getItem`/`setItem`/`removeItem` on the
 * `localStorage` instance so every key is transparently prefixed with the
 * workspace id (`ws:<slug>:<key>`), EXCEPT the two cross-workspace picker keys
 * (see `GLOBAL_KEYS`). Those three methods are what every call site uses, so
 * all ~40 are covered without per-callsite changes. `clear()`, `key(i)` and
 * `length` are NOT wrapped and still see the raw cross-workspace bucket. A
 * production `localStorage.clear()` would therefore wipe every workspace, so
 * keep those three to tests. EVERYTHING ELSE is workspace-scoped,
 * including the theme, fonts, scale, and the device id (each workspace gets its
 * own device identity). The only escape is the picker keys, which are
 * cross-workspace by definition (written inside a workspace, read by the picker
 * where namespacing is a no-op). The same scoping must be applied in the OTHER
 * realms that reach this origin's storage and bypass this prototype override —
 * the `index.html` FOUC IIFE, the engine-served `sdk-prefs.js`
 * (`crates/lucidos-engine/src/api/sdk_prefs.rs`), and the SDK
 * (`packages/lucidos-sdk/src/_storage.ts`) — or a parent write won't match an
 * iframe read. A Vitest guard (`no-raw-storage.test.ts`) fails the build on any
 * raw `localStorage`/`sessionStorage` access outside the sanctioned files.
 *
 * When the workspace id is null (the gateway picker `/~/`, a legacy direct
 * engine at base `''`, or unit tests with no stamped `<base>`), the install is a
 * no-op: storage behaves exactly as before. So `WORKSPACE_ID === null` is the
 * single switch that keeps the picker, legacy mode, and the test polyfill valid.
 *
 * Install runs ONCE, before any module-init `localStorage` read — see
 * `workspaceStorage.install.ts`, imported on the first line of `main.tsx`.
 */

import { LAST_WORKSPACE_KEY, LAST_WORKSPACE_COUNT_KEY } from './lastWorkspace';

/** Prefix for every namespaced key: `ws:<workspaceId>:<originalKey>`. */
const NAMESPACE_PREFIX = 'ws:';

/** Per-workspace marker (namespaced) that records the one-time migration ran. */
const MIGRATION_MARKER = '__migrated';

/**
 * The ONLY keys that stay raw (unscoped). Both are cross-workspace by definition:
 * written from inside a workspace (`/<slug>/`, where storage is namespaced) but
 * read by the picker (`/~/` or `/`, where namespacing is a no-op and storage is
 * raw). A namespaced key would never match across those contexts, so they MUST
 * stay raw on both ends (see `lastWorkspace.ts`).
 *
 * Everything else — theme, fonts, scale, device id, nav history, etc. — is
 * workspace-scoped. Notably `lucidos-device-id` is per-workspace now: each
 * workspace has its own device identity (its own device row, push subscription,
 * presence). That is intentional.
 */
export const GLOBAL_KEYS: ReadonlySet<string> = new Set([
  LAST_WORKSPACE_KEY,
  LAST_WORKSPACE_COUNT_KEY,
]);

/**
 * Former-global appearance keys. These used to be raw/shared, so a one-time
 * migration SEEDS each workspace's namespaced value from the legacy raw value
 * (so the user's look carries over once). Device id is deliberately NOT seeded —
 * seeding it would re-share the identity, defeating per-workspace scoping.
 */
const APPEARANCE_SEED_KEYS: readonly string[] = [
  'lucidos-theme',
  'lucidos-font-family',
  'lucidos-ui-scale',
  'lucidos-animation-speed-slider',
];

/** A key is global (kept raw) iff it is in the picker-key allowlist. */
export function isGlobalKey(key: string): boolean {
  return GLOBAL_KEYS.has(key);
}

/** The namespaced form of `key` for workspace `ws`: `ws:<ws>:<key>`. */
export function namespacedKey(key: string, ws: string): string {
  return `${NAMESPACE_PREFIX}${ws}:${key}`;
}

/** Whether the one-time migration should SEED this workspace's namespaced value
 *  from a legacy raw value: only the former-global appearance keys, and only when
 *  not already namespaced. */
export function shouldSeed(key: string): boolean {
  if (key.startsWith(NAMESPACE_PREFIX)) return false; // already namespaced
  return APPEARANCE_SEED_KEYS.includes(key);
}

/**
 * One-time per-workspace migration. Runs against RAW storage methods (call before
 * the namespacing overrides are installed), idempotent via a namespaced marker.
 *
 * Policy (the contamination fix): under the shared gateway origin, pre-existing
 * raw per-workspace keys are a cross-workspace mush — adopting them bakes another
 * workspace's state (e.g. a never-opened nav-history overlay) into this one. So
 * we DO NOT adopt arbitrary raw keys. We only SEED the former-global appearance
 * keys, copying the legacy raw value into this workspace's namespace (preserving
 * the user's look once) WITHOUT deleting the raw original — every workspace seeds
 * from the same legacy value, and nothing reads it raw anymore so it's a harmless
 * orphan. Everything else (nav, overlay, folders, scroll, filters, device id)
 * starts clean per workspace.
 */
export function migrateUnprefixedKeys(storage: Storage, ws: string): void {
  const marker = namespacedKey(MIGRATION_MARKER, ws);
  if (storage.getItem(marker) !== null) return;

  for (const k of APPEARANCE_SEED_KEYS) {
    if (!shouldSeed(k)) continue; // defensive; APPEARANCE_SEED_KEYS are all seedable
    const target = namespacedKey(k, ws);
    if (storage.getItem(target) !== null) continue; // already set for this ws — keep it
    const val = storage.getItem(k);
    if (val !== null) storage.setItem(target, val);
  }

  storage.setItem(marker, '1');
}

/** Marks a `Storage` prototype as already wrapped, so a second install (HMR, or
 *  `sessionStorage` sharing `Storage.prototype` with `localStorage`) is a no-op
 *  instead of double-prefixing every key. */
const INSTALLED = Symbol.for('lucidos.workspaceStorage.installed');

/**
 * Install the per-workspace namespacing on `storage`. No-op when `ws` is null
 * (picker / legacy root / tests). Runs the one-time migration first, then
 * overrides `getItem`/`setItem`/`removeItem` to prefix non-global keys. Wrapped
 * in try/catch so a hostile storage environment degrades to raw behaviour rather
 * than breaking the app.
 *
 * The overrides MUST go on the PROTOTYPE (`Object.getPrototypeOf(storage)` —
 * `Storage.prototype` for the real `localStorage`/`sessionStorage`), never the
 * instance. A native `Storage` object implements WebIDL `[LegacyOverrideBuiltIns]`
 * with a named-property setter, so BOTH `storage.getItem = fn` AND
 * `Object.defineProperty(storage, 'getItem', …)` are intercepted by that setter
 * and swallowed as `setItem('getItem', fn)` — the method is never replaced and
 * the namespacing silently no-ops (the original instance-assignment version of
 * this function did exactly that in every real browser, while passing the unit
 * tests because their mock was a plain object). The prototype is an ordinary
 * object where assignment works normally, and a `storage.getItem` lookup resolves
 * to it because no stored item is ever named "getItem". Overriding the shared
 * prototype also transparently covers `sessionStorage`.
 */
export function installWorkspaceStorage(storage: Storage, ws: string | null): void {
  if (!ws) return; // picker, legacy direct-engine root, or unit tests — pass through

  try {
    // Migrate using the current (still-raw) methods, before the override lands.
    migrateUnprefixedKeys(storage, ws);

    const proto = Object.getPrototypeOf(storage) as Storage &
      Partial<Record<typeof INSTALLED, boolean>>;
    if (proto[INSTALLED]) return; // already wrapped (HMR / shared prototype)

    const rawGet = proto.getItem;
    const rawSet = proto.setItem;
    const rawRemove = proto.removeItem;
    // Idempotent: an already-`ws:`-prefixed key passes through untouched. Real
    // app keys never start with `ws:`, so this can only be a value already
    // namespaced by a caller — e.g. the SDK's `_storage.ts` helpers, which
    // namespace by hand for the iframe realm and would otherwise be
    // double-prefixed (`ws:slug:ws:slug:key`) if ever invoked in this overridden
    // parent realm. Re-prefixing is always a bug, so guard it at the override.
    const mapKey = (key: string): string =>
      key.startsWith(NAMESPACE_PREFIX) || isGlobalKey(key)
        ? key
        : namespacedKey(key, ws);

    proto.getItem = function (this: Storage, key: string) {
      return rawGet.call(this, mapKey(key));
    };
    proto.setItem = function (this: Storage, key: string, value: string) {
      return rawSet.call(this, mapKey(key), value);
    };
    proto.removeItem = function (this: Storage, key: string) {
      return rawRemove.call(this, mapKey(key));
    };
    Object.defineProperty(proto, INSTALLED, { value: true, configurable: true });
  } catch (err) {
    // Runs at module-init with no UI to toast and no user intent — a hostile
    // localStorage (non-extensible / disabled) must degrade to raw storage so the
    // app still loads (it just loses per-workspace isolation in that browser).
    // Surfaced as a warn for debugging, not a user-facing error.
    console.warn('[workspaceStorage] install failed; using raw storage', err);
  }
}
