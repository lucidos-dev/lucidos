import { connectionStatus, databaseReachable, dismissToast, removeToast, showToast, workspaceName, workspacePath, engineStartedAt, lucidosRelease, lucidosReleaseDirty, engineVersion, latestEngineVersion, latestTauriAppVersion, enginePackaged, llmConfigured, configuredProviders, updateAvailable, focusedThreadId, threadMap, engineRestarting, threadsLoaded, restartRequired, engineVersionReady, TOAST_AUTO_DISMISS_MS, THREAD_LIST_REFRESH_TOAST_KEY } from '../store';
import { checkHealth, API_BASE, isTransportError } from '../../api/client';
import type { HealthInfo } from '../../api/client';
import { errorDetail, isAbortError } from '../../utils/errorDetail';
import { connectThreadEvents, disconnectThreadEvents } from './thread-sync';
import { loadAllThreads, loadThreadEvents, refreshThreadEvents, clearForcedRetries } from './thread-loading';
import { refreshChangesState, clearRestartInFlight, RESTART_LS_KEY, RESTART_TOAST_KEY } from './chat-changes';
import { loadUnreadNotifications } from './notifications';
import { flushUndeliveredComposeDrafts } from './compose';
import { isNewerVersion } from '../../utils/version';
import { syncClientUpdateFromBuild } from './client-update';
import { gatewayPickerHref } from '../../utils/basePath';

/** User-facing copy when `submitChat` couldn't reach the engine — laptop
 *  woke up to a stale connection, the engine is genuinely down, etc.
 *  Avoid claiming the engine is "disconnected": the engine is local to the
 *  user's machine and almost never down. The browser tab's stream to it is
 *  what broke — naming it that way + telling the user the recovery action
 *  is more honest than blaming the engine. */
export function getUnreachableEngineMsg(): string {
  const target = API_BASE || window.location.origin;
  return `Could not reach engine at ${target} — check your connection and reload`;
}

/** True once we've been connected at least once (distinguishes initial connect from reconnect). */
let hasEverConnected = false;

/** sessionStorage one-shot so the cold-start bounce fires at most once until we
 *  next connect. Raw `sessionStorage` (NOT workspace-namespaced) — like
 *  main.tsx's `recoverFromBrokenContext` guard — so it survives the cross-slug
 *  hop to the picker and is shared per-tab. Cleared on the first successful
 *  connect so a LATER cold session (after a real drop + reload) can bounce again. */
const BOUNCE_GUARD_KEY = 'lucidos-picker-bounce';

/** Pure: should a cold boot that can't reach its engine bounce to the workspace
 *  picker? Only when we have NEVER connected this session (so an established
 *  session that briefly drops is not yanked away mid-work), a picker is reachable
 *  (`null` on a legacy direct engine with no gateway), and we haven't already
 *  bounced (the one-shot). */
export function shouldBounceToPicker(opts: {
  connectedEver: boolean;
  pickerHref: string | null;
  alreadyBounced: boolean;
}): boolean {
  return !opts.connectedEver && opts.pickerHref !== null && !opts.alreadyBounced;
}

/** Cold-start recovery: if the very first connect never lands, return to the
 *  workspace picker (the always-reachable recovery surface) instead of stranding
 *  the user in a cached shell that can't load — the reported PWA
 *  "auto-open-into-an-unreachable-engine" case. One-shot guarded and loop-safe:
 *  the picker target `/~/?pick` stands the cold-start auto-open redirect down, so
 *  the picker renders its list rather than bouncing straight back. No-op once
 *  connected, with no picker (legacy root), or after a prior bounce. */
export function bounceToPickerIfStranded(): void {
  const pickerHref = gatewayPickerHref();
  let alreadyBounced = false;
  try {
    alreadyBounced = sessionStorage.getItem(BOUNCE_GUARD_KEY) === '1';
  } catch {
    /* storage off — treat as not-yet-bounced */
  }
  if (!shouldBounceToPicker({ connectedEver: hasEverConnected, pickerHref, alreadyBounced })) return;
  try {
    sessionStorage.setItem(BOUNCE_GUARD_KEY, '1');
  } catch {
    /* storage off — bounce anyway; the one-shot just isn't persisted */
  }
  // pickerHref is non-null here (shouldBounceToPicker guarded it).
  window.location.replace(pickerHref as string);
}

