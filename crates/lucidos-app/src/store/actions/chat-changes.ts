import { showToast, showConfirm, dismissToast, removeToast, changes, appliedChanges, lazyChanges, findChangeById, changesHasMore, changesLoadingMore, restartRequired, restartGroups, applyingChangeIds, applyingNowThreadIds, applyAllInProgress, threadMap, effectiveThreadStatus, isMidTurn, TOAST_AUTO_DISMISS_MS, engineRestarting, engineStartedAt, engineVersion, latestEngineVersion, engineNewVersionReady, enginePackaged, NEW_VERSION_TOAST_KEY, FRONTEND_UPDATE_DEFERRED_TOAST_KEY } from '../store';
import { changeToastMessage } from './changeToast';
import { toFailed } from '../types';
import type { Loadable } from '../types';
import type { RestartGroup } from '../store';
import { applyChange as apiApply, discardChange as apiDiscard, applyAllChanges as apiApplyAll, cancelApplyAllChanges as apiCancelApplyAll, discardAllChanges as apiDiscardAll, revertChange as apiRevert, fetchChanges as apiFetchChanges, getChangeById as apiGetChangeById, restartEngine, ApiError, isTransportError } from '../../api/client';
import { isTauri } from '../../utils/platform';
import { invoke } from '../../utils/tauri';
import { isNewerVersion } from '../../utils/version';
import { errorDetail, isAbortError } from '../../utils/errorDetail';
import { focusThread } from './threads';
import type { Change } from '../../api/client';

/** Shared key for the engine-restart PROGRESS toast (initiateEngineRestart) and
 *  the restart FAILURE toast. It NO LONGER carries a pre-switch "New version
 *  available" warning — that surface is owned solely by the poll-driven
 *  engine-new-version toast (engine-update.ts), which fires only once the
 *  background rebuild is actually `ready`. */
export const RESTART_TOAST_KEY = 'restart-required';
export const RESTART_LS_KEY = 'lucidos-restart-required';

export const RESTART_GROUPS_LS_KEY = 'lucidos-restart-groups';
const LEGACY_RESTART_REASONS_LS_KEY = 'lucidos-restart-reasons';

/** Marks that an engine restart is IN FLIGHT (vs. merely pending). `engineRestarting`
 *  and the two-phase progress toast are in-memory only, so a page reload mid-restart
 *  would otherwise lose them and `restoreRestartToast` would wrongly fall back to the
 *  pre-restart "Engine restart required" warning. This key lets restore re-show the
 *  PROGRESS toast instead. Payload: the engine's `started_at` BEFORE the restart (so
 *  completion can be detected as a `started_at` change after reload — restore seeds
 *  it back into engineStartedAt) and `packaged` (so the restored phase matches the
 *  live initial message: packaged has no build step). */
export const RESTART_IN_FLIGHT_LS_KEY = 'lucidos-restart-in-flight';

interface RestartInFlight {
  startedAt: string | null;
  /** Whether this restart delivers a new engine version — restores the correct
   *  progress-toast wording (new-version vs. plain) on a mid-restart reload. */
  newVersion: boolean;
}

/** Persist that a restart just started. Called from initiateEngineRestart. */
function markRestartInFlight(newVersion: boolean): void {
  const payload: RestartInFlight = {
    startedAt: engineStartedAt.value,
    newVersion,
  };
  localStorage.setItem(RESTART_IN_FLIGHT_LS_KEY, JSON.stringify(payload));
}

/** Clear the in-flight marker. MUST be called at every site where
 *  `engineRestarting` flips back to false (reconnect success, restart timeout,
 *  spawn-failure revert) so a restored progress toast can never hang. Exported
 *  for connection.ts's completion/timeout paths. */
export function clearRestartInFlight(): void {
  localStorage.removeItem(RESTART_IN_FLIGHT_LS_KEY);
}

/** Read the in-flight marker, tolerant of a malformed/legacy value. */
function readRestartInFlight(): RestartInFlight | null {
  const raw = localStorage.getItem(RESTART_IN_FLIGHT_LS_KEY);
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw) as Partial<RestartInFlight>;
    return {
      startedAt: typeof parsed.startedAt === 'string' ? parsed.startedAt : null,
      newVersion: parsed.newVersion === true,
    };
  } catch {
    localStorage.removeItem(RESTART_IN_FLIGHT_LS_KEY);
    return null;
  }
}

