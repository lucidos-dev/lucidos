import { showToast, dismissToast, removeToast, engineVersionReady, engineBuilding, engineRestarting, preferences, NEW_VERSION_TOAST_KEY, FRONTEND_UPDATE_DEFERRED_TOAST_KEY } from '../store';
import { engineVersionStatus, rebuildEngine } from '../../api/client';
import { initiateEngineRestart } from './chat-changes';
import { noteSwitchBuildId, wasSwitchDismissed } from '../../hooks/sw-update';

/** Kick off the dev engine rebuild — the "Rebuild" escape hatch behind the
 *  pending / build-failed toasts. The version-status poll (and the
 *  `EngineBuildStateChanged` SSE poke) then surfaces building → ready → the
 *  normal "Switch to new version". The engine's self-heal driver rebuilds on its
 *  own too; this is the manual override when it has given up (retry cap) or for
 *  immediate action. Toasts on failure (the user clicked, so they're owed it). */
async function triggerRebuild(): Promise<void> {
  try {
    await rebuildEngine();
  } catch (e) {
    showToast(`Failed to start rebuild: ${e instanceof Error ? e.message : String(e)}`, 'error');
  }
}

/** How often to poll the engine version-status. Dev rebuilds finish in seconds,
 *  so a short interval keeps "New version available" prompt without being chatty;
 *  the check is a cheap GET (the engine only forks `--build-id` when the on-disk
 *  binary's mtime moved). */
const ENGINE_UPDATE_POLL_MS = 4000;

const BUILD_FAILED_TOAST_KEY = 'engine-build-failed';

let pollTimer: ReturnType<typeof setInterval> | null = null;

/** Poll the engine's version status and surface the unified "New version
 *  available → Switch to new version" flow (dev half). Packaged builds report
 *  `packaged: true` and never `update_available` — their new-version source is the
 *  release updater (app-update.ts) — so this no-ops there.
 *
 *  Best-effort telemetry (frontend.md carve-out): runs on a timer without user
 *  intent, so a failed poll is logged, not toasted — the next poll retries, and
 *  the user-facing failure surface is the switch action's own toast. */