/** Consecutive `/health` failures tolerated before flipping the dot to
 *  disconnected. The engine is local and almost never down, and the dot is
 *  driven SOLELY by this 5s poll (it does not reflect SSE liveness) — so a lone
 *  failed GET is far more likely a transient blip (iOS radio nap, Tailscale
 *  latency spike, HTTP/2 coalescing hiccup after the PWA backgrounds) than a
 *  real outage. We require MAX_SUPPRESSED_FAILURES+1 consecutive failures
 *  (~20s at 5s polling) before showing red, mirroring the MIN_RECONNECT_SUCCESSES
 *  hysteresis on the way back to green. This is what stops the dot blinking red
 *  on iOS PWA when nothing is actually disconnected. */
let consecutiveFailures = 0;
const MAX_SUPPRESSED_FAILURES = 3;

/** Consecutive health successes while disconnected — prevents red→green flicker. */
let consecutiveSuccesses = 0;
const MIN_RECONNECT_SUCCESSES = 2;

/** Set when handleResume fails due to engine being unreachable.
 *  The 5s health poll picks this up and runs the sync once connected. */
let resumePending = false;

/** Tracks consecutive health-poll refreshes for the same empty focused thread.
 *  After 3 attempts (15s of polling), stop — the thread is legitimately empty
 *  or events will arrive via SSE. Resets when focused thread changes. */
let emptyRefreshState: { id: string; count: number } | null = null;
const MAX_EMPTY_REFRESHES = 3;

/** Run all state-sync operations needed after sleep/wake or reconnect.
 *  Called from handleResume (on visibility change) and checkConnection (deferred retry).
 *
 *  Inline forms must NOT be cleared here — filling them requires the user to
 *  alt-tab, take a screenshot, or look something up, and the panel must
 *  survive every focus event. The form's data lives on `panelOverlay` and is
 *  persisted via the nav stack, so reconnect doesn't need to refetch it. */
function runResumeSync(): void {
  // Reset forced-retry tracking so the watchdog can retry threads again.
  // On iOS Safari PWA, the app stays alive for days without a full reload.
  // Without this, forcedRetries accumulates permanently and the watchdog
  // timer in ThreadView can never retry a thread that had a transient failure.
  clearForcedRetries();

  void loadUnreadNotifications();
  // Re-send any compose draft the engine never received. Paired with the same
  // flush in `useStartup`'s onResume, and needed separately from it: this path
  // runs on a reconnect the 5s health poll notices with no wake at all (a
  // desktop client, or a phone that stayed awake through the outage), where no
  // resume event ever fires. Idempotent, and a no-op when nothing is parked.
  flushUndeliveredComposeDrafts();
  disconnectThreadEvents();
  connectThreadEvents();
  refreshChangesState();

  // Incrementally refresh already-loaded threads (append-only event log —
  // existing events stay, we just fetch what's new via ?after=maxSeq).
  // Focused thread awaited first for immediate UX, rest in parallel.
  //
  // The `.catch(() => {})` swallows are safe carve-outs only because
  // `refreshThreadEvents` (thread-loading.ts) toasts user-visible failures
  // itself via `showToast` after retrying — the outer catch keeps the parallel
  // fan-out from rejecting the unhandled-promise tracker; per-thread errors
  // already surfaced inside.
  const focused = focusedThreadId.value;
  const map = threadMap.value;
  const loadedIds: string[] = [];
  const failedIds: string[] = [];
  for (const [id, t] of map.entries()) {
    if (t.eventsLoaded) loadedIds.push(id);
    else if (t.eventsLoadFailed) failedIds.push(id);
  }

  if (focused && map.get(focused)?.eventsLoaded) {
    refreshThreadEvents(focused).then(() => {
      const rest = loadedIds.filter(id => id !== focused);
      if (rest.length > 0) Promise.all(rest.map(refreshThreadEvents)).catch(() => {});
    }).catch(() => {});
  } else if (loadedIds.length > 0) {
    Promise.all(loadedIds.map(refreshThreadEvents)).catch(() => {});
  }

  // Retry threads whose initial load failed — loadThreadEvents resets
  // eventsLoadFailed and does a full load (lastDbSeq is still 0).
  for (const id of failedIds) void loadThreadEvents(id);

  // Also load thread list to pick up any brand-new threads. `loadAllThreads`
  // REJECTS on a failed GET and has no Loadable or toast of its own, so surface
  // it here rather than swallowing. Transient iOS-PWA wake / stale-connection
  // rejections stay silent (the next resume re-syncs), matching
  // `refreshThreadEvents`; a genuine failure toasts.
  loadAllThreads().catch((err) => {
    if (isAbortError(err) || isTransportError(err)) {
      console.warn('[Connection] resume thread-list refresh failed transiently; next resume re-syncs', err);
      return;
    }
    showToast(`Failed to refresh the thread list: ${errorDetail(err)}`, 'error', {
      key: THREAD_LIST_REFRESH_TOAST_KEY,
    });
  });
}