/** Record an applied change as part of a thread group, mark the engine as
 *  needing a restart, and refresh the toast. Merges into the existing group
 *  for `threadId` (concat new commits, dedupe, refresh title) or appends.
 *  Empty commits are kept so the user still sees that the thread contributed. */
export function addRestartGroup(group: RestartGroup): void {
  const existing = restartGroups.value;
  const idx = existing.findIndex(g => g.threadId === group.threadId);
  if (idx === -1) {
    restartGroups.value = [...existing, group];
  } else {
    const merged: RestartGroup = {
      threadId: group.threadId,
      threadTitle: group.threadTitle,
      commits: dedupePreservingOrder([...existing[idx].commits, ...group.commits]),
    };
    const next = existing.slice();
    next[idx] = merged;
    restartGroups.value = next;
  }
  persistRestartGroups();
  restartRequired.value = true;
  syncRestartToast();
}

function dedupePreservingOrder(items: string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const item of items) {
    if (!seen.has(item)) {
      seen.add(item);
      out.push(item);
    }
  }
  return out;
}

function persistRestartGroups(): void {
  const groups = restartGroups.value;
  if (groups.length > 0) {
    localStorage.setItem(RESTART_GROUPS_LS_KEY, JSON.stringify(groups));
    localStorage.removeItem(LEGACY_RESTART_REASONS_LS_KEY);
  } else {
    localStorage.removeItem(RESTART_GROUPS_LS_KEY);
  }
}

/** Progress text for the in-flight restart status toast. The restart only respawns
 *  (Apply rebuilt the binary in the background / the updater already installed it),
 *  so there is no build phase at restart time and the message stays stable for the
 *  whole window. Its wording depends on whether the restart actually delivers a NEW
 *  engine version vs. a plain respawn of the running one — a plain restart (the
 *  reload glyph / SystemPage "Restart engine?" with nothing pending) must NOT claim
 *  "Starting new version…". */
const RESTART_NEW_VERSION_MESSAGE = 'Starting new version…';
const RESTART_PLAIN_MESSAGE = 'Restarting engine…';

/** The progress message for a restart, chosen by whether it delivers a new version. */
function restartProgressMessage(newVersion: boolean): string {
  return newVersion ? RESTART_NEW_VERSION_MESSAGE : RESTART_PLAIN_MESSAGE;
}

/** Set restarting state, show info toast, and trigger the engine restart.
 *
 *  Routing by mode (the `packaged` signal comes from /health):
 *   - packaged + Tauri  → `restart_service` Tauri command runs
 *     `launchctl kickstart -k`. Most reliable here — the GUI process can drive
 *     launchd even if the engine is wedged/unreachable.
 *   - packaged + browser/PWA (no Tauri) → POST /restart; the engine kickstarts
 *     its own LaunchAgent (the dev rebuild script isn't in the bundle).
 *   - dev (Tauri or web) → POST /restart spawns `web-dev.sh --engine-only`.
 *
 *  In every case the engine goes away and `checkConnection()` clears
 *  `engineRestarting` on reconnect (started_at change). */
