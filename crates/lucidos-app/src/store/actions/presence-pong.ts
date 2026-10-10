/** SDK handler for the engine's `PresenceCheck` SSE broadcast — see
 *  system-knowhow/notifications.md §3 (pong protocol, freshness gate).
 *
 *  PresenceCheck is a PURE pong trigger: it no longer renders the in-app
 *  toast. The toast used to fire here, synchronously on receipt — but that
 *  raced the engine's push decision (the pong round-trips back slower than
 *  the deadline on a slow link), so a foregrounded iOS PWA could get the
 *  toast AND the OS push for the same notification. The toast is now driven
 *  by `NotificationToastRequested` (§4), which the engine emits ONLY on the
 *  push-suppressed branch — so the two surfaces are mutually exclusive by the
 *  engine's single decision, not by a page-side timing race. */

import type { PongAnswer } from '@lucidos/event-stream';
import { isPageActive } from '../../utils/pageActive';
import { isInViewport } from '../../utils/viewport';
import { appFullscreenHost } from '../appFullscreenHost';
import { appPseudoFullscreen, focusedThreadId } from '../store';
import { getDeviceId } from './devices';
import { submitPong } from './event-stream';

/** Wall-clock slack on top of `deadline_ms` before a PresenceCheck is
 *  considered stale (iOS PWA SSE-queue flush after a push tap). A late ping
 *  now only costs a discarded pong (the engine has already decided), but
 *  dropping it keeps the engine's pong accounting clean. Exported so tests
 *  assert against the same constant. */
export const STALE_GRACE_MS = 2000;

export interface PresenceCheckPayload {
  notification_id: string;
  event_id: string | null;
  deadline_ms: number;
  /** Engine wall-clock at emit time. Drives the freshness gate. */
  sent_at_ms: number;
}

/** Whether the SHELL is the surface in front of the user.
 *
 *  Presence answers "can we reach this person here, instead of pushing?", and
 *  only the shell can show them a toast. An app filling the screen means they
 *  are looking at the app, so the notification belongs to the OS.
 *
 *  That also makes a fullscreen app agree with a popped-out app window. The two
 *  look identical to the user, and a popped-out one has always taken the push.
 *  Before this they were opposite, for no reason anybody had chosen.
 *
 *  Pure over its three inputs, so it is testable without a DOM. */
export function isShellActive(
  pageActive: boolean,
  nativeFullscreen: boolean,
  pseudoFullscreen: boolean,
): boolean {
  return pageActive && !nativeFullscreen && !pseudoFullscreen;
}

/** `isShellActive` against the live signals. */
export function shellIsActive(): boolean {
  return isShellActive(
    isPageActive(),
    appFullscreenHost.value !== null,
    appPseudoFullscreen.value,
  );
}

/** This document's answer to a `PresenceCheck`, before any aggregation.
 *
 *  Deliberately NOT the device-presence heartbeat's view. That one reports
 *  whether the page is visible, which it still is under a fullscreen app. The
 *  engine reads the heartbeat only to count how many pongs to wait for, and
 *  reads the push decision from `is_active` here. */
export function buildPongAnswer(payload: PresenceCheckPayload): PongAnswer {
  return {
    device_id: getDeviceId(),
    is_active: shellIsActive(),
    focused_thread_id: focusedThreadId.value,
    event_in_viewport: payload.event_id ? isInViewport(payload.event_id) : false,
  };
}

export function handlePresenceCheck(payload: PresenceCheckPayload): void {
  if (Date.now() - payload.sent_at_ms > payload.deadline_ms + STALE_GRACE_MS) {
    return;
  }
  // Through the transport, not straight to the engine. On a shared connection
  // the worker ORs this with its other documents' answers. It then POSTs one
  // pong, which is the one the engine is waiting for.
  submitPong(payload.notification_id, buildPongAnswer(payload));
}
