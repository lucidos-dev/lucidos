/** Hash-channel landing on one app that fills the window: the tab a remote
 *  workspace's app popout opens (ADR 0726). A frontend-only channel and never a
 *  `WorkspaceLanding`, so it never crosses into Rust. A leaf module, so the
 *  popout href and the router share it without an import cycle. */

/** Where the landing puts the user: an app id and, optionally, the app's own
 *  fragment in the form `openApp` takes it. */
export interface AppLanding {
  appId: string;
  fragment?: string;
}

/** ANCHORED, so it cannot shadow `#notifications`, `#thread=` or the
 *  `#notification=` push channel. */
const APP_HASH_RE = /^#app=([^&]+)(?:&fragment=([^&]*))?$/;

/** The one place the frontend writes `#app=`. */
export function appLandingHash({ appId, fragment }: AppLanding): string {
  const hash = `#app=${encodeURIComponent(appId)}`;
  return fragment ? `${hash}&fragment=${encodeURIComponent(fragment)}` : hash;
}

/** Read an `#app=` landing, or `null` for any other hash. */
export function parseAppLandingHash(hash: string): AppLanding | null {
  const match = APP_HASH_RE.exec(hash);
  if (!match) return null;
  try {
    const appId = decodeURIComponent(match[1]);
    return match[2] ? { appId, fragment: decodeURIComponent(match[2]) } : { appId };
  } catch {
    // A malformed escape is not a landing anyone wrote.
    return null;
  }
}
