/**
 * Per-workspace localStorage namespacing (gateway topology, ADR 0014).
 *
 * The gateway serves EVERY workspace from one origin (`https://localhost:5251`
 * in dev), distinguishing them only by the `/<slug>/` path prefix. `localStorage`
 * is scoped per ORIGIN (scheme + host + port) and ignores the path — so without
 * this wrapper all workspaces share one bucket and clobber each other's
 * content-pane navigation, focused thread, drawer filters, scroll positions,
 * split ratios, restart toast, etc.
 *
 * `installWorkspaceStorage` overrides `getItem`/`setItem`/`removeItem` on the
 * `localStorage` instance so every key is transparently prefixed with the
 * workspace id (`ws:<slug>:<key>`), EXCEPT a small allowlist of genuinely
 * device-global keys (see `GLOBAL_KEYS`). All ~40 existing call sites — and any
 * future one — are covered without per-callsite changes.
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
 * Keys that must NOT be namespaced — they are device-global by design:
 *   • `lucidos-device-id` — browser/device identity, shared across workspaces and
 *     read raw by the SDK (`packages/lucidos-sdk/src/preferences.ts`).
 *   • appearance prefs — a user expects a consistent look across workspaces on
 *     the same device.
 *   • service-worker / build keys — they track the checkout-shared `dist/` build,
 *     which is byte-identical across workspaces.
 *   • `lucidos-last-workspace` — the last-active workspace slug, written from
 *     inside a workspace and read by the picker; it spans workspaces by design,
 *     so it must stay raw on both ends (see `lastWorkspace.ts`).
 *   • `lucidos-last-workspace-count` — the picker's last-known workspace count,
 *     used to size its loading skeleton; a picker-surface key kept raw for the
 *     same reason (see `lastWorkspace.ts`).
 */
export const GLOBAL_KEYS: ReadonlySet<string> = new Set([
  'lucidos-device-id',
  'lucidos-theme',
  'lucidos-font-family',
  'lucidos-ui-scale',
  'lucidos-animation-speed-slider',
  'lucidos-sw-update-dismissed',
  'lucidos-chunk-reload-at',
  LAST_WORKSPACE_KEY,
  LAST_WORKSPACE_COUNT_KEY,
]);

/**
 * Non-`lucidos-`-prefixed keys that ARE workspace-specific and so must still be
 * migrated/namespaced. (Most workspace keys start with `lucidos-`; these don't.)
 */
const EXTRA_WORKSPACE_KEYS: ReadonlySet<string> = new Set([
  'pinned_apps',
  'file-preview-open',
  'app-window-open',
]);

/** A key is device-global (kept raw) iff it is in the allowlist. */
export function isGlobalKey(key: string): boolean {
  return GLOBAL_KEYS.has(key);
}

/** The namespaced form of `key` for workspace `ws`: `ws:<ws>:<key>`. */
export function namespacedKey(key: string, ws: string): string {
  return `${NAMESPACE_PREFIX}${ws}:${key}`;
}

/** Whether an existing raw key should be moved into the workspace namespace by
 *  the one-time migration: a workspace key that is not already namespaced and not
 *  device-global. */
export function shouldMigrate(key: string): boolean {
  if (key.startsWith(NAMESPACE_PREFIX)) return false; // already namespaced
  if (isGlobalKey(key)) return false; // device-global, stays raw
  return key.startsWith('lucidos-') || EXTRA_WORKSPACE_KEYS.has(key);
}

/**
 * One-time migration: copy existing unprefixed workspace keys into the active
 * workspace's namespace, then remove the originals. Runs against RAW storage
 * methods (call before the namespacing overrides are installed), so it never
 * double-prefixes. Idempotent via a namespaced marker; never clobbers an
 * already-namespaced value (newer data wins, the stale original is dropped).
 */
export function migrateUnprefixedKeys(storage: Storage, ws: string): void {
  const marker = namespacedKey(MIGRATION_MARKER, ws);
  if (storage.getItem(marker) !== null) return;

  // Snapshot keys first — we mutate `storage` while iterating.
  const keys: string[] = [];
  for (let i = 0; i < storage.length; i++) {
    const k = storage.key(i);
    if (k !== null) keys.push(k);
  }

  for (const k of keys) {
    if (!shouldMigrate(k)) continue;
    const target = namespacedKey(k, ws);
    // Don't overwrite an existing namespaced value (re-entrancy / a prior run).
    if (storage.getItem(target) === null) {
      const val = storage.getItem(k);
      if (val !== null) storage.setItem(target, val);
    }
    storage.removeItem(k);
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
    const mapKey = (key: string): string =>
      isGlobalKey(key) ? key : namespacedKey(key, ws);

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
