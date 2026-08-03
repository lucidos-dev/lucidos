/**
 * Background-activity actions: reading the embedding-model status, and driving
 * the status toast behind the brand badge. The pure derivation the toast renders
 * lives in `store/backgroundActivity.ts`.
 *
 * One keyed toast narrates whatever background activity is running (see
 * `store/backgroundActivity.ts`). It opens two ways:
 *
 *  - **By itself, once**, the first time this document sees the embedding model
 *    actually DOWNLOADING. That is the fresh-workspace case in observable
 *    terms: a warm cache never enters that state, so an existing workspace stays
 *    silent, and no first-run flag or workspace-age guess is needed.
 *  - **On demand**, when the user taps the badge.
 *
 * Once open it updates in place. Once DISMISSED it stays dismissed: later
 * frames may only update a toast that is still on screen, never resurrect one
 * the user closed. That distinction is the whole reason this module exists,
 * because `showToast` with a key creates the toast when it is absent, so a bare
 * per-frame `showToast` would pop the thing back up every few hundred
 * milliseconds.
 *
 * The toast is shared, so it also tracks WHICH work it is entitled to talk
 * about: `downloadSeen` below licenses the terminal embedding-model messages,
 * which would otherwise fire in a toast opened to watch something else.
 *
 * Updates arrive two ways, and the engine rebuild is why there are two. The
 * embedding-model download and the Expose run are each PUSHED a frame per
 * change (`EmbeddingModelStatusChanged` over SSE, `tailscale-serve-progress`
 * over Tauri), so re-rendering on each frame keeps them current. A rebuild emits
 * only its transitions, and its toast shows a seconds counter, so it also drives
 * a local 1s ticker (`ensureBuildTicker`). The ticker is held to the same rule as
 * every other update path here: it may only refresh a toast already on screen.
 */

import {
  showToast,
  dismissToast,
  toasts,
  engineBuilding,
  engineBuildDetail,
  embeddingModelStatus,
  tailscaleServeRun,
} from '../store';
import {
  activityToastContent,
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

export const BACKGROUND_ACTIVITY_TOAST_KEY = 'background-activity';

/** The Expose run's OUTCOME, which is a separate surface from the in-flight
 *  narration above. Keyed so a second run replaces the first run's result
 *  instead of stacking a second copy of it. */
export const SERVE_OUTCOME_TOAST_KEY = 'tailscale-serve-outcome';

/** How long the settled message (ready / stalled / failed) lingers before
 *  clearing itself. Long enough to read, short enough not to need dismissing. */
const SETTLED_DISMISS_MS = 8000;

/** Whether the auto-open has already fired in THIS document. Deliberately not
 *  persisted: a reload during a still-running download should show the toast
 *  again, since the information is still current and still worth having. */
let autoOpened = false;

/** Whether this document has ever seen the embedding model actually
 *  DOWNLOADING. What licenses the toast to report a terminal model outcome
 *  (see `activityToastContent`): the shared toast may resolve work it narrated,
 *  and must stay silent about work it did not.
 *
 *  Close to `autoOpened` but deliberately NOT the same flag. That one is burned
 *  only once a toast is actually on screen, so a download the restart
 *  suppression swallowed still gets its one announcement later; this one
 *  records what the MODEL did, whether or not anything was rendered. */
let downloadSeen = false;

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

/** Reset the once-per-document auto-open. Test seam only. */
export function resetBackgroundActivityToastForTest(): void {
  autoOpened = false;
  downloadSeen = false;
  liveVersion = 0;
  stopBuildTicker();
}

/** Apply a live `EmbeddingModelStatusChanged` frame. The single entry point for
 *  SSE, so the freshness counter cannot be bypassed. */
export function applyEmbeddingModelStatus(status: EmbeddingModelStatus): void {
  liveVersion += 1;
  embeddingModelStatus.value = status;
  syncBackgroundActivityToast();
}

function toastIsOpen(): boolean {
  return toasts.value.some((t) => t.key === BACKGROUND_ACTIVITY_TOAST_KEY);
}

/** Turn an action DESCRIPTOR from the pure derivation into a real toast action.
 *
 *  The one place that knows how to perform them, which is what keeps
 *  `store/backgroundActivity.ts` a pure function of its arguments. */
function toastAction(action: ActivityAction | undefined) {
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
          // The outcome arrives as a `cancelled` frame; nothing to await here.
          void cancelTailscaleServe().catch(() => {});
        },
      };
  }
}

/** Render the current content into the keyed toast, creating it if absent. */
function render(): void {
  const content = activityToastContent(
    engineBuilding.value,
    embeddingModelStatus.value,
    tailscaleServeRun.value,
    downloadSeen,
    engineBuildDetail.value,
  );
  if (!content) {
    dismissToast(BACKGROUND_ACTIVITY_TOAST_KEY);
    stopBuildTicker();
    return;
  }
  showToast(content.message, content.tone, {
    key: BACKGROUND_ACTIVITY_TOAST_KEY,
    // The spinner is the "something is happening" signal; `progress` is the
    // "how far" one. A settled message needs neither.
    spinning: !content.settled,
    progress: content.progress,
    action: toastAction(content.action),
    secondaryAction: toastAction(content.secondaryAction),
    // A settled message clears itself, EXCEPT a failure: an error the user has
    // to read and act on must not vanish while they are reading it.
    autoDismissMs: content.settled && content.tone !== 'error' ? SETTLED_DISMISS_MS : undefined,
  });
  // AFTER the showToast, never before: the ticker only runs while a toast is on
  // screen, and on the first render of a freshly opened toast that is not true
  // until the line above has created it. `showToast` is also suppressed outright
  // during an engine restart, so this is the one place that knows whether a
  // toast actually exists to tick.
  ensureBuildTicker();
}

