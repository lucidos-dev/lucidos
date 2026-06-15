import { connectionStatus, dismissToast, showToast, workspaceName, workspacePath, engineStartedAt, lucidosRelease, lucidosReleaseDirty, engineVersion, latestEngineVersion, latestTauriAppVersion, updateAvailable, focusedThreadId, threadMap, engineRestarting, threadsLoaded, restartRequired, TOAST_AUTO_DISMISS_MS } from '../store';
import { checkHealth, API_BASE } from '../../api/client';
import { connectThreadEvents, disconnectThreadEvents } from './thread-sync';
import { loadAllThreads, loadThreadEvents, refreshThreadEvents, clearForcedRetries } from './thread-loading';
import { refreshChangesState, RESTART_LS_KEY } from './chat-changes';
import { loadUnreadNotifications } from './notifications';
import { isNewerVersion } from '../../utils/version';
import { refreshClient } from '../../hooks/sw-update';
import { syncClientUpdateFromBuild } from './client-update';

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

  // Also load thread list to pick up any brand-new threads. loadAllThreads
  // stamps Loadable failed states internally; the catch is the rejection-
  // tracker silencer.
  loadAllThreads().catch(() => {});
}

/** Guards against concurrent handleResume calls — iOS Safari PWA fires
 *  visibilitychange, focus, and pageshow simultaneously on wake, causing
 *  three concurrent SSE disconnect/reconnect cycles. */
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
  if (health) {
    workspaceName.value = health.workspace;
    workspacePath.value = health.workspace_path;
    engineStartedAt.value = health.started_at;
    if (health.release) {
      lucidosRelease.value = health.release;
    }
    lucidosReleaseDirty.value = health.release_dirty === true;
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
    if (health.latest_tauri_app_version) {
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
  const reconnected = !wasConnected && connected && hasEverConnected;
  const engineRestarted = connected && hasEverConnected && !!health?.started_at && prevStartedAt !== health.started_at;

  if (reconnected || engineRestarted) {
    // Only dismiss the restart toast when the engine actually restarted
    // (started_at changed). On a simple reconnect (network hiccup, brief
    // health timeout), the toast must persist — refreshChangesState() in
    // runResumeSync will confirm the correct state from the API.
    if (engineRestarted) {
      engineRestarting.value = false;
      localStorage.removeItem(RESTART_LS_KEY);
      dismissToast('restart-required');
      // Frontend code MAY have changed — Vite HMR is dead after restart, so the
      // client needs a reload to pick up new assets. refreshClient() (not a bare
      // reload) so the new sw.js is picked up and the cache-first /assets/* graph
      // doesn't serve a stale bundle back after the reload.
      showToast('Engine restarted', 'success', {
        action: { label: 'Refresh', onClick: () => refreshClient() },
        autoDismissMs: TOAST_AUTO_DISMISS_MS,
      });
      // Light the update badge only if the frontend bundle ACTUALLY rebuilt
      // (BUILD_ID changed), not for every restart — an engine-only (Rust) change
      // bumps the engine but leaves the bundle untouched, so a blind set here
      // nagged for a refresh that does nothing. The build-watch rebuild lands a
      // few seconds after the restart, so re-check on the scheduled SW nudges
      // too; this catches a rebuild that already completed.
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
  }

  return connected;
}
