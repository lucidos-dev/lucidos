import type { Tap, NavigateUi } from '@lucidos/sdk';

export type DeepLinkAction =
  /** Forward `to` straight to handleNavigationRequest (the same router the
   *  `navigate_ui` LLM tool uses). The `notification` field carries the
   *  source notification id so the dispatcher can mark it read in parallel
   *  with the navigation. */
  | { type: 'navigate'; to: NavigateUi; notification: string | null }
  | { type: 'view-notification'; id: string }
  | { type: 'noop' };

export interface DeepLinkTarget {
  notification?: string | null;
  // CONTEXT — source-of-event for the §4 in-app matrix. `thread` + `event`
  // identify where the notification was raised; they drive Row 1's
  // "user already saw it, auto-mark-read" check. Distinct from
  // `tap.to.event_id`, which is the scroll-and-pulse target when the tap
  // navigates to a thread.
  thread?: string | null;
  event?: string | null;
  // ACTION — pre-resolved tap. Arrives populated from the SSE
  // `NotificationCreated` payload, the SW `lucidos:deep-link` message, AND
  // the URL `tap=` param (JSON-encoded for cold-tab opens). Null/undefined
  // only for pre-migration URLs that lacked the tap param entirely — the
  // dispatcher safely demotes those to the modal default.
  tap?: Tap | null;
}

/** Resolve a push-tap target to a single dispatcher action. The discriminated
 *  `Tap` carries everything we need — there's no sugar for old strings.
 *
 *  Missing `tap` (null/undefined) defaults to `{kind:'modal'}` so legacy URLs
 *  that pre-date the structured shape still open the notification detail in the
 *  panel (the `modal` tap kind is the wire name, not a UI modal). */
export function resolveDeepLink(target: DeepLinkTarget): DeepLinkAction {
  const tap: Tap = target.tap ?? { kind: 'modal' };
  // Only `navigate` deep-links; everything else opens the inbox detail. That
  // "everything else" defensively covers the retired `none` kind (and any
  // unknown future kind) arriving via untyped runtime data (an old SW message,
  // a stale URL param, a historical row) — every notification is openable.
  if (tap.kind === 'navigate') {
    return { type: 'navigate', to: tap.to, notification: target.notification ?? null };
  }
  if (target.notification) return { type: 'view-notification', id: target.notification };
  return { type: 'noop' };
}

const DEEP_LINK_KEYS = ['notification', 'thread', 'event', 'tap'] as const;

/** Read deep-link params from a URL/Location, preferring hash over query.
 *
 *  The service worker writes the deep link as a URL hash so warm clients see
 *  a hash-only navigation (no page reload, no Chrome "site updated" toast).
 *  Older SW versions and any hand-typed deep link still arrive as a query
 *  string, so we read both — hash wins when both carry the same key.
 *
 *  URL keys: `notification`, `thread`, `event` (context for the §4 matrix)
 *  plus `tap` (URL-encoded JSON of the structured `Tap` object — present only
 *  when the tap is non-modal, so navigate / none kinds survive a cold-tab
 *  open). Old `tap=open_thread` strings (left over in stale URLs from before
 *  the migration) fail `validateTap` and yield `tap: null`, which the
 *  dispatcher safely demotes to the modal default. */
export function parseDeepLinkFromUrl(loc: URL | Location): DeepLinkTarget {
  const hashParams = loc.hash.startsWith('#')
    ? new URLSearchParams(loc.hash.slice(1))
    : new URLSearchParams();
  const queryParams = new URLSearchParams(loc.search);
  const get = (k: string) => hashParams.get(k) ?? queryParams.get(k);
  return {
    notification: get('notification'),
    thread: get('thread'),
    event: get('event'),
    tap: parseTapFromUrlParam(get('tap')),
  };
}

/** Decode the URL `tap=` param: JSON-encoded structured Tap object, or null
 *  when absent / malformed / stale (legacy bare strings).  */
function parseTapFromUrlParam(raw: string | null): Tap | null {
  if (!raw) return null;
  try {
    return validateTap(JSON.parse(raw));
  } catch {
    return null;
  }
}

