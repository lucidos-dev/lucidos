import { useEffect } from 'preact/hooks';
import { effect } from '@preact/signals';
import { connectionStatus, threadsLoaded } from '../store/store';
import { bootSplashPresent, dismissBootSplash, setBootStatus } from '../utils/bootSplash';
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
const STATUS_DELAY_MS = 3_000;

/** Elapsed ms since this document started loading (≈ when the inline reveal
 *  began at first paint). `performance.now()` is time-since-navigation-start. */
function msSinceLoad(): number {
  return typeof performance !== 'undefined' ? performance.now() : BOOT_SPLASH_MIN_REVEAL_MS;
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
 *  A **direct** engine port (`isDirect`, `WORKSPACE_ID === null`) does not
 *  auto-start: only the gateway lazy-starts a workspace on access, so a still-
 *  unconnected direct context means the workspace simply isn't running — say so
 *  rather than the misleading "Opening your workspace…". Behind the gateway the
 *  frontend only loads after the engine is already up (the gateway serves its own
 *  "Starting engine…" splash until then), so a stall there is a genuine
 *  connection hiccup — keep "Connecting…". */
export function delayedBootStatus(connected: boolean, isDirect: boolean): string {
  if (connected) return 'Loading…';
  if (isDirect) return 'Workspace not started';
  return 'Connecting…';
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

    const cap = window.setTimeout(dismissBootSplash, BOOT_SPLASH_SAFETY_MS);
    const statusTimer = window.setTimeout(() => {
      setBootStatus(
        delayedBootStatus(connectionStatus.value === 'connected', WORKSPACE_ID === null),
      );
    }, STATUS_DELAY_MS);
    // Set once the readiness gate first fires, so the min-reveal delay isn't
    // re-armed on every subsequent signal change.
    let dismissArmed = false;
    let revealTimer: ReturnType<typeof setTimeout> | undefined;

    const finish = () => {
      clearTimeout(cap);
      clearTimeout(statusTimer);
      dismissBootSplash();
    };

    const stop = effect(() => {
      if (dismissArmed) return;
      if (!isWorkspaceReady(connectionStatus.value, threadsLoaded.value)) return;
      dismissArmed = true;
      // Let the reveal finish at least once before dismissing.
      const remaining = Math.max(0, BOOT_SPLASH_MIN_REVEAL_MS - msSinceLoad());
      if (remaining === 0) finish();
      else revealTimer = window.setTimeout(finish, remaining);
    });

    return () => {
      clearTimeout(cap);
      clearTimeout(statusTimer);
      if (revealTimer !== undefined) clearTimeout(revealTimer);
      stop();
    };
  }, []);
}
