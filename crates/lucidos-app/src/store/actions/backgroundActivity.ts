/**
 * Background-activity actions: reading the embedding-model status, the two
 * toasts background work may still raise, and the Expose run's narration. The
 * pure derivation lives in `store/backgroundActivity.ts`.
 *
 * Work in flight is told in the Lucidos menu's activity group, never in a
 * progress toast (ADR 0306). Two exceptions reach the toast layer:
 *
 *  - **The Expose run narrates in a toast.** The user just pressed Expose, and
 *    one of its steps blocks until they open a tailnet approval link. Settings
 *    shows no link, so a narration hidden in the menu would stall the run
 *    silently. Once the user closes the toast, later frames only update the
 *    menu: `showToast` with a key creates a missing toast, so every frame goes
 *    through the open-toast guard.
 *  - **A download this document watched reports its outcome once**, as the
 *    Expose run reports its own. Memory is quietly off for the whole download,
 *    so its end is news.
 */

import {
  showToast,
  dismissToast,
  toasts,
  engineBuilding,
  engineBuildDetail,
  frontendRefreshDetail,
  embeddingModelStatus,
  tailscaleServeRun,
} from '../store';
import {
  embeddingModelOutcome,
  tailscaleServeActivity,
  tailscaleServeOutcome,
  type ActivityAction,
} from '../backgroundActivity';
import { getEmbeddingModelStatus } from '../../api/client';
import type { EmbeddingModelStatus } from '../../api/types';
import { isTauri } from '../../utils/platform';
import { openExternalUrl } from '../../utils/openExternalUrl';
import {
  listen,
  openExternal,
  cancelTailscaleServe,
  TAILSCALE_SERVE_PROGRESS_EVENT,
  type TailscaleServeProgress,
} from '../../utils/tauri';

/** The Expose run's narration while it is in flight. */
export const SERVE_RUN_TOAST_KEY = 'tailscale-serve';

/** The Expose run's OUTCOME, a separate surface from its narration. Keyed so a
 *  second run replaces the first run's result instead of stacking a copy. */
export const SERVE_OUTCOME_TOAST_KEY = 'tailscale-serve-outcome';

/** A watched download's outcome, keyed the same way. */
export const EMBEDDING_OUTCOME_TOAST_KEY = 'embedding-model-outcome';

/** How long a settled message lingers before clearing itself. Long enough to
 *  read, short enough not to need dismissing. A failure stays up. */
const SETTLED_DISMISS_MS = 8000;

/** Whether this document has watched the embedding model DOWNLOADING. Only
 *  then may it report an outcome: `ready` is every warm-cache boot's resting
 *  state, and announcing it there would describe work that never happened. */
let downloadSeen = false;

/** The outcome last announced, so a repeated frame does not raise it again.
 *  Cleared when a download starts, so a retry that lands reports again. */
let announcedOutcome: EmbeddingModelStatus['load_state']['kind'] | null = null;

/** Bumped by every LIVE status frame. A snapshot read compares the value it
 *  captured before awaiting against this, and discards its result if a frame
 *  landed in between.
 *
 *  Without that, a snapshot in flight across an SSE transition writes the older
 *  HTTP body over the newer live state, and nothing corrects it: the loader
 *  emits its terminal `ready` frame and then returns, so there is no next frame.
 *  A `downloading` body resolving after that `ready` would spin the badge for
 *  the rest of the session. */
let liveVersion = 0;

/** Unsubscribe for the Expose progress event, or `null` when not subscribed. */
let unlistenServe: (() => void) | null = null;
/** Guards the async gap in {@link subscribeToTailscaleServeProgress}, the same
 *  way `subscribing` does for the updater: a remount must not register a second
 *  listener while the first `listen` call is still in flight. */
let subscribingServe = false;

/** Reset the per-document download memory. Test seam only. */
export function resetBackgroundActivityForTest(): void {
  downloadSeen = false;
  announcedOutcome = null;
  liveVersion = 0;
}

/** Apply a live `EmbeddingModelStatusChanged` frame. The single entry point for
 *  SSE, so the freshness counter cannot be bypassed. */
export function applyEmbeddingModelStatus(status: EmbeddingModelStatus): void {
  liveVersion += 1;
  embeddingModelStatus.value = status;
  syncEmbeddingModelOutcome();
}

