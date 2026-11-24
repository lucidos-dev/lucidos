import { focusThreadOrBootstrapResult } from './threads';
import { switchMenuItem } from './menu';
import { THREAD_HASH_RE } from './threadHash';
import { parseAppLandingHash } from './appHash';
import { openAppById } from './apps';
import {
  parseDeepLinkFromUrl,
  hasDeepLinkParams,
  stripDeepLinkFromUrl,
} from './notification-deeplink';
import { dispatchDeepLink } from './in-app-notification-toast';
import { postClientLog } from '../../utils/liveness';
import { appPseudoFullscreen, showToast, wipPreviewThreadId } from '../store';
import { replaceNavState } from './navigation';
import { errorDetail } from '../../utils/errorDetail';

const THREAD_HASH_RETRY_DELAYS_MS = [250, 500, 1000, 2000, 3000, 5000];

const sleep = (ms: number) => new Promise<void>(resolve => { setTimeout(resolve, ms); });

let landingThreadId: string | null = null;
let landingToken = 0;

function stripConsumedHash(): void {
  window.history.replaceState({}, '', window.location.pathname + window.location.search);
}

async function landThreadHash(threadId: string): Promise<void> {
  const token = ++landingToken;
  landingThreadId = threadId;
  try {
    for (let attempt = 0; ; attempt++) {
      const outcome = await focusThreadOrBootstrapResult(threadId);
      if (token !== landingToken) return;
      if (outcome.kind === 'focused' || outcome.kind === 'awaiting') {
        postClientLog('deeplink', 'route_thread_hash_landed', {
          thread: threadId,
          attempts: attempt + 1,
        });
        stripConsumedHash();
        return;
      }
      if (outcome.kind === 'not-found') {
        postClientLog('deeplink', 'route_thread_hash_not_found', { thread: threadId });
        stripConsumedHash();
        showToast(`Thread ${threadId} is not in this workspace`, 'error');
        return;
      }
      if (attempt >= THREAD_HASH_RETRY_DELAYS_MS.length) {
        postClientLog('deeplink', 'route_thread_hash_failed', {
          thread: threadId,
          attempts: attempt + 1,
        });
        stripConsumedHash();
        showToast(`Failed to open thread ${threadId}: ${errorDetail(outcome.error)}`, 'error');
        return;
      }
      await sleep(THREAD_HASH_RETRY_DELAYS_MS[attempt]);
      if (token !== landingToken) return;
    }
  } finally {
    if (token === landingToken) landingThreadId = null;
  }
}

const NOTIFICATIONS_HASH_RE = /^#notifications$/;

export function handleHashLocation(): void {
  if (NOTIFICATIONS_HASH_RE.test(window.location.hash)) {
    postClientLog('deeplink', 'route_notifications_hash', {});
    switchMenuItem('notifications');
    stripConsumedHash();
    return;
  }
  const appLanding = parseAppLandingHash(window.location.hash);
  if (appLanding) {
    postClientLog('deeplink', 'route_app_hash', { app: appLanding.appId });
    stripConsumedHash();
    void openAppById(appLanding.appId, 'a popout link', appLanding.fragment)
      .then(opened => {
        if (!opened) return;
        if (wipPreviewThreadId.value !== null) {
          wipPreviewThreadId.value = null;
          replaceNavState();
        }
        appPseudoFullscreen.value = true;
      })
      .catch(err => {
        showToast(`Failed to open app "${appLanding.appId}": ${errorDetail(err)}`, 'error');
      });
    return;
  }
  const hashMatch = THREAD_HASH_RE.exec(window.location.hash);
  if (hashMatch) {
    if (landingThreadId === hashMatch[1]) return;
    postClientLog('deeplink', 'route_thread_hash', { thread: hashMatch[1] });
    void landThreadHash(hashMatch[1]).catch(err => {
      showToast(`Failed to open thread: ${errorDetail(err)}`, 'error');
    });
    return;
  }
  const target = parseDeepLinkFromUrl(window.location);
  postClientLog('deeplink', 'handle_hash_location', {
    found: hasDeepLinkParams(target),
    has_notification: !!target.notification,
    has_thread: !!target.thread,
    tap_kind: target.tap?.kind ?? null,
    search: (() => { try { return window.location.search.slice(0, 200); } catch { return ''; } })(),
  });
  if (!hasDeepLinkParams(target)) return;
  const cleaned = stripDeepLinkFromUrl(new URL(window.location.href));
  window.history.replaceState({}, '', cleaned.toString());
  dispatchDeepLink(target);
}

export function setupHashDeeplinkRouting(): () => void {
  function onResume() {
    if (document.visibilityState === 'visible') handleHashLocation();
  }

  const coldStartTimer = window.setTimeout(handleHashLocation, 0);
  window.addEventListener('hashchange', handleHashLocation);
  document.addEventListener('visibilitychange', onResume);
  window.addEventListener('focus', onResume);
  window.addEventListener('pageshow', onResume);

  return () => {
    clearTimeout(coldStartTimer);
    window.removeEventListener('hashchange', handleHashLocation);
    document.removeEventListener('visibilitychange', onResume);
    window.removeEventListener('focus', onResume);
    window.removeEventListener('pageshow', onResume);
  };
}

export function _resetThreadHashLandingForTesting(): void {
  landingThreadId = null;
  landingToken++;
}
