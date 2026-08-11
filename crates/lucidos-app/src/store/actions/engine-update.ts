import { showToast, dismissToast, removeToast, engineVersionReady, engineBuilding, engineBuildDetail, engineRestarting, preferences, NEW_VERSION_TOAST_KEY, FRONTEND_UPDATE_DEFERRED_TOAST_KEY, FRONTEND_UPDATE_STRANDED_TOAST_KEY } from '../store';
import { engineVersionStatus, rebuildEngine } from '../../api/client';
import type { EngineVersionStatus, PendingCommits } from '../../api/client';
import { initiateEngineRestart } from './chat-changes';
import { noteSwitchBuildId, wasSwitchDismissed } from '../../hooks/sw-update';
import { syncBackgroundActivityToast } from './backgroundActivity';

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

/** The ONE writer of the building pair: the boolean that spins the badge, and
 *  the detail the status toast narrates from.
 *
 *  A single entry point because `pollEngineVersion` decides "not building" on
 *  three different paths (packaged, build failed, and the ordinary end), and two
 *  independent assignments are exactly how a stale narration survives its build.
 *  That failure already happened once here, which is what the `finally` comment
 *  in `checkEngineVersion` records. With one writer, `engineBuilding === false`
 *  implies no leftover detail by construction.
 *
 *  `anchoredAt` is stamped HERE, from the client clock, right as the response
 *  lands: the counter then advances as `elapsedMs + (now - anchoredAt)` and never
 *  subtracts an engine timestamp from a browser one. */
function setEngineBuilding(building: boolean, status?: EngineVersionStatus): void {
  engineBuilding.value = building;
  if (!building) {
    engineBuildDetail.value = null;
    return;
  }
  engineBuildDetail.value = {
    elapsedMs: status?.build_elapsed_ms ?? null,
    anchoredAt: Date.now(),
    pendingCommits: groupedCommits(status?.pending_commits),
  };
}

/** The wire's pending-commits payload, or `null` when this engine cannot speak
 *  the grouped shape.
 *
 *  This is the ONE place a version-status response becomes store state, so it is
 *  where a cross-version payload has to be caught. A new frontend against an OLD
 *  engine is not a race here, it is the ordinary state of the very window this
 *  toast narrates: an Apply rebuilds and republishes `dist/` in seconds while
 *  the engine binary keeps serving the old version until the user clicks
 *  *Switch*. That engine answers with the pre-grouping `{ total, subjects }`
 *  shape, whose `groups` is `undefined`, and reading `.length` off it throws
 *  inside the badge's own render.
 *
 *  Absent is the honest answer rather than a rebuilt list: an engine that
 *  predates the grouping cannot say which commits are features, and its
 *  `subjects` still carry the merge lines this whole change removed. So the
 *  toast shows the elapsed time alone until the Switch lands, which is exactly
 *  what it showed before any of this existed. */
function groupedCommits(pending: EngineVersionStatus['pending_commits']): PendingCommits | null {
  return pending && Array.isArray(pending.groups) ? pending : null;
}

/** Poll the engine's version status and surface the unified "New version
 *  available → Switch to new version" flow (dev half). Packaged builds report
 *  `packaged: true` and never `update_available` — their new-version source is the
 *  release updater (app-update.ts) — so this no-ops there.
 *
 *  Best-effort telemetry (frontend.md carve-out): runs on a timer without user
 *  intent, so a failed poll is logged, not toasted — the next poll retries, and
 *  the user-facing failure surface is the switch action's own toast. */
export async function checkEngineVersion(): Promise<void> {
  try {
    await pollEngineVersion();
  } finally {
    // `engineBuilding` is one of the two things the background-activity toast
    // narrates, and `pollEngineVersion` clears it on several EARLY-return paths
    // (packaged, build failed). Syncing only at its happy-path end left an open
    // toast reading "Building new version" after a build had already failed,
    // with nothing to correct it on a workspace whose model is long since
    // cached. A `finally` covers every exit, including the throw path.
    //
    // Safe to call unconditionally: it only ever updates a toast that is
    // already on screen, and never opens one.
    syncBackgroundActivityToast();
  }
}

