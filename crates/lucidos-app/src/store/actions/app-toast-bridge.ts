import { showToast, removeToast } from '../store';
import type { ToastType } from '../types';

/** The `payload` of an app iframe's toast-bridge postMessage. Every field is
 *  `unknown`: it crossed a postMessage boundary from app code, so nothing about
 *  its shape is guaranteed and each field is validated at the point of use. */
export interface AppToastPayload {
  message?: unknown;
  type?: unknown;
  durationMs?: unknown;
  dismissable?: unknown;
  key?: unknown;
  spinning?: unknown;
}

const TOAST_TYPES = ['success', 'info', 'warning', 'error'] as const;

/** Handle the app-facing toast bridge (`lucidos.ui.toast` / `lucidos.ui.dismissToast`).
 *  Returns true when it owned the message, so the caller can stop routing.
 *
 *  Split out of `useStartup`'s `onAppFrameMessage` so these two branches are
 *  reachable from a unit test: the hook itself wires SSE, service workers and a
 *  dozen timers, and the frontend test environment is deliberately non-jsdom, so
 *  standing it up to prove that a dismiss removes a toast is not practical. The
 *  confirm / prompt / preview-file branches stay in the hook (they need the
 *  `event.source` to post their result back), as does the app-frame
 *  authenticity check, which runs BEFORE this and is what makes the payload
 *  merely untrusted rather than unattributed.
 *
 *  Both branches are fire-and-forget: no id, no result reply.
 *
 *  Ordering note for the caller: `dismissToast` carries a key and NO message, so
 *  this must be called ahead of the "confirm, toast and prompt all carry a
 *  message" guard, which would otherwise swallow it. */
export function handleAppToastMessage(type: string, payload: AppToastPayload): boolean {
  if (type === 'lucidos:ui:toast') {
    if (typeof payload.message !== 'string' || payload.message.length === 0) return true;
    const toastType: ToastType = TOAST_TYPES.includes(payload.type as ToastType)
      ? payload.type as ToastType
      : 'info';
    showToast(payload.message, toastType, {
      key: typeof payload.key === 'string' && payload.key.length > 0 ? payload.key : undefined,
      autoDismissMs: typeof payload.durationMs === 'number' ? payload.durationMs : undefined,
      dismissable: typeof payload.dismissable === 'boolean' ? payload.dismissable : undefined,
      spinning: typeof payload.spinning === 'boolean' ? payload.spinning : undefined,
    });
    return true;
  }

  if (type === 'lucidos:ui:dismissToast') {
    // `removeToast`, NOT `dismissToast`: the latter's string arm layers the
    // USER-dismiss side effects on top, which for two host-owned keys record the
    // running build as dismissed. An app naming `update-available` would then
    // suppress the user's own Lucidos update prompt for that build. Structural
    // removal is all an app is entitled to, and it resolves the key with exactly
    // the `t.key === key` lookup `showToast`'s keyed replacement uses, so a key
    // matching nothing is already a silent no-op.
    //
    // App and host toasts still share ONE key namespace, so an app that names a
    // host key removes that toast from screen. Accepted rather than overlooked:
    // an app can already REPLACE a host toast by reusing its key through
    // `toast()`, and apps are semi-trusted (the same SDK hands them the whole
    // workspace data tree). The one thing removal reaches that replacement does
    // not is a toast raised during a WORKSPACE UNAVAILABLE WINDOW, since
    // `showToast` suppresses writes there and this does not: an app could clear
    // the "Restarting engine…" banner and leave the blocking overlay
    // unexplained until the reload restores it. Cosmetic and self-healing, and
    // the real fix is per-app key namespacing, which is deliberately out of
    // scope here (docs/plans/2026-08-07-spinning-and-dismissable-app-toasts.md).
    if (typeof payload.key === 'string' && payload.key.length > 0) removeToast(payload.key);
    return true;
  }

  return false;
}
