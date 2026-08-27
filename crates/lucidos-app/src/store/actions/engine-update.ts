import { showToast, dismissToast, removeToast, toasts, engineVersionReady, engineVersionPending, engineRebuildWedged, engineBuilding, engineBuildDetail, engineRestarting, preferences, NEW_VERSION_TOAST_KEY, FRONTEND_UPDATE_DEFERRED_TOAST_KEY, FRONTEND_UPDATE_STRANDED_TOAST_KEY } from '../store';
import { engineVersionStatus, rebuildEngine } from '../../api/client';
import type { EngineVersionStatus, PendingCommits } from '../../api/client';
import { initiateEngineRestart } from './chat-changes';
import { noteAnnouncedEngineVersion, wasEngineVersionDismissed } from '../../hooks/sw-update';
import { syncBackgroundActivityToast } from './backgroundActivity';
import { errorDetail } from '../../utils/errorDetail';

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
    // A bare `e.message` prints the raw browser string for a cancelled or
    // timed-out fetch, which is the shape an iOS suspend gives this click.
    showToast(`Failed to start rebuild: ${errorDetail(e)}`, 'error');
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

/** The ONE writer of the pending pair: the badge's third state, and whether a
 *  rebuild has been proved unable to resolve it.
 *
 *  Same argument as `setEngineBuilding` above. The poll decides "not pending" on
 *  several paths (packaged, build failed, a switchable build landed, a build in
 *  flight), and a wedged flag left standing after the state it describes has
 *  gone would tint the badge and strip the Rebuild button for a workspace that
 *  is merely building. One writer makes `pending === false` imply "not wedged"
 *  by construction. */
function setEngineVersionPending(pending: boolean, wedged = false): void {
  engineVersionPending.value = pending;
  engineRebuildWedged.value = pending && wedged;
}

/** Is the engine-version toast currently on screen?
 *
 *  Both shapes of it share `NEW_VERSION_TOAST_KEY`, deliberately: they are one
 *  announcement about one thing, and sharing the key is what lets a pending
 *  toast turn into the Switch toast in place when a build lands, rather than
 *  popping out and a second one popping in. */
function versionToastIsOpen(): boolean {
  return toasts.value.some((t) => t.key === NEW_VERSION_TOAST_KEY);
}

/** Which shape the engine-version announcement takes, or `null` for nothing to
 *  announce. One value rather than a pair of booleans, because `ready` and
 *  `wedged` are mutually exclusive and a two-boolean encoding makes the
 *  combination of them representable at every call site that has to avoid it. */
type VersionAnnouncement = 'ready' | 'pending' | 'wedged';

/** Draw the engine-version announcement in whichever of its three shapes fits.
 *  Keyed, so this both CREATES the toast and updates one already on screen, and
 *  re-running it on every poll neither stacks nor re-animates. The caller owns
 *  the question of whether it is entitled to create.
 *
 *  The three differ in what the user can actually do, which is the only thing
 *  worth distinguishing:
 *
 *  - **ready**: a built version is waiting, so offer the Switch, and a "Later"
 *    that defers it.
 *  - **pending**: new code exists with no version behind it, so offer the
 *    Rebuild that produces one, and the same "Later".
 *  - **wedged**: the same as pending except a rebuild has already been proved
 *    futile. Offering the button anyway is the loop the user reported: it runs a
 *    few-second no-op build and puts this toast straight back. So the button
 *    goes, the tone rises to `warning`, and the copy names the one thing that
 *    does resolve it. With nothing left to do but acknowledge, it takes the
 *    `dismissable: false` + explicit OK shape the deferred-frontend hint below
 *    already uses, rather than leaving a bare X as the only affordance. */
