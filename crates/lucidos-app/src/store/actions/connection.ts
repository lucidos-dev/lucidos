import { connectionStatus, databaseReachable, dismissToast, removeToast, showToast, workspaceName, workspacePath, engineStartedAt, lucidosRelease, lucidosReleaseDirty, engineVersion, latestEngineVersion, latestTauriAppVersion, enginePackaged, llmConfigured, agentSandboxUnavailable, agentSandboxTruncationUnguarded, agentSandboxMetadataUnguarded, configuredProviders, updateAvailable, focusedThreadId, threadMap, engineRestarting, threadsLoaded, threadListFetched, restartRequired, engineVersionReady, TOAST_AUTO_DISMISS_MS, THREAD_EVENTS_FETCH_CONCURRENCY } from '../store';
import { checkHealth } from '../../api/client';
import type { HealthInfo } from '../../api/client';
import type { ConnectionStatus } from '../types';
import { connectThreadEvents, reviveThreadEvents } from './thread-stream';
import { repairGapState } from '../eventGap';
import { engineVersionStatusVersion } from '../engineVersionStatus';
import { loadAllThreads, loadThreadEvents, refreshThreadEvents, retryStaleOpenThreadEvents, clearThreadFetchGuards, markLoadedThreadsStale } from './thread-loading';
import { refreshThreadList } from './thread-list-refresh';
import { runWithConcurrency } from '../../utils/concurrentPool';
import { refreshChangesState, clearRestartInFlight, RESTART_LS_KEY, RESTART_FAILURE_TOAST_KEY } from './chat-changes';
import { loadUnreadNotifications } from './notifications';
import { loadThreadQueue } from './threadQueue';
import { resyncPluginCatalog } from './plugin-marketplaces';
import { loadPreferences } from './preferences';
import { loadReleaseNotices } from './releaseNotices';
import { loadReaderDevices } from './reader-devices';
import { loadUsers } from './users';
import { reloadChangelogIfRead } from './whatsNew';
import { releaseNoticeDismissed } from '../releaseNotices';
import { flushUndeliveredComposeDrafts } from './compose';
import { isNewerBuildId } from '../../utils/version';
import { syncClientUpdateFromBuild } from './client-update';
import { gatewayPickerHref } from '../../utils/basePath';
import { postClientLog } from '../../utils/clientLog';
import { watchPageAway } from '../../utils/pageVisit';

let hasEverConnected = false;

const BOUNCE_GUARD_KEY = 'lucidos-picker-bounce';

export function shouldBounceToPicker(opts: {
  connectedEver: boolean;
  engineAnswered: boolean;
  pickerHref: string | null;
  alreadyBounced: boolean;
}): boolean {
  return !opts.connectedEver && !opts.engineAnswered && opts.pickerHref !== null && !opts.alreadyBounced;
}

export function bounceToPickerIfStranded(): void {
  const pickerHref = gatewayPickerHref();
  let alreadyBounced = false;
  try {
    alreadyBounced = sessionStorage.getItem(BOUNCE_GUARD_KEY) === '1';
  } catch {
  }
  if (!shouldBounceToPicker({
    connectedEver: hasEverConnected,
    engineAnswered: threadListFetched.value,
    pickerHref,
    alreadyBounced,
  })) return;
  try {
    sessionStorage.setItem(BOUNCE_GUARD_KEY, '1');
  } catch {
  }
  window.location.replace(pickerHref as string);
}

let consecutiveFailures = 0;
const MAX_SUPPRESSED_FAILURES = 3;

let consecutiveSuccesses = 0;
const MIN_RECONNECT_SUCCESSES = 2;

export const START_SETTLE_MS = 20_000;

export const START_BOOT_MS = 240_000;

let startAcceptedAt: number | null = null;

export function noteWorkspaceStartAccepted(): void {
  startAcceptedAt = Date.now();
  if (connectionStatus.peek() !== 'starting') recordConnectionTransition('starting', false);
  connectionStatus.value = 'starting';
}

function startStillBooting(probeStopped: boolean): boolean {
  if (startAcceptedAt === null) return false;
  const elapsed = Date.now() - startAcceptedAt;
  if (elapsed <= START_BOOT_MS && !(probeStopped && elapsed > START_SETTLE_MS)) return true;
  startAcceptedAt = null;
  return false;
}

let lastTransitionAt: number | null = null;