async function pollEngineVersion(): Promise<void> {
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
    setEngineBuilding(false);
    return;
  }

  if (status.build_state === 'failed') {
    engineVersionReady.value = false;
    setEngineBuilding(false);
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

  // A background rebuild of the shared binary is in flight — this engine's own
  // (build_state === 'building') OR a CO-LOCATED peer's. Co-located workspaces
  // share ONE target/ + ONE build lock, so a peer's build advances the binary
  // THIS engine serves. When THIS workspace lost the lock its own rebuild
  // `SkippedLocked` → build_state fell back to 'idle', but a build IS running:
  // show the building spinner (below) and withhold the manual "Rebuild" toast,
  // exactly as we do for our own build.
  const sharedBuilding = status.shared_build_in_progress === true;

  // A new version is announced ONLY when there is genuinely something newer to
  // switch onto — `update_available`, which the engine now derives from "the
  // on-disk binary is readable, differs from the running one, and is not
  // provably OLDER than it" (engine_version::disk_binary_is_upgrade).
  //
  // `build_state === 'ready'` deliberately does NOT count on its own. A rebuild
  // can finish successfully and produce nothing new — a no-op cargo run, or one
  // whose uplifted binary predates the running engine — and announcing that as a
  // new version offers a Switch that respawns onto the same (or an older) binary.
  // That is half of the 2026-07-26 endless-toast loop: the ~10s self-heal driver
  // cycled build_state idle → building → ready forever, and every `ready` re-showed
  // the toast (docs/plans/2026-07-26-downgrade-switch-toast-loop.md).
  //
  // The `!== 'building'` term stays: the on-disk binary can already differ
  // mid-build (a prior build wrote it, or it is being rewritten right now), and
  // switching then respawns onto a half-written binary.
  const ready = status.update_available && status.build_state !== 'building';
  // Badge (engineVersionReady, read by the brand badge) = readiness ALONE:
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
  } else if (status.source_behind_head && !status.update_available && status.build_state !== 'building' && !sharedBuilding) {
    // A new engine version exists in source but no fresh binary is on disk yet
    // AND nothing is building it (a mixed Apply's rebuild failed / never ran, and
    // no co-located peer holds the shared build lock). The engine's self-heal
    // driver retries automatically, but surface a manual escape so the Switch is
    // never a dead-end. Two load-bearing guards:
    //  - `!update_available`: after a SUCCESSFUL rebuild both `update_available`
    //    (disk binary differs) AND `source_behind_head` (running engine's commit
    //    still behind HEAD) are true — that state belongs to the ready→Switch
    //    branch above (and its per-build dismissal), NOT here; without the guard a
    //    dismissed Switch would re-nag as "Rebuild" every poll.
    //  - `!sharedBuilding`: a co-located peer building the shared binary is NOT a
    //    stuck state — its build WILL advance the binary → Switch. In the
    //    multi-workspace case a lost-the-lock workspace has build_state 'idle' but
    //    a build is in flight; showing "Rebuild" there is wrong (it'd just
    //    SkippedLock again) and misleading. That case shows the spinner instead
    //    (engineBuilding below); the pending toast is reserved for genuinely stuck.
    // So this fires ONLY when no switchable binary exists and nothing is building.
    // `!== 'building'` rather than `=== 'idle'` deliberately ('failed' already
    // returned above via the build-failed toast, so this admits idle + ready): a
    // rebuild that COMPLETED without producing anything newer leaves build_state
    // 'ready' forever, and that is the most stuck state there is. Gating on 'idle'
    // would answer it with silence — hiding both the pending version and this
    // escape hatch (docs/plans/2026-07-26-downgrade-switch-toast-loop.md, INV-5b).
    // Reuses NEW_VERSION_TOAST_KEY so it transitions in place to the Switch toast
    // once a rebuild lands. Short-lived (self-heal / a peer build flips it) so no
    // persistent "Later" (no on-disk build id to key a dismissal on anyway); the X
    // just hides it and the next poll re-derives it.
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
  // switch onto — drives the spinning-refresh brand badge. True for THIS engine's
  // own build (build_state === 'building') OR when a co-located peer is building
  // the shared binary this workspace is waiting on: source behind, no fresh binary
  // yet, and our own build_state fell back to 'idle' (lost the shared lock →
  // SkippedLocked). The `!update_available` term drops the spinner the instant a
  // switchable binary lands, even within a probe window, handing off to the
  // ready→Switch surface above.
  setEngineBuilding(
    status.build_state === 'building' ||
      (sharedBuilding &&
        status.source_behind_head === true &&
        !status.update_available &&
        status.build_state === 'idle'),
    status,
  );
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

export interface FrontendUpdateStrandedPayload {
  /** Absolute path of the dist/ the engine serves from. */
  served_dir: string;
  /** Whether that path lies inside a coding-agent worktree — the known cause,
   *  and the one with a specific fix, so it gets a specific message. */
  served_in_worktree: boolean;
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

/** SSE handler for the engine's `FrontendUpdateStranded` signal — a frontend-only
 *  Apply whose rebuild did not reach this client within the engine's wait
 *  (engine::frontend_refresh). Distinct from the deferred hint above in the one way
 *  that matters: no Switch is coming that will deliver it, so it must not say there is.
 *
 *  **Only the worktree case is permanent, and the wording must respect that.** A
 *  worktree-pinned `dist/` can never receive the rebuild — the build-watch
 *  republishes a different directory — so that message says "will not appear" and
 *  asks for operator action. Any other timeout is *recoverable*: a build slower than
 *  the wait, or a briefly-stopped watch, still lands and the engine's ~10s peer sync
 *  advances the snapshot by itself. Telling the user their change is lost there would
 *  be false, so that branch says "hasn't arrived yet" and names the likely cause.
 *
 *  Both are `warning` rather than `info`: an applied change that isn't visible is
 *  never normal, even when it self-heals. Before 2026-07-26 the engine returned
 *  silently here and the only symptom was "my change did nothing"
 *  (docs/plans/2026-07-26-worktree-pinned-stack-guard.md).
 *
 *  Same freshness gate as the deferred hint: a late SSE-queue flush arriving after
 *  the stack was already fixed shouldn't raise a now-false alarm. */
export function handleFrontendUpdateStranded(payload: FrontendUpdateStrandedPayload): void {
  if (Date.now() - payload.sent_at_ms > DEFERRED_HINT_STALE_AFTER_MS) {
    return;
  }
  const message = payload.served_in_worktree
    ? `Frontend change applied but it will not appear: the engine is serving a coding-agent worktree (${payload.served_dir}), which the build-watch never rebuilds. Relaunch the stack from the real checkout.`
    : `Frontend change applied but not served yet — ${payload.served_dir} hasn't rebuilt. It will appear on its own if the build lands; if it doesn't, check the build-watch.`;
  showToast(message, 'warning', {
    key: FRONTEND_UPDATE_STRANDED_TOAST_KEY,
    noAutofocus: true,
    dismissable: false,
    action: {
      label: 'OK',
      onClick: () => { dismissToast(FRONTEND_UPDATE_STRANDED_TOAST_KEY); },
    },
  });
}
