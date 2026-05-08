// Per-app scroll memory inside iframe-hosted apps.
//
// Apps live in iframes that the parent destroys/recreates on every app switch
// (AppUiInline.tsx uses `key={refreshKey}` and the parent only mounts the
// current app's iframe). Reloads also lose scroll. This module preserves
// scrollY across both — keyed per-app so apps never inherit each other's
// position.
//
// Storage: sessionStorage. Per-tab, per-origin. Survives iframe remount and
// in-tab navigation but dies on tab close — long enough for the use case
// (resume where you were today), short enough that yesterday's stale offsets
// don't accumulate forever.
//
// Save trigger: `pagehide` on window. Fires when the iframe element is removed
// from the DOM (parent unmounts) and on tab close. Reliable in iframes.

const SCROLL_KEY_PREFIX = 'lucidos-scroll-app-';
const APP_PATH_RE = /^\/app\/([^/]+)/;
const RESTORE_DEADLINE_MS = 3000;

export function parseAppId(pathname: string): string | null {
  const match = pathname.match(APP_PATH_RE);
  if (!match) return null;
  const raw = match[1];
  if (!raw) return null;
  try {
    return decodeURIComponent(raw);
  } catch {
    return raw;
  }
}

export function scrollKey(appId: string): string {
  return `${SCROLL_KEY_PREFIX}${appId}`;
}

export function parseSavedScroll(raw: string | null): number | null {
  if (raw === null || raw === '') return null;
  const n = Number.parseFloat(raw);
  if (!Number.isFinite(n) || n < 0) return null;
  return Math.floor(n);
}

export function isFullyRestorable(saved: number, scrollHeight: number, clientHeight: number): boolean {
  if (saved <= 0) return false;
  const max = Math.max(0, scrollHeight - clientHeight);
  return max >= saved;
}

/**
 * Install scroll-memory for the current iframe. Call once on SDK load.
 * Returns a cleanup function that removes listeners.
 *
 * No-op when the document is not served from an `/app/<id>/` path
 * (so the SDK loaded into a non-app context — e.g. standalone test harness —
 * silently does nothing).
 */
export function installScrollMemory(): () => void {
  const appId = parseAppId(window.location.pathname);
  if (!appId) return () => {};
  const key = scrollKey(appId);

  // Restore — but never fight an explicit anchor target.
  if (!window.location.hash) {
    const saved = parseSavedScroll(sessionStorage.getItem(key));
    if (saved !== null && saved > 0) {
      const root = document.documentElement;
      if (isFullyRestorable(saved, root.scrollHeight, root.clientHeight)) {
        window.scrollTo(0, saved);
      } else {
        // Body height grows asynchronously (scripts run, fonts/images load,
        // SPA frameworks hydrate). Observe <html> with both observers because
        // (a) the SDK runs in <head> before body exists, so observing
        // document.body would NPE, and (b) <html>'s box stays viewport-sized
        // even as scrollHeight grows, so ResizeObserver on <html> alone never
        // fires for the typical "content added below the fold" case —
        // MutationObserver fills that gap.
        let restored = false;
        let resize: ResizeObserver | null = null;
        let mutate: MutationObserver | null = null;
        const onDeadline = () => {
          // Content didn't grow tall enough in time — restore as far as we
          // can so the user lands close to their last position rather than
          // being yanked to the top. Skip if the user already scrolled.
          if (!restored && window.scrollY === 0) {
            const max = Math.max(0, root.scrollHeight - root.clientHeight);
            if (max > 0) window.scrollTo(0, Math.min(saved, max));
          }
          stop();
        };
        const deadline = setTimeout(onDeadline, RESTORE_DEADLINE_MS);
        const stop = () => {
          restored = true;
          resize?.disconnect();
          mutate?.disconnect();
          clearTimeout(deadline);
        };
        const tryRestore = () => {
          if (restored) return;
          if (!isFullyRestorable(saved, root.scrollHeight, root.clientHeight)) return;
          window.scrollTo(0, saved);
          stop();
        };
        if (typeof ResizeObserver !== 'undefined') {
          resize = new ResizeObserver(tryRestore);
          resize.observe(root);
        }
        if (typeof MutationObserver !== 'undefined') {
          mutate = new MutationObserver(tryRestore);
          mutate.observe(root, { childList: true, subtree: true });
        }
      }
    }
  }

  const onPageHide = () => {
    const y = window.scrollY;
    try {
      if (y <= 0) {
        sessionStorage.removeItem(key);
      } else {
        sessionStorage.setItem(key, String(Math.floor(y)));
      }
    } catch (err) {
      // Quota exceeded or storage disabled (third-party cookie blocking, etc.).
      // Genuinely unrecoverable from inside the iframe — log so a developer
      // tracking down "scroll didn't restore" sees the cause.
      console.warn('[lucidos-sdk] scroll-memory save failed:', err);
    }
  };

  window.addEventListener('pagehide', onPageHide);

  return () => {
    window.removeEventListener('pagehide', onPageHide);
  };
}