export async function initiateEngineRestart(): Promise<void> {
  // Single version surface: the switch replaces the poll-driven "New version
  // available → Switch to new version" toast with the progress toast below.
  // Remove it here (the canonical switch entry point) so every path — the toast's
  // own button, the control-panel reload glyph, the SystemPage dialog — collapses
  // to one toast instead of stacking "Starting new version…" on top of the
  // still-visible "New version available." toast. Use removeToast (structural),
  // NOT dismissToast: clicking Switch is ACTING on the prompt, not deferring it,
  // so it must not mark this on-disk build dismissed (which would suppress the
  // toast for a build still sitting on disk if the switch then FAILS). The badge
  // is unaffected either way — dismissToast no longer clears engineVersionReady
  // (dismiss = defer, badge persists), so a failed switch keeps the reload-glyph
  // affordance regardless. A successful switch clears the signal via
  // connection.ts's engineRestarted path anyway.
  removeToast(NEW_VERSION_TOAST_KEY);
  // Same collapse for the "frontend change applies on Switch" hint — the switch
  // the user just started IS what delivers that queued change, so fold it into
  // the progress toast rather than leaving it stacked. removeToast (structural),
  // not dismissToast, for the same acting-not-deferring reason as above.
  removeToast(FRONTEND_UPDATE_DEFERRED_TOAST_KEY);
  // Decide the progress wording from the SAME predicate that drives the switch
  // badge (engineNewVersionReady) — so the toast can never disagree with the badge,
  // and can only claim a new version when the engine's own version check says the
  // running binary and the one we'll respawn onto actually differ: in dev the
  // running build-id vs the on-disk build-id (version-status `update_available`,
  // once the rebuild is ready); packaged, the installed vs latest release. A plain
  // restart (reload glyph / SystemPage with nothing newer) reads "Restarting
  // engine…"; a genuine switch reads "Starting new version…". No lies.
  const newVersion = engineNewVersionReady();
  engineRestarting.value = true;
  // Persist the in-flight state so a page reload mid-restart restores the
  // PROGRESS toast (restoreRestartToast) instead of the pre-restart warning.
  // Records the pre-restart started_at so reconnect detection still fires after
  // the reload (the everOrRestarting gate in connection.ts), plus `newVersion` so
  // the restored toast keeps the correct wording. Cleared at every engineRestarting
  // flip-false site below + in connection.ts.
  markRestartInFlight(newVersion);
  // Light, dismissible status toast — the UI is NOT deactivated during a restart
  // anymore (the gateway boot splash + GET-gate + SSE reconnect make it a
  // recoverable non-event), so this is just a "why is it briefly unresponsive"
  // hint the user can dismiss. It carries a spinner (spinning: true) to signal
  // ongoing work. The wording is stable for the whole window (no build phase at
  // restart time) and is chosen by `newVersion` above. showWhileUnavailable: true
  // keeps it visible past the central suppression in showToast (which still eats
  // read-path / SW-update noise during the window); the key de-dupes and lets
  // started_at detection dismiss it on reconnect.
  showToast(restartProgressMessage(newVersion), 'info', { key: RESTART_TOAST_KEY, showWhileUnavailable: true, spinning: true });
  try {
    if (enginePackaged.value && isTauri()) {
      // Drive launchd directly from the desktop shell — works even if the
      // engine is unresponsive. invoke rejects with a string error.
      await invoke('restart_service');
      // Service is being killed + respawned; reconnect detection takes over.
      return;
    }
    await restartEngine();
  } catch (e) {
    if (e instanceof ApiError) {
      // Spawn-failure path: engine rejected restart and is still alive.
      // Revert the indicator so the UI doesn't freeze on "Restarting…".
      engineRestarting.value = false;
      clearRestartInFlight();
      showToast(`Restart failed: ${e.reason}`, 'error', { key: RESTART_TOAST_KEY });
      return;
    }
    if (typeof e === 'string') {
      // restart_service (Tauri) rejects with a plain string. The service is
      // still alive — revert the indicator and surface the reason.
      engineRestarting.value = false;
      clearRestartInFlight();
      showToast(`Restart failed: ${e}`, 'error', { key: RESTART_TOAST_KEY });
      return;
    }
    // Network rejection after a 2xx: the engine is being killed.
    // Leave engineRestarting set; checkConnection() clears it on reconnect.
  }
}

/** Confirm, then restart. The single confirm-then-restart entry point behind
 *  every Settings restart control, so the dev "Rebuild & Restart" (System >
 *  Overview) and the packaged "Restart Engine" (System > Debugging) cannot drift
 *  on what the dialog says or offers. The dialog lists the applied changes this
 *  restart activates, and on the desktop app offers restarting the GUI client as
 *  a second, lighter action: `restart_app` re-execs the window shell only and
 *  leaves the always-on service (and therefore every running thread) alone. */
export async function confirmAndRestartEngine(): Promise<void> {
  const extraAction = isTauri()
    ? {
        label: 'Restart App',
        onClick: () => {
          invoke('restart_app').catch((e: unknown) => {
            showToast(`Failed to restart app: ${e}`, 'error');
          });
        },
      }
    : undefined;
  const groups = restartGroups.value;
  const details = groups.length > 0
    ? {
        intro: 'These changes will be applied:',
        groups: groups.map(g => ({ header: g.threadTitle, items: g.commits })),
      }
    : undefined;
  if (await showConfirm('Restart engine?', 'Restart', { extraAction, variant: 'default', details })) {
    await initiateEngineRestart();
  }
}