function renderVersionToast(shape: VersionAnnouncement): void {
  const later = {
    label: 'Later',
    onClick: () => { dismissToast(NEW_VERSION_TOAST_KEY); },
  };
  if (shape === 'ready') {
    showToast('New version available.', 'info', {
      key: NEW_VERSION_TOAST_KEY,
      // "Later" defers the switch: same path as the X, remembering this on-disk
      // build and hiding the toast while the reload badge stays lit.
      secondaryAction: later,
      action: {
        label: 'Switch to new version',
        onClick: () => { void initiateEngineRestart(); },
      },
    });
    return;
  }
  if (shape === 'wedged') {
    showToast(
      'New engine version pending, and rebuilding cannot deliver it: a build for this commit ' +
        'already succeeded without producing one. Relaunch the stack from your checkout.',
      'warning',
      {
        key: NEW_VERSION_TOAST_KEY,
        dismissable: false,
        action: {
          label: 'OK',
          onClick: () => { dismissToast(NEW_VERSION_TOAST_KEY); },
        },
      },
    );
    return;
  }
  showToast('New engine version pending.', 'info', {
    key: NEW_VERSION_TOAST_KEY,
    secondaryAction: later,
    action: {
      label: 'Rebuild',
      onClick: () => { void triggerRebuild(); },
    },
  });
}

/** The repeatable failure the user has acknowledged, so the 4s poll stops
 *  redrawing it.
 *
 *  Without this the OK button is a lie. `showToast` re-creates a keyed toast
 *  the moment it is gone, so an acknowledged toast came straight back on the
 *  next poll, forever. That is tolerable for the retryable shape, which is a
 *  standing nag about something the user CAN act on here. It is not tolerable
 *  for a toast whose whole message is "there is nothing to do about this from
 *  the phone in your hand".
 *
 *  Keyed on the cause, not a boolean, so a DIFFERENT failure still surfaces.
 *  Cleared when the build state leaves `failed`, since the next failure is a
 *  new event even if it reads the same. */
let acknowledgedBuildFailure: string | null = null;

/** Draw the build-failed toast, in whichever of its two shapes the failure
 *  earns. Keyed, so a re-poll updates it in place rather than stacking.
 *
 *  Both shapes SAY WHAT BROKE. The copy this replaced said only "see the engine
 *  log". That names a file no phone can open, and the phone is where this gets
 *  read.
 *
 *  The shapes differ only in whether the user can do anything with Retry:
 *
 *  - **ordinary**: the next build could genuinely succeed, so offer Retry.
 *  - **repeatable**: a rebuild is proved futile, because the same cached
 *    artifact is replayed byte for byte. Offering the button anyway is the loop
 *    the user reported. So the button goes, the tone rises to `warning`, and
 *    the copy names the one command that does resolve it. This is exactly the
 *    `wedged` treatment above, applied to a failing build rather than a
 *    fruitless successful one, down to the `dismissable: false` + explicit OK.
 *
 *  An ABSENT failure is not a third shape. The engine could not read its own
 *  build output, so the cause is unknown and Retry stays. That is the same
 *  fallible-but-worth-trying position as an ordinary error. */
function renderBuildFailedToast(failure: EngineVersionStatus['build_failure']): void {
  // A compiler error line does not end in a full stop, so it would run
  // straight into the instruction after it.
  const sentence = (s: string) => (/[.!?]$/.test(s.trim()) ? s.trim() : `${s.trim()}.`);
  // "Ask a coding agent" is the honest instruction on a phone: the workspace
  // can fix its own build, and it is the only remedy that does not require
  // being sat at the checkout.
  const cause = sentence(failure?.summary ?? 'the engine could not read the build output');
  if (failure?.repeatable) {
    if (acknowledgedBuildFailure === cause) return;
    const fix = failure.remedy
      ? `Run \`${failure.remedy}\` in your checkout, or ask a coding agent to fix it.`
      : 'Ask a coding agent to fix it.';
    showToast(
      `New engine version failed to build, and retrying cannot help: ${cause} ${fix}`,
      'warning',
      {
        key: BUILD_FAILED_TOAST_KEY,
        dismissable: false,
        action: {
          label: 'OK',
          onClick: () => {
            acknowledgedBuildFailure = cause;
            dismissToast(BUILD_FAILED_TOAST_KEY);
          },
        },
      },
    );
    return;
  }
  // A retryable failure keeps nagging while it stands, which is the behavior
  // this toast has always had: there IS something to do about it, and it
  // clears itself the moment a build starts or succeeds.
  showToast(`New engine version failed to build: ${cause}`, 'error', {
    key: BUILD_FAILED_TOAST_KEY,
    action: { label: 'Retry build', onClick: () => { void triggerRebuild(); } },
  });
}

/** The announced engine version id the last poll saw, so the badge tap below can
 *  name the version it is asking to see again without re-reading the status. */
let lastAnnouncedId: string | undefined;

