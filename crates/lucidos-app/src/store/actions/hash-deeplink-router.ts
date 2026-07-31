/** Route whatever's in `window.location.hash` to the right handler, then strip
 *  the consumed state so a refresh doesn't re-fire.
 *
 *  Two channels coexist in the hash:
 *  - Bare `#thread=<uuid>` (cross-workspace landing channel — see
 *    `openThreadInWorkspace`) → focusThreadOrBootstrap.
 *  - `#notification=…&thread=…&event=…&tap=…` (Safari's declarative-push
 *    navigate target, the SW client.navigate after a push tap, or a
 *    hand-typed deep link) → dispatchDeepLink.
 *
 *  The same function serves three call sites — cold-start setTimeout,
 *  warm hashchange, and resume (visibilitychange → visible / focus / pageshow).
 *  Resume coverage exists because iOS Safari brings a backgrounded PWA to
 *  foreground after a declarative push tap, but the suspended page often
 *  misses the hashchange event that paired with the URL update. Without the
 *  resume call, the URL hash carries the deep link forever and no dispatch
 *  fires — push tap silently no-ops.
 *
 *  Idempotent: the function strips the consumed hash params via
 *  `window.history.replaceState`, so a second call with the same URL sees
 *  no deep-link and returns. Safe to wire to multiple events. The `#thread=`
 *  branch is asynchronous (it keeps the hash until the focus lands, see
 *  `landThreadHash`), so it holds an in-flight guard for the same purpose while
 *  its hash is still in the URL. */
import { focusThreadOrBootstrapResult } from './threads';
import { THREAD_HASH_RE } from './cross-workspace';
import {
  parseDeepLinkFromUrl,
  hasDeepLinkParams,
  stripDeepLinkFromUrl,
} from './notification-deeplink';
import { dispatchDeepLink } from './in-app-notification-toast';
import { postClientLog } from '../../utils/liveness';
import { showToast } from '../store';
import { errorDetail } from '../../utils/errorDetail';

/** Backoff between landing attempts, in ms. One attempt runs before the first
 *  delay, so this is `length + 1` attempts over ~11.75s.
 *
 *  Sized for the case that made the hash durable in the first place: the peer
 *  workspace is reached through the gateway, which LAZY-STARTS a stopped engine
 *  on the proxy hit. The shell is served (and this code runs) while that engine
 *  is still coming up, so the landing's very first `GET /api/v1/threads` can lose
 *  the race outright. Bounded rather than open-ended so a genuinely unreachable
 *  engine reports an error instead of spinning forever. */
const THREAD_HASH_RETRY_DELAYS_MS = [250, 500, 1000, 2000, 3000, 5000];

const sleep = (ms: number) => new Promise<void>(resolve => { setTimeout(resolve, ms); });

/** The thread id whose landing is currently in flight, or null. `handleHashLocation`
 *  is wired to cold start, `hashchange` and all three resume events, and the hash
 *  now SURVIVES until the focus lands, so without this guard every resume during
 *  the retry window would start a competing landing for the same thread. */
let landingThreadId: string | null = null;
/** Bumped per landing so a superseded one (the user clicked a different
 *  cross-workspace link mid-retry) neither focuses its stale thread nor strips
 *  the newer hash. */
let landingToken = 0;

function stripConsumedHash(): void {
  window.history.replaceState({}, '', window.location.pathname + window.location.search);
}

/** Drive the bare `#thread=<uuid>` landing to a verdict, then consume the hash.
 *
 *  The ordering is the whole point: the hash is the ONLY durable record of where
 *  the user asked to go, so it is stripped on success (or on a terminal failure
 *  the user is told about), never merely on dispatch. Stripping first is what
 *  made a cross-workspace link need two clicks: the first landing raced the peer
 *  engine's lazy start, lost, and had nothing left to retry from. */
async function landThreadHash(threadId: string): Promise<void> {
  const token = ++landingToken;
  landingThreadId = threadId;
  try {
    for (let attempt = 0; ; attempt++) {
      const outcome = await focusThreadOrBootstrapResult(threadId);
      if (token !== landingToken) return; // superseded by a newer landing
      if (outcome.kind === 'focused') {
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

export function handleHashLocation(): void {
  const hashMatch = THREAD_HASH_RE.exec(window.location.hash);
  if (hashMatch) {
    if (landingThreadId === hashMatch[1]) return; // already landing this one
    // Diagnostic breadcrumb (best-effort telemetry, no user intent): records that
    // the bare cross-workspace `#thread=` channel routed. Lets a "push tap opened
    // the app but went nowhere" report be traced to the exact router branch.
    postClientLog('deeplink', 'route_thread_hash', { thread: hashMatch[1] });
    void landThreadHash(hashMatch[1]).catch(err => {
      // `landThreadHash` folds a failed bootstrap into its retry loop, so this
      // only fires if the focus itself threw. Surface it instead of leaving an
      // unhandled rejection (frontend.md, no hidden errors).
      showToast(`Failed to open thread: ${errorDetail(err)}`, 'error');
    });
    return;
  }
  const target = parseDeepLinkFromUrl(window.location);
  // Diagnostic breadcrumb: did the router find deep-link params, and what tap?
  // The iOS declarative-push deep-link path is otherwise silent — this is how we
  // see whether the reload landed the params and which action was dispatched.
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

/** Wire window/document listeners so iOS PWA resume always re-checks the URL
 *  hash for a deep-link, NOT just on hashchange/cold-start. iOS Safari brings
 *  the PWA to foreground after a declarative push tap and may update the URL,
 *  but the suspended JS misses the paired `hashchange` event. The resume
 *  listeners (visibilitychange → visible, focus, pageshow) re-run
 *  `handleHashLocation` on every wake — `handleHashLocation` strips the hash
 *  after dispatching, so duplicate calls no-op.
 *
 *  Also installs the warm-path `hashchange` listener and the cold-start
 *  `setTimeout(500)` so a single import owns the full hash routing.
 *
 *  Returns a teardown that removes every listener — call from `useStartup`'s
 *  effect cleanup so a hot-reload remount starts fresh. */
export function setupHashDeeplinkRouting(): () => void {
  // Every resume entry point gates on `visibilityState === 'visible'` — `focus`
  // can fire on a hidden tab (DevTools focus, programmatic focus on a
  // background window), and dispatching the deep link to a tab the user can't
  // see strips the hash silently and leaves no URL state to retry from. The
  // gate matches what the visibilitychange-only path enforced before.
  function onResume() {
    if (document.visibilityState === 'visible') handleHashLocation();
  }

  // Cold-start: route the deep link as soon as the synchronous boot setup
  // settles. A 0ms timer yields one macrotask (letting signal init + the first
  // mount commit run) without the old fixed 500ms wait — on an iOS notification
  // tap that whole delay sat on the critical path between the reload and the
  // thread appearing. The dispatch is robust to not-yet-hydrated stores: a
  // navigate-kind tap routes through focusThreadOrBootstrap, which fetches the
  // thread when it isn't in the loaded window yet.
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

/** Test-only: drop the in-flight landing state. The guard is module-level so
 *  production holds it for the life of the page; tests that run several landings
 *  need a clean slate between cases. */
export function _resetThreadHashLandingForTesting(): void {
  landingThreadId = null;
  landingToken++;
}