/** Read the embedding-model snapshot and reconcile the toast.
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
    syncBackgroundActivityToast();
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

/** How often the open toast re-renders while a build runs. One second, because
 *  the thing being redrawn is a seconds counter: slower and it reads as a hung
 *  build, which is the misreading the counter exists to prevent. */
const BUILD_TICK_MS = 1000;

/** The live build-timer interval, or `null` when nothing needs one. */
let buildTicker: ReturnType<typeof setInterval> | null = null;

/** Whether the toast currently has a ticking number in it: a build in flight,
 *  with an elapsed time of its own to count up. A co-located PEER's build spins
 *  the badge but reports no elapsed, so it needs no ticker. */
function buildTimerIsLive(): boolean {
  return engineBuilding.value && engineBuildDetail.value?.elapsedMs != null;
}

/** Start the 1s re-render if one is warranted and not already running, or stop
 *  the running one once it isn't.
 *
 *  Two conditions, both required, re-checked on every tick rather than assumed:
 *  a build with a live timer, and a toast actually on screen. Dropping the
 *  second would make this the one thing in the module that can resurrect a
 *  toast the user dismissed, since `render` creates the toast when it is absent.
 *  So the tick calls `render` only through the same open-toast guard every other
 *  update path uses, and retires itself the moment either condition fails. */
function ensureBuildTicker(): void {
  const wanted = buildTimerIsLive() && toastIsOpen();
  if (!wanted) {
    stopBuildTicker();
    return;
  }
  if (buildTicker !== null) return;
  buildTicker = setInterval(() => {
    if (!buildTimerIsLive() || !toastIsOpen()) {
      stopBuildTicker();
      return;
    }
    render();
  }, BUILD_TICK_MS);
}

function stopBuildTicker(): void {
  if (buildTicker !== null) {
    clearInterval(buildTicker);
    buildTicker = null;
  }
}

/** Open the status toast on demand (the brand badge was tapped). */
export function openBackgroundActivityToast(): void {
  render();
}

/** Reconcile the toast with the current activity. Safe to call on every frame.
 *
 *  Called from the `EmbeddingModelStatusChanged` SSE handler and from the
 *  engine-build version check, the two things that move the underlying state. */
export function syncBackgroundActivityToast(): void {
  const downloading = embeddingModelStatus.value?.load_state.kind === 'downloading';
  // Every writer of `embeddingModelStatus` (the SSE frame, the snapshot read)
  // ends here, so this is the one place that sees every state the model passes
  // through, and therefore the one place that can record having seen it.
  if (downloading) downloadSeen = true;

  // A real download is starting and this document has not announced one yet:
  // open unprompted, exactly once. Memory is quietly disabled for the whole
  // download, so the user is owed the warning without having to go looking.
  if (downloading && !autoOpened) {
    render();
    // Burn the one-shot only if the toast actually appeared. `showToast` is
    // suppressed outright while the engine is restarting, and marking it opened
    // regardless would spend the single auto-open on a call that rendered
    // nothing, leaving this document with no announcement at all.
    autoOpened = toastIsOpen();
    return;
  }

  // Otherwise, update what is already on screen and nothing else. An absent
  // toast here means either the user dismissed it or it was never opened, and
  // in both cases they have not asked to see this.
  if (toastIsOpen()) render();
}

// --- The Expose run (`tailscale serve`) ---

/** Note that an Expose run has STARTED, before its first frame arrives.
 *
 *  Called from the button's own handler rather than waiting for Rust, because
 *  the IPC hop plus the CLI probe take long enough that the button would
 *  otherwise look dead for a moment. Same reason `installAppUpdate` paints its
 *  first frame on the click.
 *
 *  Opens the toast unconditionally, and that is the one place this run differs
 *  from the embedding-model download beside it: the download is unsolicited
 *  news, so it announces itself once per document and never again, while this
 *  is a button the user just pressed and they are owed the narration every
 *  time. */
export function beginTailscaleServeRun(): void {
  tailscaleServeRun.value = { phase: 'starting' };
  render();
}

/** Clear the run without narrating an outcome.
 *
 *  For the case Rust could not report on at all: a rejected invoke, an ACL
 *  denial, a dead bridge. Its caller shows the error, since there is no frame
 *  carrying one. Leaving the run set would spin the badge with nothing behind
 *  it. */
export function clearTailscaleServeRun(): void {
  tailscaleServeRun.value = null;
  if (toastIsOpen()) render();
}

/** Apply one `tailscale-serve-progress` frame.
 *
 *  In-flight frames update the shared background-activity toast. A terminal one
 *  ends the run, releases the shared toast back to whatever else is in flight
 *  (or clears it), and reports its outcome in a toast of its OWN.
 *
 *  The separation is load-bearing rather than tidy. The shared toast narrates
 *  work IN FLIGHT, so a finished run contributes nothing to it: routing the
 *  outcome through it meant a failure was silently dropped whenever anything
 *  else was running, and a cancel took a live embedding-download narration down
 *  with it. */
export function applyTailscaleServeProgress(frame: TailscaleServeProgress): void {
  if (frame.phase === 'done' || frame.phase === 'failed' || frame.phase === 'cancelled') {
    // Clear first, so the shared toast re-renders without this run: it reverts
    // to a concurrent activity, or clears when there is none.
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
  // Only ever UPDATES what is on screen. An absent toast means the user closed
  // it, and the spinning badge is how they ask for it back.
  if (toastIsOpen()) render();
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
