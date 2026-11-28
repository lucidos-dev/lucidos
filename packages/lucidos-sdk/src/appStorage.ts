
export type AppStorageSpace = 'sdk' | 'app';

export type AppStorageArea = 'local' | 'session';

export const APP_STORAGE_SPACES: readonly AppStorageSpace[] = ['sdk', 'app'];
export const APP_STORAGE_AREAS: readonly AppStorageArea[] = ['local', 'session'];

export type AppStorageSnapshot = Record<AppStorageSpace, Record<AppStorageArea, Record<string, string>>>;

export const APP_STORAGE_QUOTA = 512 * 1024;

export const APP_STORAGE_VALUE_MAX = 256 * 1024;

export const SDK_STORAGE_QUOTA = 8 * 1024;

const SPACE_QUOTA: Record<AppStorageSpace, number> = {
  sdk: SDK_STORAGE_QUOTA,
  app: APP_STORAGE_QUOTA,
};

export function appStoragePrefix(appId: string, space: AppStorageSpace): string {
  if (!appId || appId.includes(':')) throw new Error(`"${appId}" is not an app id storage can scope by`);
  return `appbridge:${appId}:${space}:`;
}

export function emptyAppStorageSnapshot(): AppStorageSnapshot {
  return {
    sdk: { local: {}, session: {} },
    app: { local: {}, session: {} },
  };
}

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
