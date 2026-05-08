import { connectionStatus, dismissToast, showToast, isProcessing, workspaceName, workspacePath, engineStartedAt, lucidosRelease, lucidosReleaseDirty, engineVersion, latestEngineVersion, latestTauriAppVersion, updateAvailable, focusedThreadId, threadMap, engineRestarting, threadsLoaded, restartRequired } from '../store';
import { checkHealth, API_BASE } from '../../api/client';
import { connectThreadEvents, disconnectThreadEvents } from './thread-sync';
import { loadAllThreads, loadThreadEvents, refreshThreadEvents, clearForcedRetries } from './thread-loading';
import { refreshChangesState, RESTART_LS_KEY } from './chat-changes';
import { refreshUnreadCount } from './notifications';
import { isNewerVersion } from '../../utils/version';

export function getDisconnectedMsg(): string {
  const target = API_BASE || window.location.origin;
  return `Disconnected from engine at ${target}`;
}

/** True once we've been connected at least once (distinguishes initial connect from reconnect). */
let hasEverConnected = false;

/** Consecutive health check failures while processing is active.
 *  After 3 failures (15s at 5s polling) we force disconnect regardless of processing state —
 *  the engine is genuinely down, not just slow. */
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

  refreshUnreadCount();
  disconnectThreadEvents();
  connectThreadEvents();
  refreshChangesState();

  // Incrementally refresh already-loaded threads (append-only event log —
  // existing events stay, we just fetch what's new via ?after=maxSeq).
  // Focused thread awaited first for immediate UX, rest in parallel.
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
  for (const id of failedIds) loadThreadEvents(id);

  // Also load thread list to pick up any brand-new threads
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

export async function checkConnection(): Promise<boolean> {
  const wasConnected = connectionStatus.value === 'connected';
  const health = await checkHealth();
  let connected = health !== null;

  // During active processing, tolerate a few health check timeouts before transitioning to
  // disconnected. The engine can be slow under heavy load, but if it fails
  // MAX_SUPPRESSED_FAILURES times in a row, it's genuinely down.
  if (!connected && isProcessing.value && wasConnected) {
    consecutiveFailures++;
    if (consecutiveFailures <= MAX_SUPPRESSED_FAILURES) {
      connected = true;
    }
  }

  if (connected) {
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
    if (health.latest_tauri_app_version) {
      latestTauriAppVersion.value = health.latest_tauri_app_version;
      const currentAppVersion = window.__LUCIDOS_APP_VERSION__;
      if (currentAppVersion && isNewerVersion(health.latest_tauri_app_version, currentAppVersion)) {
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
        if (!emptyRefreshState || emptyRefreshState.id !== focusedId) {
          emptyRefreshState = { id: focusedId, count: 1 };
          refreshThreadEvents(focusedId);
        } else if (emptyRefreshState.count < MAX_EMPTY_REFRESHES) {
          emptyRefreshState.count++;
          refreshThreadEvents(focusedId);
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
      // Frontend code may have changed — Vite HMR is dead after restart,
      // so the client needs a reload to pick up new assets.
      showToast('Engine restarted', 'success', {
        action: { label: 'Refresh', onClick: () => window.location.reload() },
        autoDismissMs: 4000,
      });
      updateAvailable.value = true;
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
