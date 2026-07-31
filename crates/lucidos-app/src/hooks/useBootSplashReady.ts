import { useEffect } from 'preact/hooks';
import { effect } from '@preact/signals';
import { connectionStatus, threadsLoaded } from '../store/store';
import {
  bootSplashPresent,
  bootSplashRevealSkipped,
  dismissBootSplash,
  revealBootEscape,
  setBootStatus,
} from '../utils/bootSplash';
import { WORKSPACE_ID } from '../utils/basePath';
import type { ConnectionStatus } from '../store/types';

/** Safety cap: dismiss the splash even if the workspace never reports ready, so a
 *  down/unreachable engine reveals the app's own connection/error UI instead of
 *  trapping the user on an infinite splash. */
export const BOOT_SPLASH_SAFETY_MS = 15_000;

/** Keep the splash up at least this long (since navigation start) in a final
 *  (non-reload) document so the inline mark reveal — which the workspace document
 *  plays once, see index.html — always finishes at least once before we dismiss,
 *  even if the app reports ready sooner. Matches the reveal's tail (~1.15s). */
export const BOOT_SPLASH_MIN_REVEAL_MS = 1_200;

/** Delay before the status text updates to a phase word. Set well beyond a normal
 *  cold start so a typical launch shows ONLY the baked "Opening your workspace…"
 *  until it fades (no text swap mid-launch); the update appears only on a
 *  genuinely slow/stuck load, where a phase word is actually informative. */
export const STATUS_DELAY_MS = 3_000;

/** Elapsed ms since this document started loading (≈ when the inline reveal
 *  began at first paint). `performance.now()` is time-since-navigation-start. */
function msSinceLoad(): number {
  return typeof performance !== 'undefined' ? performance.now() : BOOT_SPLASH_MIN_REVEAL_MS;
}

/** How much longer to hold a ready splash so its reveal can finish. Pure so the
 *  floor is testable without the DOM.
 *
 *  `revealSkipped` is the gateway handover (`boot-splash-formed`): the gateway
 *  boot splash already built the mark on this url, so this document plays no
 *  reveal and there is nothing to wait for. Holding the floor there would park a
 *  fully built mark on screen for another second at the end of a cold boot that
 *  was already slow, which is the opposite of what the floor is for. */
export function remainingRevealMs(elapsedMs: number, revealSkipped: boolean): number {
  if (revealSkipped) return 0;
  return Math.max(0, BOOT_SPLASH_MIN_REVEAL_MS - elapsedMs);
}

/** The workspace is "ready" — and the boot splash may fade — once the engine
 *  connection is live AND the thread list has loaded. Pure so it can be tested
 *  without the DOM/signals. */
export function isWorkspaceReady(connection: ConnectionStatus, threads: boolean): boolean {
  return connection === 'connected' && threads;
}

/** The status word shown if the splash is STILL up after {@link STATUS_DELAY_MS}
 *  — i.e. the launch is slow or stuck. Pure so it can be tested without the
 *  DOM/signals.
 *
 *  Takes the whole {@link ConnectionStatus}, not a bare `connected` boolean,
 *  because the two not-connected states carry very different evidence.
 *  `'connecting'` means no health probe has come back yet — we know nothing, so
 *  the only honest line is "Connecting…". `'disconnected'` means a probe
 *  actually FAILED.
 *
 *  Only on that failed-probe evidence do we name a cause, and only for a
 *  **direct** engine port (`isDirect`, `WORKSPACE_ID === null`): nothing
 *  lazy-starts a workspace there the way the gateway does on access, so an
 *  unreachable engine really does mean the workspace isn't running. Behind the
 *  gateway the frontend only loads after the engine is already up (the gateway
 *  serves its own "Starting engine…" splash until then), so a failed probe is a
 *  connection hiccup — keep "Connecting…".
 *
 *  Deriving "Workspace not started" from the mere ABSENCE of a connection was a
 *  false claim: the engine served this very document, and on an iOS PWA cold
 *  start the first health round-trip routinely lands after STATUS_DELAY_MS, so
 *  the splash accused a perfectly healthy workspace of being down. */