/** Record a download starting, or announce how a watched one ended. Safe to
 *  call on every frame: an outcome is announced once. */
export function syncEmbeddingModelOutcome(): void {
  const state = embeddingModelStatus.value?.load_state;
  if (state?.kind === 'downloading') {
    downloadSeen = true;
    announcedOutcome = null;
    return;
  }
  if (!downloadSeen || !state || state.kind === announcedOutcome) return;
  const outcome = embeddingModelOutcome(state);
  if (!outcome) return;
  showToast(outcome.message, outcome.tone, {
    title: outcome.title,
    key: EMBEDDING_OUTCOME_TOAST_KEY,
    autoDismissMs: outcome.tone === 'error' ? undefined : SETTLED_DISMISS_MS,
  });
  // Spent only once a toast is on screen. `showToast` drops everything while
  // the workspace is unavailable, and the next frame or resume must try again.
  if (toasts.value.some((t) => t.key === EMBEDDING_OUTCOME_TOAST_KEY)) announcedOutcome = state.kind;
}

/** Turn an action DESCRIPTOR from the pure derivation into a real button
 *  action, for the Expose toast and the menu detail alike.
 *
 *  The one place that knows how to perform them, which is what keeps
 *  `store/backgroundActivity.ts` a pure function of its arguments. */
export function activityAction(action: ActivityAction | undefined) {
  if (!action) return undefined;
  switch (action.kind) {
    case 'open-url': {
      const { url } = action;
      return {
        label: action.label,
        onClick: () => {
          // The desktop app has the OS opener; anywhere else a new tab. Branching
          // rather than catching, because the Tauri bridge throws SYNCHRONOUSLY
          // off Tauri (see `openTailscaleDownload` for the same rule and the bug
          // that taught it).
          if (!isTauri()) {
            openExternalUrl(url);
            return;
          }
          openExternal(url).catch((e) => {
            showToast(`Couldn't open ${url}: ${String(e)}`, 'error');
          });
        },
      };
    }
    case 'cancel-tailscale-serve':
      return {
        label: action.label,
        onClick: () => {
          // The outcome arrives as a `cancelled` frame, so there is nothing to
          // await on the happy path. A REJECTED invoke emits no frame at all
          // though (a dead bridge, an ACL denial), so the run would keep
          // spinning with the user's click swallowed. The user pressed a
          // button, so they are owed the reason (.claude/rules/frontend.md:
          // the telemetry carve-out never covers a mutating user intent). The
          // run is deliberately left set: Rust may still be tearing it down,
          // and clearing it here would claim a cancel that did not happen.
          void cancelTailscaleServe().catch((e) => {
            showToast(`Couldn't cancel: ${String(e)}`, 'error');
          });
        },
      };
  }
}

/** Read the embedding-model snapshot and reconcile.
 *
 *  Called at startup and on window resume. Both are needed because the SSE
 *  frames are transient and never replayed: a fresh workspace starts its
 *  download before this document exists, and a backgrounded PWA sleeps through
 *  every frame in between. */
export async function loadEmbeddingModelStatus(): Promise<void> {
  try {
    const readAt = liveVersion;
    const status = await getEmbeddingModelStatus();
    // A live frame landed while this was in flight, so the response is already
    // history. Dropping it is always safe: SSE carries the newer truth, and the
    // next resume re-reads. Writing it is NOT safe, because after the loader's
    // terminal frame there is no further frame to correct the regression.
    if (readAt !== liveVersion) return;
    embeddingModelStatus.value = status;
    syncEmbeddingModelOutcome();
  } catch (e) {
    // Best-effort telemetry (frontend.md carve-out): an unsolicited startup /
    // resume probe the user did not ask for. No toast, because failing to read
    // a progress snapshot is not something to interrupt anyone with, and it is
    // self-recovering: live SSE frames keep arriving, the next resume re-reads,
    // and a genuinely broken model still announces itself through the loader's
    // own notifications.
    console.warn('[background-activity] embedding-model status read failed', e);
  }
}

/** Whether a build in flight has an elapsed time of its own to count up, so
 *  an open menu re-reads the clock. A co-located PEER's build spins the badge
 *  but reports no elapsed, so it needs no ticker. */