export async function checkEngineVersion(): Promise<void> {
  // Don't poll (or re-toast) while a switch is already in flight — the restart
  // toast owns the UI and the engine is on its way down.
  if (engineRestarting.value) return;
  // The switch dismissal is now a GLOBAL preference (not synchronous
  // localStorage), so until preferences load we can't tell whether this on-disk
  // build was already dismissed. Skip rather than flash an already-dismissed
  // Switch toast on cold start — the 4s poll and useStartup's post-load
  // checkEngineVersion re-run it once preferences are known. A 'failed' load
  // proceeds (fail-open surfacing is the safe default).
  if (preferences.value.status === 'not-loaded' || preferences.value.status === 'loading') return;
  let status;
  try {
    status = await engineVersionStatus();
  } catch (e) {
    console.warn('[engine-update] version-status poll failed; will retry', e);
    return;
  }
  // Packaged builds never run a background rebuild — the spinning-build badge is
  // a dev-only affordance.
  if (status.packaged) {
    engineBuilding.value = false;
    return;
  }

  if (status.build_state === 'failed') {
    engineVersionReady.value = false;
    engineBuilding.value = false;
    // A new version exists in source but the last rebuild failed. Offer a manual
    // retry so the user isn't stuck (the engine self-heal driver also retries, up
    // to a per-HEAD cap). No auto-switch — the build must succeed first.
    showToast('New engine version failed to build — see the engine log.', 'error', {
      key: BUILD_FAILED_TOAST_KEY,
      action: { label: 'Retry build', onClick: () => { void triggerRebuild(); } },
    });
    return;
  }
  // A prior failure cleared once a build starts / succeeds.
  dismissToast(BUILD_FAILED_TOAST_KEY);

  // While a build is actively in progress, never announce "new version
  // available" — wait for THIS build to flip build_state to 'ready'.
  // `update_available` compares the on-disk binary's build-id to the running
  // one, and the on-disk binary can already differ mid-build (a prior build
  // wrote it, or it's being rewritten right now), which would surface the switch
  // "before it could have been built yet". `build_state === 'ready'` is the
  // honest signal for a freshly-finished rebuild; `update_available` is the
  // backstop for a newer binary sitting on disk when no build is running (e.g.
  // after an engine restart resets build_state to 'idle').
  const ready =
    status.build_state === 'ready' ||
    (status.update_available && status.build_state !== 'building');
  // Badge (engineVersionReady, read by the ControlPanel badge) = readiness ALONE:
  // the persistent "switch available" affordance that survives a dismiss (the
  // user can still switch from the reload badge). On ARRIVAL it appears with the
  // Switch toast from this one `ready` check (INV-C arrival); the two decouple
  // only on dismiss.
  const diskId = status.disk_build_id;
  // Toast = ready AND not dismissed for THIS on-disk build (disk_build_id): the
  // one-time announcement. A dismiss defers it (badge stays) until a genuinely
  // newer build lands, mirroring the client refresh's per-served-build dismissal.
  const toastAvailable = ready && !(diskId !== undefined && wasSwitchDismissed(diskId));
  if (toastAvailable) {
    // Keep the Switch toast present alongside the badge. Keyed → idempotent: a
    // re-show updates in place, so re-running each poll neither stacks nor
    // re-animates. Record the on-disk build so a dismiss pins the right id.
    if (diskId !== undefined) noteSwitchBuildId(diskId);
    showToast('New version available.', 'info', {
      key: NEW_VERSION_TOAST_KEY,
      // "Later" defers the switch: same path as the X — remembers this on-disk
      // build and hides the toast, while the reload badge stays lit.
      secondaryAction: {
        label: 'Later',
        onClick: () => { dismissToast(NEW_VERSION_TOAST_KEY); },
      },
      action: {
        label: 'Switch to new version',
        onClick: () => { void initiateEngineRestart(); },
      },
    });
  } else if (status.source_behind_head && !status.update_available && status.build_state === 'idle') {
    // A new engine version exists in source but no fresh binary is on disk yet
    // (a mixed Apply's rebuild failed / never ran). The engine's self-heal driver
    // retries automatically, but surface a manual escape so the Switch is never a
    // dead-end. The `!update_available` guard is load-bearing: after a SUCCESSFUL
    // rebuild both `update_available` (disk binary differs) AND `source_behind_head`
    // (running engine's commit still behind HEAD) are true — that state belongs to
    // the ready→Switch branch above (and its per-build dismissal), NOT here; without
    // the guard a dismissed Switch would re-nag as "Rebuild" every poll. So this
    // fires ONLY when no switchable binary exists. Reuses NEW_VERSION_TOAST_KEY so
    // it transitions in place to the Switch toast once the rebuild lands. This state
    // is short-lived — self-heal flips it to `building` within a tick — so no
    // persistent "Later" (there's no on-disk build id to key a dismissal on anyway);
    // the X just hides it and the next poll re-derives it.
    showToast('New engine version pending.', 'info', {
      key: NEW_VERSION_TOAST_KEY,
      action: {
        label: 'Rebuild',
        onClick: () => { void triggerRebuild(); },
      },
    });
  } else {
    // No toast offered (no build ready, none pending in source, or deferred for
    // this on-disk build) → hide it. removeToast (not dismissToast) so this
    // signal-driven hide isn't recorded as a user dismissal. This also clears a
    // sticky toast once a NEW build starts (build_state 'building'), whose switch
    // would respawn onto a binary that's mid-rewrite.
    removeToast(NEW_VERSION_TOAST_KEY);
  }
  engineVersionReady.value = ready;
  // A background rebuild is in flight (Apply kicked it off) but not yet ready to
  // switch onto — drives the spinning-refresh brand badge.
  engineBuilding.value = status.build_state === 'building';
}