function isEngineOutdated(): boolean {
  const ev = engineVersion.value;
  const lev = latestEngineVersion.value;
  return !!(ev && lev && isNewerVersion(lev, ev));
}

/** Persist (or clear) the restart-pending state driven by `restartRequired` so
 *  the control-panel badge and the restart confirm dialog (SystemPage) survive a
 *  page reload.
 *
 *  It does NOT show a pre-switch toast, and (in dev) does NOT light the
 *  "New version available" badge at Apply time. That whole engine surface — the
 *  poll-driven engine-new-version toast (engine-update.ts) AND the control-panel
 *  badge / reload-glyph highlight (`engineNewVersionReady()` in store.ts)
 *  — fires only once the background rebuild is actually `ready`, so nothing can
 *  claim "available" before the build finishes. `restartRequired` here still
 *  drives the restart-pending persistence + the client-refresh ordering guard
 *  (client-update.ts holds a client refresh until after the engine switch), which
 *  is a separate concern from that visible badge.
 *
 *  An in-flight restart owns RESTART_TOAST_KEY via the two-phase progress toast
 *  (initiateEngineRestart / restoreRestartToast) — leave it untouched here so a
 *  re-sync (SSE reconnect, startup/resume refreshChangesState, a freshly-applied
 *  ChangeApplied → addRestartGroup) can't wipe the progress toast. */
export function syncRestartToast(): void {
  if (engineRestarting.value) {
    if (restartRequired.value) localStorage.setItem(RESTART_LS_KEY, 'true');
    return;
  }
  if (restartRequired.value) {
    localStorage.setItem(RESTART_LS_KEY, 'true');
  } else {
    localStorage.removeItem(RESTART_LS_KEY);
    restartGroups.value = [];
    persistRestartGroups();
    // Clear any lingering keyed toast (e.g. a stale restart-failure toast) once
    // the pending state resolves; the progress toast is excluded by the
    // engineRestarting guard above.
    dismissToast(RESTART_TOAST_KEY);
  }
}

/** Rehydrate the per-thread restart groups from localStorage (best-effort). */
function restoreRestartGroupsFromStorage(): void {
  try {
    const saved = localStorage.getItem(RESTART_GROUPS_LS_KEY);
    if (saved) restartGroups.value = JSON.parse(saved) as RestartGroup[];
  } catch {
    localStorage.removeItem(RESTART_GROUPS_LS_KEY);
  }
}

/** Restore the restart toast from localStorage on startup. Called before the
 *  async refreshChangesState() so the toast is visible immediately, even if the
 *  API call is slow or fails.
 *
 *  Two cases, in priority order:
 *   1. A restart was IN FLIGHT when the page unloaded (the user reloaded
 *      mid-restart). Restore the PROGRESS toast + `engineRestarting`, and resume
 *      completion detection so checkConnection still fires "Engine restarted" on
 *      reconnect. This takes precedence — re-showing the pre-restart warning here
 *      would nag the user to start a restart already underway.
 *   2. A restart is merely PENDING (`RESTART_LS_KEY`). Restore `restartRequired`
 *      + the restart groups so the control-panel badge and restart confirm dialog
 *      reappear. No toast — the engine "New version available" toast is owned by
 *      the poll (engine-update.ts) once the rebuild is `ready`. */
export function restoreRestartToast(): void {
  localStorage.removeItem(LEGACY_RESTART_REASONS_LS_KEY);

  const inFlight = readRestartInFlight();
  if (inFlight) {
    restoreRestartGroupsFromStorage();
    restartRequired.value = true;
    engineRestarting.value = true;
    // Seed the pre-restart started_at so checkConnection's completion check sees
    // the new engine's started_at as a genuine restart (and NOT the dev build
    // phase, where the old engine is still up with this same started_at). The
    // restored `engineRestarting` itself unlocks that detection across the reload
    // (see the everOrRestarting gate in connection.ts), so completion still fires
    // and the flag can't hang.
    engineStartedAt.value = inFlight.startedAt;
    // Restore the same stable progress wording initiateEngineRestart chose (from
    // the persisted `newVersion`); checkConnection clears it all on reconnect.
    // syncRestartToast is intentionally NOT called — the engineRestarting guard
    // would suppress it anyway.
    showToast(restartProgressMessage(inFlight.newVersion), 'info', { key: RESTART_TOAST_KEY, showWhileUnavailable: true, spinning: true });
    return;
  }

  if (localStorage.getItem(RESTART_LS_KEY) === 'true') {
    restoreRestartGroupsFromStorage();
    restartRequired.value = true;
    syncRestartToast();
  }
}