export function buildTimerIsLive(): boolean {
  const engineTicks = engineBuilding.value && engineBuildDetail.value?.elapsedMs != null;
  return engineTicks || frontendRefreshDetail.value?.elapsedMs != null;
}

// --- The Expose run (`tailscale serve`) ---

function serveToastIsOpen(): boolean {
  return toasts.value.some((t) => t.key === SERVE_RUN_TOAST_KEY);
}

/** Draw the run's current step into its toast, creating it if absent. */
function renderServeRun(): void {
  const activity = tailscaleServeActivity(tailscaleServeRun.value);
  if (!activity) {
    dismissToast(SERVE_RUN_TOAST_KEY);
    return;
  }
  const [title, message] = activity.note ? [activity.label, activity.note] : [undefined, activity.label];
  showToast(message, 'info', {
    title,
    key: SERVE_RUN_TOAST_KEY,
    spinning: true,
    action: activityAction(activity.action),
    secondaryAction: activityAction(activity.secondaryAction),
  });
}

/** Note that an Expose run has STARTED, before its first frame arrives, and
 *  open its toast.
 *
 *  Called from the button's own handler rather than waiting for Rust, because
 *  the IPC hop plus the CLI probe take long enough that the button would
 *  otherwise look dead for a moment. Same reason `installAppUpdate` paints its
 *  first frame on the click. */
export function beginTailscaleServeRun(): void {
  tailscaleServeRun.value = { phase: 'starting' };
  renderServeRun();
}

/** Clear the run without narrating an outcome.
 *
 *  For the case Rust could not report on at all: a rejected invoke, an ACL
 *  denial, a dead bridge. Its caller shows the error, since there is no frame
 *  carrying one. Leaving the run set would spin the badge with nothing behind
 *  it. */
export function clearTailscaleServeRun(): void {
  tailscaleServeRun.value = null;
  dismissToast(SERVE_RUN_TOAST_KEY);
}

/** Apply one `tailscale-serve-progress` frame.
 *
 *  An in-flight frame updates the run's toast, if the user left it open. A
 *  terminal one ends the run and clears the narration. Its outcome gets a toast
 *  of its OWN, so a closed narration never swallows a failure. */
export function applyTailscaleServeProgress(frame: TailscaleServeProgress): void {
  if (frame.phase === 'done' || frame.phase === 'failed' || frame.phase === 'cancelled') {
    clearTailscaleServeRun();
    const outcome = tailscaleServeOutcome(frame);
    if (outcome) {
      showToast(outcome.message, outcome.tone, {
        key: SERVE_OUTCOME_TOAST_KEY,
        // A failure is the one the user has to read and act on, so it stays up.
        autoDismissMs: outcome.tone === 'error' ? undefined : SETTLED_DISMISS_MS,
      });
    }
    return;
  }
  tailscaleServeRun.value = frame;
  // An absent toast means the user closed it, and the run's row in the Lucidos
  // menu is where they see it again.
  if (serveToastIsOpen()) renderServeRun();
}

/** Subscribe to the Rust Expose run's progress stream. Idempotent across
 *  remounts; Tauri-only.
 *
 *  Best-effort (frontend.md carve-out): this runs at startup without user
 *  intent, and a failed subscription costs the narration, not the run. The
 *  command's own rejection still reports a failure. It leaves itself
 *  unsubscribed on failure, so the next mount retries. */
export async function subscribeToTailscaleServeProgress(): Promise<void> {
  if (!isTauri() || unlistenServe || subscribingServe) return;
  subscribingServe = true;
  try {
    unlistenServe = await listen<TailscaleServeProgress>(
      TAILSCALE_SERVE_PROGRESS_EVENT,
      (e) => { applyTailscaleServeProgress(e.payload); },
    );
  } catch (e) {
    console.warn('[background-activity] tailscale serve progress subscription failed', e);
  } finally {
    subscribingServe = false;
  }
}

/** Drop the Expose progress subscription (startup cleanup). */
export function unsubscribeFromTailscaleServeProgress(): void {
  if (unlistenServe) {
    unlistenServe();
    unlistenServe = null;
  }
}
