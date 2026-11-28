
export interface AppLanding {
  appId: string;
  fragment?: string;
}

const APP_HASH_RE = /^#app=([^&]+)(?:&fragment=([^&]*))?$/;

export function appLandingHash({ appId, fragment }: AppLanding): string {
  const hash = `#app=${encodeURIComponent(appId)}`;
  return fragment ? `${hash}&fragment=${encodeURIComponent(fragment)}` : hash;
}

export function parseAppLandingHash(hash: string): AppLanding | null {
  const match = APP_HASH_RE.exec(hash);
  if (!match) return null;
  try {
    const appId = decodeURIComponent(match[1]);
    return match[2] ? { appId, fragment: decodeURIComponent(match[2]) } : { appId };
  } catch {
    return null;
  }
}