/** True when the target has anything `dispatchDeepLink` can act on.
 *  A notification id is required — bare `thread=`/`event=` URLs without a
 *  `notification` key resolve to noop in the dispatcher (no tap to apply,
 *  no inbox row to view); claiming they're dispatch-worthy here just
 *  causes `stripDeepLinkFromUrl` to clear them with nothing replacing
 *  the navigation. The bare `#thread=<uuid>` cross-workspace channel is
 *  intercepted by THREAD_HASH_RE in `hash-deeplink-router.ts` before this
 *  gate fires. */
export function hasDeepLinkParams(target: DeepLinkTarget): boolean {
  return !!target.notification;
}

/** Validate an unknown value against the `Tap` discriminated union. Returns
 *  the value when the `kind` is recognized; `null` otherwise. The SW message
 *  channel carries the structured object natively (no JSON round-trip), but
 *  we still type-guard at the boundary so a malformed payload (e.g. an old
 *  SW version sending a bare string) doesn't crash the dispatcher. */
function validateTap(raw: unknown): Tap | null {
  if (!raw || typeof raw !== 'object') return null;
  const obj = raw as Record<string, unknown>;
  switch (obj.kind) {
    case 'modal':
      return { kind: 'modal' };
    // The retired `none` kind is intentionally NOT accepted — a `{kind:'none'}`
    // payload (old SW message / stale URL) falls through to `null`, which
    // `resolveDeepLink` demotes to the openable modal default.
    case 'navigate': {
      const to = obj.to;
      if (!to || typeof to !== 'object') return null;
      const target = (to as Record<string, unknown>).target;
      if (typeof target !== 'string') return null;
      return { kind: 'navigate', to: to as NavigateUi };
    }
    default:
      return null;
  }
}

/** Translate the SW's `tapData` shape (`notification_id` / `thread_id` /
 *  `event_id` / `tap`) to a `DeepLinkTarget`. The service worker posts this
 *  shape as a `lucidos:deep-link` message to an already-open top-level tab on
 *  notification tap (`routeToDeepLink` in `sw.js`) — the deterministic warm-tab
 *  delivery that replaced a fragment-only `client.navigate('/#…')` (Chrome
 *  doesn't fire `hashchange` for it; see system-knowhow/notifications.md §4.5).
 *  The message handler in `useStartup` routes it through `dispatchDeepLink` —
 *  the same dispatcher the cold-open URL path (`handleHashLocation`) uses.
 *
 *  `tap` arrives as a structured object (the engine puts it on the push
 *  payload, the SW forwards it on `event.notification.data`). `app_id` is
 *  intentionally NOT carried — when the tap navigates to an app, the app
 *  id lives inside `tap.to.app_id`. */
export function parseDeepLinkFromSwMessage(raw: unknown): DeepLinkTarget | null {
  if (!raw || typeof raw !== 'object') return null;
  const obj = raw as Record<string, unknown>;
  return {
    notification: typeof obj.notification_id === 'string' ? obj.notification_id : null,
    thread: typeof obj.thread_id === 'string' ? obj.thread_id : null,
    event: typeof obj.event_id === 'string' ? obj.event_id : null,
    tap: validateTap(obj.tap),
  };
}

/** Return a new URL with every deep-link key removed from query and hash.
 *
 *  Called once we've dispatched the deep link so a refresh doesn't re-fire
 *  it. Non-deep-link keys in either channel are preserved (rare, but a
 *  third-party iframe could legitimately put state in the hash). */
export function stripDeepLinkFromUrl(loc: URL): URL {
  const next = new URL(loc.href);
  for (const k of DEEP_LINK_KEYS) next.searchParams.delete(k);
  if (next.hash.startsWith('#')) {
    const hashParams = new URLSearchParams(next.hash.slice(1));
    for (const k of DEEP_LINK_KEYS) hashParams.delete(k);
    const rest = hashParams.toString();
    next.hash = rest ? `#${rest}` : '';
  }
  return next;
}