/** The version id the user re-opened from the badge, which is what buys the
 *  toast an exemption from its own dismissal on later polls.
 *
 *  A flag rather than "the toast happens to be on screen", and the distinction
 *  is load-bearing: the dismissal is a WORKSPACE-GLOBAL preference, deliberately,
 *  so that putting the toast away on the phone puts it away on the laptop too.
 *  Inferring the exemption from an open toast would keep it open on every device
 *  that already had it up, which is the one thing a global dismissal exists to
 *  prevent. Only a tap ON THIS DEVICE sets this.
 *
 *  Never cleared, and it does not need to be: it is only ever consulted together
 *  with a matching announced id and an open toast, so dismissing the re-opened
 *  toast retires it, and a genuinely newer version stops matching. */
let reopenedVersionId: string | undefined;

/** Forget which version was last announced and which was re-opened. Test seam
 *  only, mirroring `resetBackgroundActivityToastForTest`: these two outlive any
 *  signal a test resets, so without it one test's badge tap grants the next
 *  test's toast an exemption it never asked for. */
export function resetEngineVersionToastForTest(): void {
  lastAnnouncedId = undefined;
  reopenedVersionId = undefined;
  acknowledgedBuildFailure = null;
}

/** Re-open the pending version toast on demand (the brand badge was tapped).
 *
 *  The badge's counterpart to the poll's create-or-update rule: the poll will
 *  not resurrect a toast the user dismissed, so the badge is how they ask for it
 *  back, which is what makes dismissing it safe in the first place. Reads the
 *  signals rather than taking a status, because the badge that calls this is
 *  rendered from those same signals and there is no fresher truth to be had
 *  between polls.
 *
 *  Only the PENDING shape is re-openable. The ready state's badge is not
 *  clickable at all: it falls through to the Lucidos menu, where the Restart row
 *  carries the switch. */
