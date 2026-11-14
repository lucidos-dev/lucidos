/**
 * The shape of *app storage*, shared by the SDK and the host.
 *
 * An isolated app frame cannot reach browser storage, so the host keeps it in
 * the shell's own `localStorage` / `sessionStorage`. A standalone app tab
 * writes the same keys itself. Both sides build keys and check limits here, so
 * the two can never disagree about where a value lives or whether it fits.
 *
 * The host imports it from the SDK barrel. Rationale:
 * `docs/adr/0372-app-storage-lives-in-the-shell.md`.
 */

/** Two key spaces per app. `sdk` holds the SDK's own keys (scroll memory) and
 *  `app` holds the app's. So an app's `clear()` leaves the SDK's keys alone. */
export type AppStorageSpace = 'sdk' | 'app';

/** Which browser store a value lives in. */
export type AppStorageArea = 'local' | 'session';

export const APP_STORAGE_SPACES: readonly AppStorageSpace[] = ['sdk', 'app'];
export const APP_STORAGE_AREAS: readonly AppStorageArea[] = ['local', 'session'];

/** Everything one app frame has stored, as the host hands it over. */
export type AppStorageSnapshot = Record<AppStorageSpace, Record<AppStorageArea, Record<string, string>>>;

/** How much one app may keep in one area, in UTF-16 code units of key plus
 *  value. Browsers count `localStorage` the same way. */
export const APP_STORAGE_QUOTA = 512 * 1024;

/** The largest single value an app may store, in UTF-16 code units. */
export const APP_STORAGE_VALUE_MAX = 256 * 1024;

/** How much the SDK's own space may hold per app and area. It keeps one scroll
 *  position, and the frame picks the space, so it gets a cap of its own. */
export const SDK_STORAGE_QUOTA = 8 * 1024;

const SPACE_QUOTA: Record<AppStorageSpace, number> = {
  sdk: SDK_STORAGE_QUOTA,
  app: APP_STORAGE_QUOTA,
};

/**
 * Where one app's keys in one space start, before workspace namespacing.
 *
 * The app id comes from the host's own frame element, never from the frame. A
 * `:` would let one id be a prefix of another's key space, so it is refused.
 */
export function appStoragePrefix(appId: string, space: AppStorageSpace): string {
  if (!appId || appId.includes(':')) throw new Error(`"${appId}" is not an app id storage can scope by`);
  return `appbridge:${appId}:${space}:`;
}

/** An empty snapshot, the shape a first visit has. */
export function emptyAppStorageSnapshot(): AppStorageSnapshot {
  return {
    sdk: { local: {}, session: {} },
    app: { local: {}, session: {} },
  };
}

/**
 * Why writing `key` = `value` would break a limit, or null if it fits.
 *
 * `entries` is everything the app holds in that space and area now. A key
 * being replaced counts once, at its new size.
 */
export function appStorageRefusal(
  entries: Iterable<[string, string]>,
  key: string,
  value: string,
  space: AppStorageSpace = 'app',
): string | null {
  if (value.length > APP_STORAGE_VALUE_MAX) {
    return `The value for "${key}" is ${value.length} characters, over the ${APP_STORAGE_VALUE_MAX} limit`;
  }
  const quota = SPACE_QUOTA[space];
  let used = key.length + value.length;
  for (const [k, v] of entries) {
    if (k !== key) used += k.length + v.length;
  }
  if (used > quota) {
    return `Storing "${key}" would use ${used} characters, over this app's ${quota} limit`;
  }
  return null;
}