/** Guards against concurrent handleResume calls — iOS Safari PWA fires
 *  visibilitychange, focus, and pageshow simultaneously on wake, causing
 *  three concurrent SSE disconnect/reconnect cycles.
 *
 *  Still load-bearing after `useStartup`'s `onResumeCoalesced` gate, which
 *  collapses that same three-event burst before it reaches here. The two cover
 *  different windows: the gate is a fixed ~1s leading edge, while this flag
 *  lasts exactly as long as the `checkConnection` round-trip, which on a slow
 *  link routinely outlives the gate. Whichever one is wider is the one holding
 *  at that moment, so both stay. */
let resumeInFlight = false;

/** Called on visibility change / focus / pageshow after laptop wake.
 *  Health-checks the engine first — if unreachable, defers to the 5s poll.
 *  Guarded against concurrent calls — only one runs at a time. */
export async function handleResume(): Promise<void> {
  if (resumeInFlight) return;
  resumeInFlight = true;
  try {
    const healthy = await checkConnection();
    if (!healthy) {
      resumePending = true;
      return;
    }
    resumePending = false;
    runResumeSync();
  } finally {
    resumeInFlight = false;
  }
}

/** The restart safety timeout fired (UiBlockingOverlay). Before declaring a
 *  timeout, confirm the engine is actually still unreachable.
 *
 *  On iOS the restart timer is frozen while the PWA is suspended and fires on
 *  resume — even though the engine restarted fine while we were backgrounded.
 *  A blind "Engine restart timed out" toast there is a false error, and worse:
 *  clearing engineRestarting also drops the central toast suppression, letting
 *  the post-restart resync's transient failures leak as a pile of toasts. So
 *  probe health first, but always unblock (the timer's contract). Reachable →
 *  run the normal reconnect path for the real "Engine restarted" toast +
 *  resync. Genuinely unreachable after the full window → tell the user. Both
 *  branches unblock; only the real-timeout branch toasts. */
export async function handleRestartTimeout(): Promise<void> {
  // The safety timeout's contract is to ALWAYS unblock the UI once it fires —
  // clear the overlay flag up front, regardless of the outcome below, so the
  // overlay can never hang on a reachable-but-not-restart-detected edge.
  const health = await checkHealth();
  engineRestarting.value = false;
  // Timeout's contract is to always end the restart — drop the in-flight marker
  // too so a reload after a timed-out restart won't restore the progress toast.
  clearRestartInFlight();
  if (health.status === 'loaded') {
    // Engine is reachable — the restart completed while we were suspended /
    // foregrounded and we just hadn't noticed. Run the normal reconnect path
    // for the real "Engine restarted" toast + resync; no false timeout error.
    await checkConnection();
    return;
  }
  // Genuinely unreachable after the full window — tell the user.
  showToast('Engine restart timed out', 'error');
}

/** Key of the one toast that names a database outage. Keyed so the 5s health
 *  poll updates it in place instead of stacking a copy every tick. */
export const DATABASE_UNREACHABLE_TOAST_KEY = 'database-unreachable';

/** What to tell the user when the engine can't reach its database. Pure, so the
 *  copy is testable and lives in exactly one place.
 *
 *  The remedy differs by deployment and we only claim the one we can support: a
 *  dev workspace's Postgres is a Docker container (ADR 0014 §6/§7), so naming
 *  Docker is a fact there. A packaged install runs a bundled Postgres it manages
 *  itself, where "start Docker" would be actively misleading, so it gets the
 *  condition without a guessed fix. */
export function databaseUnreachableMessage(packaged: boolean): string {
  const cause = "Lucidos can't reach its database, so nothing can load.";
  return packaged ? cause : `${cause} Check that Docker is running.`;
}