/** Leave a breadcrumb in the engine log every time the dot actually changes
 *  colour, and only then.
 *
 *  It exists because the dot was, until this line, the one user-visible signal
 *  in the product with no record on either side. `/api/v1/health` is excluded
 *  from the engine's request log (`api/mod.rs`), correctly: the gateway probes
 *  every workspace every 2s for the picker badge, which would bury everything
 *  else. So a report of "a lot of red blinking" on a phone could be read from
 *  the source but not measured, and the thresholds below could only be tuned by
 *  guessing. A transition costs at least four polls to reach, so this cannot
 *  approach the rate of the gateway probe already in the log.
 *
 *  Three fields, each carrying something that cannot be derived from the other
 *  two. `probe_ok` is the RAW health result: alongside `to` it says whether the
 *  dot moved because the engine answered differently or because a counter
 *  crossed its threshold. `held_ms` is how long the previous colour lasted,
 *  which is the actual question behind "a lot of red blinking" (twenty seconds
 *  of red every other minute is a different fault from five minutes of it).
 *  `visible` separates the case that matters on iOS, flickering while the user
 *  is watching, from a transition noticed on a wake.
 *
 *  The two counters are deliberately NOT carried. Both are reset before this
 *  line runs and both sit at their threshold by definition when the dot flips,
 *  so they could only ever log a constant, and a constant that reads like a
 *  measurement is worse than no field at all.
 *
 *  Permanent observability, not a diagnostic to be removed once this bug is
 *  understood: the dot is a permanent surface, so a record of when it changed
 *  keeps its value. Same standing as the `[Client/lifecycle] startup`
 *  breadcrumb, which `docs/temporary-measures.md` retained on exactly that
 *  reasoning. Best-effort per `.claude/rules/frontend.md`: no user intent, no
 *  toast, nothing to recover. `postClientLog` never throws and never rejects,
 *  and is not awaited, so a slow or failing breadcrumb cannot delay or fail the
 *  probe it describes; if one is lost, the dot is still the user-facing surface
 *  and the next transition logs again. */
function recordConnectionTransition(to: Exclude<ConnectionStatus, 'connecting'>, probeOk: boolean): void {
  const now = Date.now();
  const heldMs = lastTransitionAt === null ? null : now - lastTransitionAt;
  lastTransitionAt = now;
  postClientLog('connection', 'state_changed', {
    from: connectionStatus.peek(),
    to,
    probe_ok: probeOk,
    held_ms: heldMs,
    ever_connected: hasEverConnected,
    visible: typeof document !== 'undefined' ? document.visibilityState === 'visible' : null,
  });
}

let resumePending = false;

let emptyRefreshState: { id: string; count: number } | null = null;
const MAX_EMPTY_REFRESHES = 3;

function runResumeSync(): void {
  clearThreadFetchGuards();
  markLoadedThreadsStale();
  repairGapState();

  void loadUnreadNotifications();
  void loadPreferences();
  void loadReleaseNotices();
  void loadReaderDevices();
  void loadUsers();
  void reloadChangelogIfRead();
  flushUndeliveredComposeDrafts();
  reviveThreadEvents();
  refreshChangesState();
  void loadThreadQueue();
  resyncPluginCatalog();

  const focused = focusedThreadId.value;
  const map = threadMap.value;
  const failedIds: string[] = [];
  for (const [id, t] of map.entries()) {
    if (!t.eventsLoaded && t.eventsLoadFailed) failedIds.push(id);
  }

  if (focused && map.get(focused)?.eventsLoaded) {
    void refreshThreadEvents(focused, { coalesce: true });
  }

  const retryIds = focused && failedIds.includes(focused)
    ? [focused, ...failedIds.filter(id => id !== focused)]
    : failedIds;
  void runWithConcurrency(retryIds, THREAD_EVENTS_FETCH_CONCURRENCY, loadThreadEvents);

  void refreshThreadList();
}

let resumeInFlight = false;

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

export async function handleRestartTimeout(): Promise<void> {
  const health = await checkHealth();
  engineRestarting.value = false;
  clearRestartInFlight();
  engineVersionStatusVersion.value++;
  if (health.status === 'loaded') {
    await checkConnection();
    return;
  }
  showToast('Engine restart timed out', 'error');
}

export const DATABASE_UNREACHABLE_TOAST_KEY = 'database-unreachable';

export function databaseUnreachableMessage(packaged: boolean, poolExhausted: boolean): string {
  if (poolExhausted) {
    return "All of Lucidos's database connections are busy, so nothing can load. If it lasts, restart Lucidos.";
  }
  const cause = "Lucidos can't reach its database, so nothing can load.";
  return packaged ? cause : `${cause} Check that Docker is running.`;
}

export function syncDatabaseReachability(health: HealthInfo): void {
  const reachable = health.database_reachable !== false;
  if (!reachable) {
    databaseReachable.value = false;
    const message = databaseUnreachableMessage(health.packaged === true, health.database_pool_exhausted === true);
    showToast(message, 'error', {
      key: DATABASE_UNREACHABLE_TOAST_KEY,
      showWhileUnavailable: true,
      dismissable: false,
    });
    return;
  }
  retireDatabaseClaim();
}

function retireDatabaseClaim(): void {
  removeToast(DATABASE_UNREACHABLE_TOAST_KEY);
  databaseReachable.value = true;
}

let connectionCheckInFlight: Promise<boolean> | null = null;

export function checkConnection(): Promise<boolean> {
  if (connectionCheckInFlight) return connectionCheckInFlight;
  connectionCheckInFlight = runConnectionCheck().finally(() => {
    connectionCheckInFlight = null;
  });
  return connectionCheckInFlight;
}