/** Reconcile the optimistic Apply Now state (`applyingNowThreadIds` + the
 *  sticky `applying-<threadId>` spinner toast) against backend truth fetched on
 *  resume / reconnect / startup.
 *
 *  The per-thread state is normally cleared by the live ChangeApplied /
 *  ChangeApplyFailed SSE event. If that event is missed — an iOS PWA suspend, an
 *  SSE reconnect gap — the spinner toast sticks "Applying changes…" forever and
 *  the WaitingBanner stays on a disabled "Apply..." even though the change
 *  already applied. This mirrors the `apply_all_in_progress` rehydration.
 *
 *  A thread is still genuinely applying iff its change is still pending (the row
 *  flips to `applied` only on success, staying pending through harden / merge /
 *  conflict resolution) OR the thread is mid-turn (CC is running the apply, or
 *  hasn't proposed the change yet). Anything else means the apply resolved while
 *  we weren't listening — drop the optimistic state and resolve the toast:
 *  "Applied" when the change landed in the applied list, otherwise dismiss it.
 *  `pending` is the complete unbounded list, so absence is authoritative. */
function reconcileApplyingNow(pending: Change[], applied: Change[]): void {
  const tracked = applyingNowThreadIds.value;
  if (tracked.size === 0) return;
  const pendingThreadIds = new Set(
    pending.map((c) => c.thread_id).filter((id): id is string => !!id),
  );
  const next = new Map(tracked);
  let changed = false;
  for (const threadId of tracked.keys()) {
    if (pendingThreadIds.has(threadId)) continue; // change still pending → applying
    const thread = threadMap.value.get(threadId);
    if (thread && isMidTurn(effectiveThreadStatus(thread))) continue; // CC running the apply
    next.delete(threadId);
    changed = true;
    const key = `applying-${threadId}`;
    const appliedChange = applied.find((c) => c.thread_id === threadId);
    if (appliedChange) {
      showToast(changeToastMessage('Applied', threadId, appliedChange.description), 'success', {
        key,
        onClick: () => focusThread(threadId),
        autoDismissMs: TOAST_AUTO_DISMISS_MS,
      });
    } else {
      dismissToast(key);
    }
  }
  if (changed) applyingNowThreadIds.value = next;
}

/** Fetch changes from backend and update all related signals.
 *  On a transient wake failure — a TimeoutError (10s client timeout) OR a
 *  transport-layer TypeError (Safari "Load failed" on a stale HTTP/2 connection)
 *  — we retry once before bothering the user: the iOS PWA fires this from
 *  `runResumeSync` after every visibilitychange, and the cellular/Wi-Fi/Tailscale
 *  radio just-waking case can hang or drop the first request even when the engine
 *  is responding fast; the retry lands on a now-warm connection.
 *
 *  A browser-cancelled AbortError AND a transport TypeError that fails even the
 *  retry are swallowed silently (see the final catch): this path has no manual
 *  AbortController, so both are the browser failing an in-flight fetch on an iOS
 *  PWA freeze / radio handoff / Tailscale reconnect — transient page-lifecycle
 *  noise. The engine is reachable via SSE (which keeps the list live) and the next
 *  runResumeSync re-syncs; the connection dot (connection.ts, debounced ~20s) is
 *  the honest sustained-outage surface, so a per-fetch toast here is just noise.
 *  A TimeoutError that survives the retry still surfaces: that's the stronger
 *  "waited the full window and got nothing" signal, and this is ONE request per
 *  wake, so the deadline really is evidence about the endpoint. Mirrors
 *  `loadUnreadNotifications`. It deliberately no longer mirrors the two
 *  per-thread event fetches (thread-loading.ts): those are fanned out one
 *  request per loaded thread, so a single outage fires every deadline at once
 *  and they treat a timeout as transient. */