/** Mirror `/health`'s `database_reachable` into the signal that drives the whole
 *  degraded surface, and keep the authoritative toast in lockstep with it.
 *
 *  Absent (an older engine) reads as reachable: the client must never invent an
 *  outage from a field the engine never sent.
 *
 *  Ordering is load-bearing. The signal is written BEFORE the toast so that
 *  `workspaceUnavailable()` is already true when the toast is emitted (it opts in
 *  via `showWhileUnavailable`, and everything else emitted this tick is
 *  suppressed behind it), and the toast is retracted BEFORE the signal clears so
 *  the retraction is not itself suppressed. `removeToast`, not `dismissToast`:
 *  this is a signal-driven hide, not the user dismissing anything. */
export function syncDatabaseReachability(health: HealthInfo): void {
  const reachable = health.database_reachable !== false;
  if (!reachable) {
    databaseReachable.value = false;
    showToast(databaseUnreachableMessage(health.packaged === true), 'error', {
      key: DATABASE_UNREACHABLE_TOAST_KEY,
      showWhileUnavailable: true,
      // Not dismissable, and no auto-dismiss: it is the ONLY thing reporting an
      // outage that may last as long as Docker is down, and it is what makes
      // suppressing every other failure toast honest rather than silent. It
      // retracts itself the moment the engine says the database is back.
      dismissable: false,
    });
    return;
  }
  retireDatabaseClaim();
}

/** Drop the outage claim and its toast, returning the client to its normal
 *  surface. Two callers, and the second is the one that is easy to miss.
 *
 *  The claim is evidence FROM the engine, so it cannot outlive our ability to
 *  reach the engine. Once the connection settles to `disconnected` we can no
 *  longer confirm or refute it, and leaving it set would strand an outage toast
 *  under a red dot AND keep `workspaceUnavailable()` true, suppressing the
 *  engine-level toasts that now own the story (an "Engine restart timed out"
 *  among them, which does not opt in). So a settled disconnect retires it. */
function retireDatabaseClaim(): void {
  // Retract BEFORE clearing the signal, so the retraction is not itself
  // suppressed. `removeToast`, not `dismissToast`: this is a signal-driven hide,
  // not the user dismissing anything.
  removeToast(DATABASE_UNREACHABLE_TOAST_KEY);
  databaseReachable.value = true;
}