export function openEngineVersionToast(): void {
  if (!engineVersionPending.value) return;
  reopenedVersionId = lastAnnouncedId;
  renderVersionToast(engineRebuildWedged.value ? 'wedged' : 'pending');
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
    setEngineVersionPending(false);
    return;
  }

  if (status.build_state === 'failed') {
    engineVersionReady.value = false;
    setEngineBuilding(false);
    // A failed build owns its own toast and its own Retry, so the pending
    // surface stands down rather than putting a second badge and a second
    // rebuild button on screen for the same stuck version.
    setEngineVersionPending(false);
    renderBuildFailedToast(status.build_failure);
    return;
  }
  // A prior failure cleared once a build starts / succeeds.
  dismissToast(BUILD_FAILED_TOAST_KEY);
  acknowledgedBuildFailure = null;

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
  // Also the badge (`engineVersionReady`, read by the brand badge): readiness
  // ALONE, the persistent "switch available" affordance that survives a dismiss.
  // On ARRIVAL it appears with the Switch toast from this one check (INV-C
  // arrival); the two decouple only on dismiss.
  const ready = status.update_available && status.build_state !== 'building';

  // A new engine version exists in source but no fresh binary is on disk yet
  // AND nothing is building it (a mixed Apply's rebuild failed / never ran, and
  // no co-located peer holds the shared build lock). Two load-bearing guards:
  //  - `!update_available`: after a SUCCESSFUL rebuild both `update_available`
  //    (disk binary differs) AND `source_behind_head` (running engine's commit
  //    still behind HEAD) are true. That state belongs to the ready/Switch
  //    surface and its per-build dismissal, NOT here. It is also what makes
  //    `ready` and `pending` mutually exclusive by construction rather than by
  //    the order of an if/else chain.
  //  - `!sharedBuilding`: a co-located peer building the shared binary is NOT a
  //    stuck state, since its build WILL advance the binary and surface the
  //    Switch. In the multi-workspace case a lost-the-lock workspace has
  //    build_state 'idle' but a build is in flight; showing "Rebuild" there is
  //    wrong (it'd just SkippedLock again) and misleading. That case shows the
  //    spinner instead (engineBuilding below); pending is for genuinely stuck.
  // `!== 'building'` rather than `=== 'idle'` deliberately ('failed' already
  // returned above via the build-failed toast, so this admits idle + ready): a
  // rebuild that COMPLETED without producing anything newer leaves build_state
  // 'ready' forever, and that is the most stuck state there is. Gating on 'idle'
  // would answer it with silence, hiding both the pending version and its escape
  // hatch (docs/plans/2026-07-26-downgrade-switch-toast-loop.md, INV-5b).
  const pending =
    status.source_behind_head === true &&
    !status.update_available &&
    status.build_state !== 'building' &&
    !sharedBuilding;
  // ...and the engine has PROVED that rebuilding cannot resolve it: a build for
  // this HEAD already finished and produced nothing switchable. Never derived
  // here from `build_state === 'ready'`, which looks like the same thing and
  // isn't: a build that completed before newer commits landed would read as
  // wedged when a rebuild would genuinely help, and only the engine knows which
  // HEAD a finished build was built from.
  const wedged = pending && status.rebuild_wedged === true;

  // The identity of the version being announced, whichever shape it takes: the
  // on-disk build when there is one to switch onto, the checkout's HEAD when the
  // version exists only in source. Recorded whether or not the toast is on
  // screen, so the badge can re-open a dismissed toast and a dismiss of THAT
  // still pins the right id.
  const announcedId = ready ? status.disk_build_id : pending ? status.head_commit : undefined;
  if (announcedId !== undefined) noteAnnouncedEngineVersion(announcedId);
  lastAnnouncedId = announcedId;
  // Deferred for THIS version: the user dismissed it and nothing newer has
  // arrived since. The badge stays lit either way; the dismissal defers only the
  // toast, exactly as it does for the client refresh.
  const deferred = announcedId !== undefined && wasEngineVersionDismissed(announcedId);
  // ...unless the user asked for it back from the badge on THIS device and it is
  // still up. Both terms are required: the id so a newer version is announced on
  // its own merits, and the open check so dismissing the re-opened toast retires
  // the exemption instead of making it permanent.
  const reopened =
    announcedId !== undefined && reopenedVersionId === announcedId && versionToastIsOpen();

  const shape: VersionAnnouncement | null = ready ? 'ready' : pending ? (wedged ? 'wedged' : 'pending') : null;
  if (shape !== null && (!deferred || reopened)) {
    renderVersionToast(shape);
  } else {
    // Nothing to announce, or deferred and not re-opened here, so hide it.
    // removeToast (not dismissToast) so this signal-driven hide isn't recorded
    // as a user dismissal. This also clears the toast once a NEW build starts
    // (build_state 'building'), whose switch would respawn onto a binary that's
    // mid-rewrite, and it is what carries a dismissal made on ANOTHER device
    // across to this one: the preference is workspace-global on purpose, so a
    // poll that sees it must close a toast this device still has up.
    removeToast(NEW_VERSION_TOAST_KEY);
  }
  engineVersionReady.value = ready;
  setEngineVersionPending(pending, wedged);
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
  /** What the build-watch last said went wrong, when it said anything. Absent
   *  for a healthy build, and on any stack whose watcher predates the status
   *  file it reads. Present, it is the actual answer to "why did nothing
   *  appear", so it replaces the guess. */
  build_error?: string;
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
/** Pure: what to tell the user about a stranded frontend Apply.
 *
 *  Three cases, and the order matters. A worktree-pinned stack is permanent and
 *  keeps its own advice. A reported build failure is the actual answer, so it
 *  replaces the guess rather than being appended to it. Everything else keeps
 *  the recoverable wording, which must not claim the change is lost.
 *
 *  Exported so all three are testable without a toast. */
export function strandedMessage(payload: FrontendUpdateStrandedPayload): string {
  if (payload.served_in_worktree) {
    return `Frontend change applied but it will not appear: the engine is serving a coding-agent worktree (${payload.served_dir}), which the build-watch never rebuilds. Relaunch the stack from the real checkout.`;
  }
  const failure = payload.build_error?.trim();
  if (failure) {
    return `Frontend change applied but the build is failing, so nothing new is being served: ${failure}`;
  }
  return `Frontend change applied but not served yet. ${payload.served_dir} hasn't rebuilt. It will appear on its own if the build lands; if it doesn't, check the build-watch.`;
}

export function handleFrontendUpdateStranded(payload: FrontendUpdateStrandedPayload): void {
  if (Date.now() - payload.sent_at_ms > DEFERRED_HINT_STALE_AFTER_MS) {
    return;
  }
  const message = strandedMessage(payload);
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