export function refreshChangesState(): void {
  apiFetchChanges({ limit: 15 })
    .catch(e => {
      if ((e instanceof DOMException && e.name === 'TimeoutError') || isTransportError(e)) {
        return apiFetchChanges({ limit: 15 });
      }
      throw e;
    })
    .then(state => {
      const applied = state.applied || [];
      changes.value = { status: 'loaded', data: state.pending };
      appliedChanges.value = { status: 'loaded', data: applied };
      changesHasMore.value = state.has_more_applied;
      restartRequired.value = state.restart_required || isEngineOutdated();
      // Backend is the source of truth across page reloads — the live
      // ChangeApplied SSE event isn't replayed, so the toast detail would
      // otherwise be lost.
      restartGroups.value = (state.restart_groups ?? []).map(g => ({
        threadId: g.thread_id ?? '',
        threadTitle: g.thread_title ?? 'unknown',
        commits: g.commits,
      }));
      // Backend is the source of truth across page reloads for the Apply All
      // batch too: the ApplyAllBatchStarted SSE that set this isn't replayed,
      // so without this the sticky "Applying changes…" toast vanishes on reload
      // while the batch is still running. The effects.ts edge-guard shows/hides
      // the toast off this signal.
      applyAllInProgress.value = state.apply_all_in_progress ?? false;
      // Same rehydration for the per-thread Apply Now state: its optimistic
      // spinner toast + WaitingBanner "Apply..." clear only on the live
      // ChangeApplied/ChangeApplyFailed SSE event, so a missed event (iOS PWA
      // suspend, an SSE reconnect gap) strands them even though the apply
      // finished. Reconcile against the freshly-fetched backend truth.
      reconcileApplyingNow(state.pending, applied);
      // The update badge is NOT lit from the applied-changes list here. It shares
      // the toast's single honest source of truth — the build-id check
      // (syncClientUpdateFromBuild), which runs on startup/resume/SW-activate and
      // sets the badge true only when the loaded bundle is genuinely older than the
      // served /sw.js. Lighting it from "a frontend change was applied since page
      // load" led the real update (the rebuilt bundle may not be served yet) and
      // could disagree with the toast.
      syncRestartToast();
    })
    .catch(e => {
      // Browser-cancelled fetch (AbortError) OR a transport-layer TypeError
      // (Safari "Load failed") that failed even the retry above: no manual
      // AbortController on this path, so both are page-lifecycle / reachability
      // noise on an iOS PWA wake over a flaky link (radio handoff, Tailscale
      // reconnect), not a real outage. Leave the already-loaded list intact
      // (don't paint a spurious "Failed to fetch changes: Load failed" or flip
      // the view to a failed state) — the next runResumeSync re-syncs, SSE keeps
      // the list live while connected, and the connection dot is the honest
      // sustained-outage surface. A TimeoutError still surfaces (see the retry
      // doc above), which is where this parts company with the per-thread event
      // fetches: theirs are a fan-out, so a timeout says nothing about them.
      if (isAbortError(e) || isTransportError(e)) return;
      changes.value = toFailed<Change[]>(e);
      appliedChanges.value = toFailed<Change[]>(e);
      showToast(`Failed to fetch changes: ${errorDetail(e)}`, 'error');
    });
}

/** Apply a single pending change by ID. */
export async function applySingleChange(id: string): Promise<void> {
  try {
    const result = await apiApply(id);
    if (result.status === 'hardening') {
      // Hardening recovery: backend spawned a hardening session that will auto-apply.
      // Track the change as "applying" so ChangesPanel shows persistent state.
      // The user-facing toast is fired by the MissingHardeningDetected SSE
      // handler (thread-sync.ts) — like merge conflict, so it surfaces uniformly
      // across Apply Now / Apply All / recovery and doesn't double-fire here.
      applyingChangeIds.value = new Set([...applyingChangeIds.value, id]);
    }
  } catch (e) {
    showToast(errorDetail(e) || 'Failed to apply change', 'error');
  }
}

/** Discard a single pending change by ID. */
export async function discardSingleChange(id: string): Promise<void> {
  try {
    await apiDiscard(id);
  } catch (e) {
    showToast(errorDetail(e) || 'Failed to discard change', 'error');
  }
}