export async function checkConnection(): Promise<boolean> {
  const wasConnected = connectionStatus.value === 'connected';
  const healthResult = await checkHealth();
  const health = healthResult.status === 'loaded' ? healthResult.data : null;
  let connected = health !== null;

  // Debounce transient health-check failures before declaring disconnect — see
  // MAX_SUPPRESSED_FAILURES above. This runs whenever we were connected,
  // independent of processing state: the old code gated this on isProcessing,
  // so the idle case (PWA in a pocket — exactly when iOS naps the radio) had
  // ZERO tolerance and a single blip flipped the dot red. Only after
  // MAX_SUPPRESSED_FAILURES consecutive failures do we let `connected` stay
  // false and paint red.
  // Track the *real* health result separately from the displayed `connected`
  // value so a suppressed failure doesn't reset the counter on the next line —
  // without this guard the suppression accumulator would zero out after every
  // suppressed failure and the cap would never fire (the engine could stay
  // "down" for hours and we'd still display connected).
  const healthOk = connected;
  if (!connected && wasConnected) {
    consecutiveFailures++;
    if (consecutiveFailures <= MAX_SUPPRESSED_FAILURES) {
      connected = true;
    }
  }

  if (healthOk) {
    consecutiveFailures = 0;
  }

  // The in-flight restart status toast (initiateEngineRestart / restoreRestartToast)
  // shows a single stable message for the whole window — there is no build→swap
  // phase transition to advance here anymore. It stays up with its spinner via
  // showWhileUnavailable until reconnect (started_at change) dismisses it below.

  // When disconnected, require multiple consecutive successes before showing connected.
  // Prevents red→green flicker when the engine flaps during restarts.
  // Skip hysteresis when started_at changed — a new started_at is a definitive signal
  // the engine genuinely restarted, so reconnect immediately.
  const prevStartedAt = engineStartedAt.value;
  const engineJustRestarted = health && prevStartedAt && health.started_at !== prevStartedAt;
  if (connected && !wasConnected && hasEverConnected && !engineJustRestarted) {
    consecutiveSuccesses++;
    if (consecutiveSuccesses < MIN_RECONNECT_SUCCESSES) {
      connected = false;
    }
  }

  // Reset on connect or when the engine is unreachable; keep counting only
  // during hysteresis throttling (health ok but waiting for MIN_RECONNECT_SUCCESSES).
  if (connected || !health) {
    consecutiveSuccesses = 0;
  }

  connectionStatus.value = connected ? 'connected' : 'disconnected';
  // A settled disconnect with NO health to speak for it retires any database
  // claim: see `retireDatabaseClaim`. Both halves matter. The displayed status
  // (not the raw probe result) keeps a blip inside the failure-tolerance window
  // from flapping the toast; and `!health` keeps this off the reconnect path,
  // where a loaded probe can still read `disconnected` during the
  // MIN_RECONNECT_SUCCESSES hysteresis. There the fresh health below is the
  // better evidence, so retiring first would only churn the signal false to true
  // and back inside one tick.
  if (!connected && !health) retireDatabaseClaim();
  if (health) {
    workspaceName.value = health.workspace;
    workspacePath.value = health.workspace_path;
    engineStartedAt.value = health.started_at;
    if (health.release) {
      lucidosRelease.value = health.release;
    }
    lucidosReleaseDirty.value = health.release_dirty === true;
    enginePackaged.value = health.packaged === true;
    // Before the fields below, and before anything else this tick can toast: a
    // dead database is the cause of every other failure in flight, so the
    // suppression window it opens has to be in place first.
    syncDatabaseReachability(health);
    // Only an explicit `false` flips to unconfigured; absent (older engine) or
    // true keeps the configured default so onboarding never shows spuriously.
    llmConfigured.value = health.llm_configured !== false;
    // Providers the engine actually has configured → filters the model picker.
    // `undefined` (older engine) keeps the current value; `null` means "don't
    // filter"; an array narrows to those backends. Reflects a runtime swap.
    if (health.configured_providers !== undefined) {
      configuredProviders.value = health.configured_providers;
    }
    if (health.engine_version) {
      engineVersion.value = health.engine_version;
    }
    if (health.latest_engine_version) {
      latestEngineVersion.value = health.latest_engine_version;
      if (health.engine_version && isNewerVersion(health.latest_engine_version, health.engine_version)) {
        restartRequired.value = true;
      }
    }
    // Tauri app: the shell updates as a versioned unit, so the app-version
    // comparison is the right "client behind" signal. `__LUCIDOS_APP_VERSION__`
    // is injected only by the Tauri shell (lib.rs), so its presence IS the Tauri
    // signal. The WEB client is NOT compared against the engine version here —
    // the frontend bundle and the engine version independently in dev (a
    // Rust-only change bumps the engine but not the bundle), so that comparison
    // flagged a phantom update with nothing to refresh to. Web freshness is the
    // service-worker BUILD_ID check (syncClientUpdateFromBuild), run on load /
    // resume / panel open and after a restart below.
    // `'unknown'` is the engine's sentinel for "couldn't read it", not a version.
    // It is what EVERY packaged install reports: the engine derives this field by
    // reading `crates/lucidos-app/VERSION` under `paths::repo_root()`, and a
    // packaged install has no repo root. Storing the sentinel would both render as
    // a bogus "(latest: unknown)" and clobber the real answer the packaged
    // updater writes here (see store/actions/app-update.ts).
    if (health.latest_tauri_app_version && health.latest_tauri_app_version !== 'unknown') {
      latestTauriAppVersion.value = health.latest_tauri_app_version;
      const appVersion = window.__LUCIDOS_APP_VERSION__;
      if (appVersion && isNewerVersion(health.latest_tauri_app_version, appVersion)) {
        updateAvailable.value = true;
      }
    }
  }

  // Ensure SSE is connected when the engine is available.
  // Skip thread loads during restart — they'd fail and show error toasts.
  // runResumeSync() will handle all loads after the restart completes.
  if (connected && !engineRestarting.value) {
    connectThreadEvents();
    // Retry thread list load if initial load failed — prevents permanent
    // blank screen when startup loadAllThreads hit a transient error.
    if (!threadsLoaded.value) {
      // loadAllThreads stamps Loadable failed via thread-loading's catch path;
      // the swallow is just the rejection-tracker silencer.
      loadAllThreads().catch(() => {});
    }
    // Recovery for focused thread with eventsLoaded=true but 0 events:
    // loadThreadEvents may have completed before the backend committed events.
    // Capped at MAX_EMPTY_REFRESHES to avoid polling indefinitely for
    // legitimately empty threads. Resets when focused thread changes.
    const focusedId = focusedThreadId.value;
    if (focusedId) {
      const ft = threadMap.value.get(focusedId);
      if (ft && ft.eventsLoaded && ft.events.size === 0 && ft.pendingUserMessages.length === 0) {
        // refreshThreadEvents surfaces failures via showToast itself.
        if (!emptyRefreshState || emptyRefreshState.id !== focusedId) {
          emptyRefreshState = { id: focusedId, count: 1 };
          void refreshThreadEvents(focusedId);
        } else if (emptyRefreshState.count < MAX_EMPTY_REFRESHES) {
          emptyRefreshState.count++;
          void refreshThreadEvents(focusedId);
        }
      }
    }
  }

  // Detect engine restart: either we reconnected after a visible disconnect, or the
  // engine's started_at changed (fast restart within the polling interval).
  //
  // `hasEverConnected` gates out a fresh cold start's first connect (not a
  // restart). It resets on a page reload, so a reload mid-restart would normally
  // strand the detection — but `engineRestarting` (restored from the in-flight
  // marker by restoreRestartToast, which also seeds the pre-restart started_at)
  // is an equally valid signal that a connect now is a restart completion. Either
  // unlocks detection; the `started_at` comparison still distinguishes a genuine
  // restart from the dev build phase (engine still up, same started_at).
  const everOrRestarting = hasEverConnected || engineRestarting.value;
  const reconnected = !wasConnected && connected && everOrRestarting;
  const engineRestarted = connected && everOrRestarting && !!health?.started_at && prevStartedAt !== health.started_at;

  if (reconnected || engineRestarted) {
    // Only dismiss the restart toast when the engine actually restarted
    // (started_at changed). On a simple reconnect (network hiccup, brief
    // health timeout), the toast must persist — refreshChangesState() in
    // runResumeSync will confirm the correct state from the API.
    if (engineRestarted) {
      engineRestarting.value = false;
      localStorage.removeItem(RESTART_LS_KEY);
      // The restart finished — drop the in-flight marker so a later reload won't
      // restore a stale progress toast (restoreRestartToast in chat-changes.ts).
      clearRestartInFlight();
      // Restart done — clear the engine "switch pending / available" signals so
      // the reload-glyph badge reflects the fresh engine immediately; the version
      // poll re-affirms the true state right after (an outdated release re-sets
      // restartRequired), so an optimistic clear is safe.
      restartRequired.value = false;
      engineVersionReady.value = false;
      dismissToast(RESTART_TOAST_KEY);
      // Plain, benign confirmation with NO action. The client Refresh prompt is
      // owned SOLELY by the honest client-staleness check
      // (syncClientUpdateFromBuild below). The switched-to engine now serves its
      // OWN pinned client (api/frontend_snapshot.rs), so on a mixed change that
      // served BUILD_ID differs from the still-loaded bundle → the check surfaces
      // the Refresh toast + badge together (badge ⟺ toast). A pure engine-only
      // restart leaves the served client byte-identical, so nothing surfaces (a
      // refresh would be a no-op nag).
      showToast('Engine restarted', 'success', {
        autoDismissMs: TOAST_AUTO_DISMISS_MS,
      });
      // Re-check client staleness now that the new engine serves its pinned
      // client. The build-watch rebuild may land a few seconds later, so the
      // scheduled SW nudges (scheduleServiceWorkerUpdateChecks, fired from the
      // ChangeApplied arm) re-run this check too; this catches a rebuild that
      // already completed.
      void syncClientUpdateFromBuild();
    }
    runResumeSync();
  } else if (connected && resumePending) {
    // Deferred resume from a failed handleResume — engine is back, run sync now
    resumePending = false;
    runResumeSync();
  }

  if (connected) {
    hasEverConnected = true;
    // Reset the cold-start bounce one-shot so a LATER cold session (a real drop
    // then reload) can bounce again. Cheap; runs only while connected.
    try {
      sessionStorage.removeItem(BOUNCE_GUARD_KEY);
    } catch {
      /* storage off */
    }
  }

  return connected;
}