/** SSE handler for the engine's `EngineBuildStateChanged` poke. The engine emits
 *  it on every dev background-rebuild transition (building → ready/failed) so the
 *  connected client learns of a build over the live stream rather than waiting on
 *  the throttled 4s poll (which iOS suspends on a backgrounded PWA — the reason
 *  the "building" spinner never showed).
 *
 *  It is a pure POKE: it re-runs the authoritative version-status read rather than
 *  trusting the event payload, so `engineBuilding` is still set ONLY from
 *  `build_state === 'building'`. A stale/duplicate poke therefore just triggers a
 *  harmless authoritative re-check, never a spurious spin. */
export function handleEngineBuildStateChanged(): void {
  void checkEngineVersion();
}

/** Start the periodic engine version-status poll (immediately + every
 *  {@link ENGINE_UPDATE_POLL_MS}). Idempotent. */
export function startEngineUpdateChecks(): void {
  if (pollTimer !== null) return;
  void checkEngineVersion();
  pollTimer = setInterval(() => { void checkEngineVersion(); }, ENGINE_UPDATE_POLL_MS);
}

/** Stop the periodic check (startup cleanup). */
export function stopEngineUpdateChecks(): void {
  if (pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

/** Wall-clock budget after which a `FrontendUpdateDeferred` is too stale to
 *  render — same rationale as the notification toast's freshness gate: a frame
 *  that flushes late from a suspended iOS PWA's SSE queue (after the user
 *  already Switched, which delivers the deferred change) would show a
 *  now-false hint. Generous — dev SSE is low-latency, so this only drops
 *  genuinely stale flushes. Exported so tests assert against the same value. */
export const DEFERRED_HINT_STALE_AFTER_MS = 10_000;

export interface FrontendUpdateDeferredPayload {
  /** Engine wall-clock (ms) at emit time. Drives the freshness gate. */
  sent_at_ms: number;
}

/** SSE handler for the engine's `FrontendUpdateDeferred` signal. The engine
 *  emits it when a frontend-only Apply's served-client advance was deferred
 *  because an engine version change is pending (engine::frontend_refresh
 *  INV-A): the rebuilt client is for the NEW engine and can't be served on the
 *  running old one, so the change ships when the user Switches. Surface a keyed
 *  hint so the just-applied frontend change reads as queued, not ignored.
 *  Keyed → repeated frontend-only applies while a Switch is pending coalesce
 *  into one toast.
 *
 *  Deliberately NO Switch action here. The deferral can fire while a mixed
 *  rebuild is still `build_state: 'building'` (the early INV-A branch), and in
 *  that window switching would respawn onto a mid-rewrite / old binary — which
 *  is exactly why `checkEngineVersion` above withholds the Switch affordance
 *  until the build is `ready`. This is a pure hint; the guarded version toast /
 *  reload badge (driven by the version-status poll) is the sole Switch surface,
 *  so it can never offer a switch before there's a ready engine to switch to. */
export function handleFrontendUpdateDeferred(payload: FrontendUpdateDeferredPayload): void {
  if (Date.now() - payload.sent_at_ms > DEFERRED_HINT_STALE_AFTER_MS) {
    return;
  }
  // Pops unsolicited (the user applied a change, didn't ask for a toast) → don't
  // steal focus. Persists until acknowledged or until the Switch clears it
  // (initiateEngineRestart removes this key).
  //
  // Sticky, action-less hint → give it an explicit OK the user acknowledges,
  // rather than only a corner X: `dismissable: false` drops the redundant close
  // X so the OK is the sole dismiss (it does the same job as the X did). The OK
  // just dismisses — deliberately NOT a Switch, since the deferral can fire
  // mid-build when switching is unsafe (see the doc comment above).
  showToast(
    "Frontend change applied — it'll take effect when you switch to the new version.",
    'info',
    {
      key: FRONTEND_UPDATE_DEFERRED_TOAST_KEY,
      noAutofocus: true,
      dismissable: false,
      action: {
        label: 'OK',
        onClick: () => { dismissToast(FRONTEND_UPDATE_DEFERRED_TOAST_KEY); },
      },
    },
  );
}