export function delayedBootStatus(connection: ConnectionStatus, isDirect: boolean): string {
  if (connection === 'connected') return 'Loading…';
  if (workspaceIsUnreachable(connection, isDirect)) return 'Workspace not started';
  return 'Connecting…';
}

/** The one state the boot splash can name a cause for AND offer a way out of: a
 *  direct engine port whose health probe actually FAILED. Nothing on that origin
 *  lazy-starts the workspace, so the gateway escape (see `revealBootEscape`) is
 *  the only action that can help.
 *
 *  Shared by the status line and the escape so the message and the affordance
 *  cannot disagree about when the workspace is unreachable. Pure, so the pairing
 *  is testable without the DOM. */
export function workspaceIsUnreachable(connection: ConnectionStatus, isDirect: boolean): boolean {
  return connection === 'disconnected' && isDirect;
}

/** Wire the splash's delayed status line and return its (idempotent) teardown.
 *  Nothing is written for the first {@link STATUS_DELAY_MS}; past it the line
 *  SUBSCRIBES to the connection rather than sampling it once — a one-shot read
 *  froze whichever state happened to be live at the delay, so a probe landing a
 *  moment later left the splash still claiming the workspace was unreachable for
 *  the rest of its life. Subscribed, the line retracts itself the instant the
 *  evidence moves.
 *
 *  Factory-shaped (same convention as `makeLongPressHandlers` /
 *  `makeDismissHandlers`) so the timing and the retraction are testable without
 *  rendering the hook. */
export function startDelayedBootStatus(isDirect: boolean): () => void {
  let stop: (() => void) | null = null;
  const timer = window.setTimeout(() => {
    stop = effect(() => {
      setBootStatus(delayedBootStatus(connectionStatus.value, isDirect));
      // Naming the cause is not enough on a direct port: the user cannot start
      // the workspace from this origin. Offer the gateway, which starts a
      // stopped workspace on the way in. Re-runs with the status, so a probe
      // that lands later retracts the message and leaves the link behind
      // whatever the splash does next (a ready workspace dismisses it).
      if (workspaceIsUnreachable(connectionStatus.value, isDirect)) revealBootEscape();
    });
  }, STATUS_DELAY_MS);
  return () => {
    clearTimeout(timer);
    stop?.();
    stop = null;
  };
}

/**
 * Drives the inline boot splash (index.html) for a WORKSPACE document: holds it
 * up — the mark playing its reveal then breathing — until the app is genuinely
 * loaded + connected AND the reveal has finished at least once, then fades it.
 * Readiness-gated (not a fixed timer), so the launch is smooth regardless of
 * connection; the min-reveal floor lets the one-time reveal complete; a safety
 * cap prevents an infinite splash when the engine is unreachable.
 */
export function useBootSplashReady(): void {
  useEffect(() => {
    if (!bootSplashPresent()) return;

    const stopStatus = startDelayedBootStatus(WORKSPACE_ID === null);
    // Set once the readiness gate first fires, so the min-reveal delay isn't
    // re-armed on every subsequent signal change.
    let dismissArmed = false;
    let revealTimer: ReturnType<typeof setTimeout> | undefined;

    // Declared as a hoisted function so the safety cap below can be wired to it
    // while `finish` still clears that cap's own timer.
    function finish() {
      clearTimeout(cap);
      stopStatus();
      dismissBootSplash();
    }

    // The safety cap goes through `finish` so it also tears down the status
    // subscription — left running it would keep writing into a splash that is
    // already gone.
    const cap = window.setTimeout(finish, BOOT_SPLASH_SAFETY_MS);

    const stop = effect(() => {
      if (dismissArmed) return;
      if (!isWorkspaceReady(connectionStatus.value, threadsLoaded.value)) return;
      dismissArmed = true;
      // Let the reveal finish at least once before dismissing.
      const remaining = remainingRevealMs(msSinceLoad(), bootSplashRevealSkipped());
      if (remaining === 0) finish();
      else revealTimer = window.setTimeout(finish, remaining);
    });

    return () => {
      clearTimeout(cap);
      if (revealTimer !== undefined) clearTimeout(revealTimer);
      stopStatus();
      stop();
    };
  }, []);
}