/** Apply all changes. */
export async function applyAllChanges(): Promise<void> {
  // Optimistic busy state: the batch applies the first change synchronously and
  // drives the rest in the background — including a multi-minute pause while it
  // hardens an unhardened member — so reflect "in progress" the instant the
  // user clicks. ApplyAllBatchCompleted (SSE) clears it; an immediate HTTP
  // error clears it in the catch below. Set before the await so a double-click
  // can't fire a second batch in the click→SSE gap.
  applyAllInProgress.value = true;
  try {
    // Both first-change outcomes that warrant a toast — hardening
    // (status === 'hardening') and merge conflict (conflict_thread_id) — are
    // surfaced by their SSE handlers in thread-sync.ts (MissingHardeningDetected
    // / MergeConflictDetected), uniform with single Apply. We deliberately do
    // NOT fire an HTTP-response toast here. Those SSE toasts are keyed and
    // transition in place to "applied" / "resolved" (or dismiss) once the
    // change resolves; an unkeyed HTTP toast can't be reached by that resolver,
    // so it dangles forever as a stale "resolving automatically" warning even
    // after the conflict is fixed and the batch applies (the bug this avoids).
    // The bulk button stays "Applying..." via applyAllInProgress until
    // ApplyAllBatchCompleted (SSE) clears it.
    await apiApplyAll();
  } catch (e) {
    // No batch was started — drop the optimistic busy state so the button
    // doesn't stay stuck on "Applying...".
    applyAllInProgress.value = false;
    showToast(errorDetail(e) || 'Failed to apply changes', 'error');
  }
}

/** Cancel the running Apply All batch (from the sticky batch toast). Aborts the
 *  in-flight hardening/merge and stops applying the rest; already-applied
 *  changes stay applied, the remainder return to pending. The
 *  ApplyAllBatchCompleted SSE clears `applyAllInProgress` and dismisses the
 *  toast — here we optimistically swap the toast to "Canceling..." (replacing
 *  the Cancel action so a second click can't fire) for immediate feedback. */
export async function cancelApplyAllBatch(): Promise<void> {
  showToast('Canceling apply...', 'info', { key: 'apply-all-batch', spinning: true, dismissable: false });
  try {
    await apiCancelApplyAll();
  } catch (e) {
    showToast(errorDetail(e) || 'Failed to cancel apply', 'error');
  }
}

/** Discard all changes. */
export async function discardAllChanges(): Promise<void> {
  try {
    const result = await apiDiscardAll();
    if (result.failed > 0) {
      showToast(`${result.failed} change(s) failed to discard: ${result.errors.join('; ')}`, 'error');
    }
  } catch (e) {
    showToast(errorDetail(e) || 'Failed to discard changes', 'error');
  }
}

/** Revert a previously applied change. */
export async function revertChange(id: string): Promise<void> {
  try {
    await apiRevert(id);
  } catch (e) {
    showToast(errorDetail(e) || 'Failed to revert change', 'error');
  }
}

function setLazyChange(id: string, value: Loadable<Change>): void {
  const next = new Map(lazyChanges.value);
  next.set(id, value);
  lazyChanges.value = next;
}

/** Fetch a `Change` row on demand when its id falls outside the
 *  `changes`/`appliedChanges` windows. No-op if already cached, in flight, or
 *  known unfetchable — the `loading` and `failed` entries in `lazyChanges`
 *  serve as the dedup token and negative cache respectively. */
export async function ensureChangeLoaded(id: string): Promise<void> {
  if (findChangeById(id)) return;
  if (lazyChanges.value.has(id)) return;

  setLazyChange(id, { status: 'loading' });
  try {
    const change = await apiGetChangeById(id);
    setLazyChange(id, { status: 'loaded', data: change });
  } catch (e) {
    setLazyChange(id, toFailed<Change>(e));
  }
}

/** Load the next page of applied changes (infinite scroll). Pagination
 *  needs the last row's `resolved_at` as the cursor — only the `loaded`
 *  state has it. */
export async function loadMoreChanges(): Promise<void> {
  if (changesLoadingMore.value || !changesHasMore.value) return;

  const loadable = appliedChanges.value;
  if (loadable.status !== 'loaded') return;
  const current = loadable.data;
  if (current.length === 0) return;

  const lastItem = current[current.length - 1];
  const resolvedAt = lastItem.resolved_at;
  if (!resolvedAt) return;
  const beforeTs = new Date(resolvedAt).getTime() / 1000;

  changesLoadingMore.value = true;
  try {
    const data = await apiFetchChanges({ limit: 15, before: beforeTs });
    appliedChanges.value = { status: 'loaded', data: [...current, ...(data.applied || [])] };
    changesHasMore.value = data.has_more_applied;
  } catch (e) {
    showToast(`Failed to load more changes: ${errorDetail(e)}`, 'error');
  } finally {
    changesLoadingMore.value = false;
  }
}