let awaySinceLastProbe = watchPageAway();

async function runConnectionCheck(): Promise<boolean> {
  const wasConnected = connectionStatus.value === 'connected';
  const firstSinceReturn = awaySinceLastProbe();
  const awayDuringProbe = watchPageAway();
  awaySinceLastProbe = awayDuringProbe;
  const healthResult = await checkHealth();
  const health = healthResult.status === 'loaded' ? healthResult.data : null;
  let connected = health !== null;

  const healthOk = connected;
  if (!connected && wasConnected) {
    if (!awayDuringProbe()) consecutiveFailures++;
    if (consecutiveFailures <= MAX_SUPPRESSED_FAILURES) {
      connected = true;
    }
  }

  if (healthOk) {
    consecutiveFailures = 0;
  }


  const prevStartedAt = engineStartedAt.value;
  const engineJustRestarted = health && prevStartedAt && health.started_at !== prevStartedAt;
  if (connected && !wasConnected && hasEverConnected && !engineJustRestarted && !firstSinceReturn) {
    consecutiveSuccesses++;
    if (consecutiveSuccesses < MIN_RECONNECT_SUCCESSES) {
      connected = false;
    }
  }

  if (connected || !health) {
    consecutiveSuccesses = 0;
  }

  const probeStopped = healthResult.status === 'stopped';
  if (connected) startAcceptedAt = null;
  const nextStatus = connected ? 'connected'
    : startStillBooting(probeStopped) ? 'starting'
    : probeStopped ? 'stopped' : 'disconnected';
  if (nextStatus !== connectionStatus.value) {
    recordConnectionTransition(nextStatus, healthOk);
  }
  connectionStatus.value = nextStatus;
  if (!connected && !health) retireDatabaseClaim();
  if (health) {
    workspaceName.value = health.workspace;
    workspacePath.value = health.workspace_path;
    engineStartedAt.value = health.started_at;
    if (health.release) {
      lucidosRelease.value = health.release;
    }
    lucidosReleaseDirty.value = health.release_dirty === true;
    enginePackaged.value = health.packaged ?? null;
    syncDatabaseReachability(health);
    llmConfigured.value = health.llm_configured !== false;
    agentSandboxUnavailable.value = health.agent_sandbox_unavailable ?? null;
    agentSandboxTruncationUnguarded.value = health.agent_sandbox_truncation_unguarded === true;
    agentSandboxMetadataUnguarded.value = health.agent_sandbox_metadata_unguarded === true;
    if (health.configured_providers !== undefined) {
      configuredProviders.value = health.configured_providers;
    }
    if (health.engine_version) {
      engineVersion.value = health.engine_version;
    }
    if (health.latest_engine_version) {
      latestEngineVersion.value = health.latest_engine_version;
      if (health.engine_version && isNewerBuildId(health.latest_engine_version, health.engine_version)) {
        restartRequired.value = true;
      }
    }
    if (health.latest_tauri_app_version && health.latest_tauri_app_version !== 'unknown') {
      latestTauriAppVersion.value = health.latest_tauri_app_version;
      const appVersion = window.__LUCIDOS_APP_VERSION__;
      if (appVersion && isNewerBuildId(health.latest_tauri_app_version, appVersion)) {
        updateAvailable.value = true;
      }
    }
  }

  if (connected && healthOk && !engineRestarting.value) {
    connectThreadEvents();
    if (!threadsLoaded.value) {
      loadAllThreads().catch(() => {});
    }
    const focusedId = focusedThreadId.value;
    if (focusedId) {
      const catchingUp = retryStaleOpenThreadEvents(focusedId);
      const ft = threadMap.value.get(focusedId);
      if (!catchingUp && ft && ft.eventsLoaded && ft.events.size === 0 && ft.pendingUserMessages.length === 0) {
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

  const everOrRestarting = hasEverConnected || engineRestarting.value;
  const reconnected = !wasConnected && connected && everOrRestarting;
  const engineRestarted = connected && everOrRestarting && !!health?.started_at && prevStartedAt !== health.started_at;

  if (reconnected || engineRestarted) {
    if (engineRestarted) {
      engineRestarting.value = false;
      localStorage.removeItem(RESTART_LS_KEY);
      clearRestartInFlight();
      restartRequired.value = false;
      engineVersionReady.value = false;
      dismissToast(RESTART_FAILURE_TOAST_KEY);
      showToast('Engine restarted', 'success', {
        autoDismissMs: TOAST_AUTO_DISMISS_MS,
      });
      void syncClientUpdateFromBuild();
      releaseNoticeDismissed.value = false;
    }
    runResumeSync();
  } else if (connected && resumePending) {
    resumePending = false;
    runResumeSync();
  }

  if (connected) {
    hasEverConnected = true;
    try {
      sessionStorage.removeItem(BOUNCE_GUARD_KEY);
    } catch {
    }
  }

  return connected;
}
