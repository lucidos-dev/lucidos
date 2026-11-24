
import { wsSessionGet, wsSessionSet, wsSessionRemove } from './_storage';
import { getBaseUrl } from './_fetch';

const SCROLL_KEY_PREFIX = 'lucidos-scroll-app-';
const APP_PATH_RE = /^\/app\/([^/]+)/;
const RESTORE_DEADLINE_MS = 3000;

export function parseAppId(pathname: string, baseUrl: string = ''): string | null {
  const rel = baseUrl && pathname.startsWith(baseUrl) ? pathname.slice(baseUrl.length) : pathname;
  const match = rel.match(APP_PATH_RE);
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

export function installScrollMemory(): () => void {
  const appId = parseAppId(window.location.pathname, getBaseUrl());
  if (!appId) return () => {};
  const key = scrollKey(appId);

  if (!window.location.hash) {
    const saved = parseSavedScroll(wsSessionGet(key));
    if (saved !== null && saved > 0) {
      const root = document.documentElement;
      if (isFullyRestorable(saved, root.scrollHeight, root.clientHeight)) {
        window.scrollTo(0, saved);
      } else {
        let restored = false;
        let resize: ResizeObserver | null = null;
        let mutate: MutationObserver | null = null;
        const onDeadline = () => {
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
        wsSessionRemove(key);
      } else {
        wsSessionSet(key, String(Math.floor(y)));
      }
    } catch (err) {
      console.warn('[lucidos-sdk] scroll-memory save failed:', err);
    }
  };

  window.addEventListener('pagehide', onPageHide);

  return () => {
    window.removeEventListener('pagehide', onPageHide);
  };
}
