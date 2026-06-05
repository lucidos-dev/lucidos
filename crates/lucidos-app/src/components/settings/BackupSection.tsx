import { Fragment, type VNode } from 'preact';
import { useState, useEffect, useRef } from 'preact/hooks';
import { backupProgress, restoreState, backupListVersion, backupStatusVersion, showToast } from '../../store/store';
import { grantOAuthScope } from '../../store/actions/oauth';
import { formatDateTime, formatTimeAgo } from '../../utils/formatTime';
import { formatBytes } from '../../utils/formatBytes';
import { Dropdown } from '../shared/Dropdown';
import {
  getBackupProviders,
  getBackupKey,
  generateBackupKey,
  getBackupKeyExists,
  getBackupSchedule,
  setBackupSchedule,
  getBackupRetention,
  setBackupRetention,
  createBackup,
  listBackups,
  getBackupStatus,
  restoreBackup,
  getRestoreStatus,
  clearRestoreStatus,
  validateWorkspaceName,
  startWorkspace,
  ApiError,
  type BackupEntry,
  type BackupProviderInfo,
  type BackupKeyResponse,
  type BackupStatus,
  type BackupLastRun,
  type ValidateNameResult,
} from '../../api/client';
import type { Loadable } from '../../store/types';
import { toFailed } from '../../store/types';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { errorDetail } from '../../utils/errorDetail';

const PHASE_LABELS: Record<string, string> = {
  starting: 'Starting...',
  estimating: 'Estimating...',
  dumping_db: 'Dumping database...',
  compressing: 'Compressing...',
  encrypting: 'Encrypting...',
  uploading: 'Uploading...',
  downloading: 'Downloading...',
  decrypting: 'Decrypting...',
  decompressing: 'Decompressing...',
  initializing: 'Initializing workspace...',
  starting_db: 'Starting database...',
  restoring_db: 'Restoring database...',
};

const SCHEDULE_OPTIONS: { label: string; value: string }[] = [
  { label: 'Manual only', value: 'off' },
  { label: 'Daily (03:00)', value: '0 0 3 * * *' },
  { label: 'Weekly (Sun 03:00)', value: '0 0 3 * * 0' },
  { label: 'Every 12 hours', value: '0 0 */12 * * *' },
];

const RETENTION_OPTIONS: { label: string; value: string }[] = [
  { label: 'Keep 1', value: '1' },
  { label: 'Keep 3', value: '3' },
  { label: 'Keep 5', value: '5' },
  { label: 'Keep 10', value: '10' },
  { label: 'Keep 20', value: '20' },
  { label: 'Keep 50', value: '50' },
];

/** Extract workspace name from backup filename: lucidos-backup-{name}-{YYYYMMDD}-{HHMMSS}.enc */
function extractWorkspaceName(filename: string): string {
  const match = filename.match(/^lucidos-backup-(.+)-\d{8}-\d{6}\.enc$/);
  return match ? match[1] : '';
}

function scheduleLabel(cron: string): string {
  const match = SCHEDULE_OPTIONS.find((o) => o.value === cron);
  return match ? match.label : cron;
}

/** Map backup provider IDs to the OAuth scopes needed for backup. */
const PROVIDER_SCOPES: Record<string, string> = {
  google_drive: 'https://www.googleapis.com/auth/drive.file',
};

/** How often to re-poll /backup/status while a backup is still running. */
const STATUS_POLL_MS = 4000;

type LiveProgress = { phase: string; progress: number; total: number } | null;

/** Shared progress-bar fill used by both the health card and the restore flow.
 *  Null when total is unknown (0) so the bar doesn't render a 0%/NaN width. */
function progressBarFill(progress: { progress: number; total: number }): VNode | null {
  if (progress.total <= 0) return null;
  const pct = Math.round((progress.progress / progress.total) * 100);
  return (
    <div class="progress-bar">
      <div class="progress-bar-fill" style={`width: ${pct}%`} />
    </div>
  );
}

/** Escalating wording for how long it's been since a good cloud backup. */
function staleMessage(ageSeconds: number | null): string {
  if (ageSeconds == null) return 'No cloud backup found — your data is not backed up';
  if (ageSeconds >= 72 * 3600) return 'No successful backup in over 3 days';
  if (ageSeconds >= 48 * 3600) return 'No successful backup in over 48 hours';
  return 'No successful backup in over 24 hours';
}

/** "Last backup: ✓ succeeded 2h ago" / "✗ failed 5m ago — <error>". */
function lastRunLine(lastRun: BackupLastRun | null): VNode | null {
  if (!lastRun) return null;
  const when = formatTimeAgo(new Date(lastRun.at));
  if (lastRun.status === 'success') {
    return (
      <span class="backup-health-line">
        Last backup: <span class="backup-health-success">{'✓'} succeeded {when}</span>
      </span>
    );
  }
  return (
    <span class="backup-health-line">
      Last backup:{' '}
      <span class="backup-health-error">
        {'✗'} failed {when}{lastRun.error ? ` — ${lastRun.error}` : ''}
      </span>
    </span>
  );
}

/** The authoritative "last good cloud backup" line, escalated to a warning
 *  when there's nothing recent. Suppressed when the provider couldn't be
 *  listed — the muted list_error line speaks to that instead. */
function cloudLine(s: BackupStatus): VNode | null {
  if (s.list_error) return null;
  if (!s.latest_backup) {
    return <span class="backup-health-warn">{staleMessage(null)}</span>;
  }
  const when = formatTimeAgo(new Date(s.latest_backup.created_at));
  const size = formatBytes(s.latest_backup.size_bytes);
  if (s.stale) {
    return (
      <span class="backup-health-warn">
        {staleMessage(s.age_seconds)} — last cloud backup {when} ({size})
      </span>
    );
  }
  return <span class="backup-health-ok">Last cloud backup: {when} ({size})</span>;
}

/** The backup health card shown at the top of the Backup section. Pure render
 *  fn (no hooks) so it's unit-testable like `directoryPickerBody`. Answers:
 *  running now? last run outcome? how old is the last good cloud backup? */
export function backupHealthCard(props: {
  status: Loadable<BackupStatus>;
  liveProgress: LiveProgress;
  providerName: string;
}): VNode | null {
  const { status, liveProgress, providerName } = props;

  // Running takes precedence — driven by live SSE progress, or by the persisted
  // `running` flag if the page loaded mid-backup before SSE reconnected.
  const statusRunning = status.status === 'loaded' && status.data.running;
  if (liveProgress || statusRunning) {
    const phase = liveProgress ? (PHASE_LABELS[liveProgress.phase] || liveProgress.phase) : 'Working...';
    return (
      <div class="backup-health-card" data-state="running">
        <span class="backup-health-line">Backup in progress — {phase}</span>
        {liveProgress && progressBarFill(liveProgress)}
      </div>
    );
  }

  if (status.status === 'failed') {
    return (
      <div class="backup-health-card" data-state="failed">
        <span class="backup-health-muted">Couldn't load backup status: {status.error}</span>
      </div>
    );
  }
  // not-loaded / loading: render nothing rather than flashing an empty card.
  if (status.status !== 'loaded') return null;

  const s = status.data;
  return (
    <div class="backup-health-card" data-state={s.stale ? 'stale' : 'idle'}>
      {lastRunLine(s.last_run)}
      {cloudLine(s)}
      {s.list_error && (
        <span class="backup-health-muted">Couldn't reach {providerName} to list backups</span>
      )}
    </div>
  );
}

/** Whether to keep polling `/backup/status`. The engine holds
 *  `backup_in_progress` set through post-backup pruning, so a refetch fired by
 *  the terminal SSE can still read `running:true` and wedge the card on "Backup
 *  in progress" forever. Poll until the flag clears — but skip while live
 *  progress is flowing, since the terminal SSE will refresh us then. */
export function shouldPollBackupStatus(status: Loadable<BackupStatus>, liveProgress: LiveProgress): boolean {
  return status.status === 'loaded' && status.data.running && !liveProgress;
}

/** Label for the single backup-key button. `keyExists` is the read-only probe
 *  result (null = not probed yet); it decides "Show backup key" (a key exists)
 *  vs "Generate new backup key" (none yet). Once a key is revealed the button
 *  toggles to "Hide backup key". Defaulting the unknown state to "Show backup
 *  key" is safe because the click handler falls back to generate if the reveal
 *  404s. This is what stops a "show" action from silently minting a key — the
 *  behavior that surfaced a "New backup key generated" toast for a workspace
 *  that already had backups. */
export function backupKeyButtonLabel(keyExists: boolean | null, showingKey: boolean): string {
  if (showingKey) return 'Hide backup key';
  if (keyExists === false) return 'Generate new backup key';
  return 'Show backup key';
}

export function BackupSection() {
  const [providersLoadable, setProvidersLoadable] = useState<Loadable<BackupProviderInfo[]>>({ status: 'not-loaded' });
  const showProvidersLoading = useDelayedLoading(providersLoadable);
  const [selectedProvider, setSelectedProvider] = useState<string>('');
  const [keyInfo, setKeyInfo] = useState<BackupKeyResponse | null>(null);
  const [showKey, setShowKey] = useState(false);
  // null until the on-mount existence probe answers. Drives the button label
  // (Show vs Generate) without revealing or minting the key.
  const [keyExists, setKeyExists] = useState<boolean | null>(null);
  const [backupsLoadable, setBackupsLoadable] = useState<Loadable<BackupEntry[]>>({ status: 'not-loaded' });
  const showBackupsLoading = useDelayedLoading(backupsLoadable);
  const [statusLoadable, setStatusLoadable] = useState<Loadable<BackupStatus>>({ status: 'not-loaded' });
  const [selectedBackupId, setSelectedBackupId] = useState<string | null>(null);
  const [restoreKey, setRestoreKey] = useState('');
  const [schedule, setSchedule] = useState<string>('off');
  const [scheduleLoaded, setScheduleLoaded] = useState(false);
  const [scheduleSaving, setScheduleSaving] = useState(false);
  const [retention, setRetention] = useState<string>('5');
  const [retentionLoaded, setRetentionLoaded] = useState(false);
  const [retentionSaving, setRetentionSaving] = useState(false);
  const [granting, setGranting] = useState(false);
  const [restoreWorkspaceName, setRestoreWorkspaceName] = useState('');
  const [nameValidation, setNameValidation] = useState<ValidateNameResult | null>(null);
  const [nameValidating, setNameValidating] = useState(false);
  const [startingWorkspace, setStartingWorkspace] = useState(false);
  const nameDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const nameSeqRef = useRef(0);

  useEffect(() => {
    setProvidersLoadable({ status: 'loading' });
    getBackupProviders().then((p) => {
      setProvidersLoadable({ status: 'loaded', data: p });
      if (p.length > 0) setSelectedProvider(p[0].id);
    }).catch((err: unknown) => {
      setProvidersLoadable(toFailed(err));
    });

    getBackupSchedule().then((s) => {
      setSchedule(s.schedule || 'off');
      setScheduleLoaded(true);
    }).catch((err) => {
      setScheduleLoaded(true);
      showToast(`Failed to load backup schedule: ${errorDetail(err)}`, 'error');
    });

    getBackupRetention().then((r) => {
      setRetention(String(r.keep));
      setRetentionLoaded(true);
    }).catch((err) => {
      setRetentionLoaded(true);
      showToast(`Failed to load backup retention: ${errorDetail(err)}`, 'error');
    });

    // Re-attach to any in-flight (or just-finished) restore. This seeds the
    // SAME restoreState shape the Restore* SSE events drive, so a page reloaded
    // mid-restore shows the identical phase/percent — the stream and the refetch
    // never diverge. Best-effort: a failed probe leaves restoreState null (no
    // banner), and the next SSE event re-establishes it.
    getRestoreStatus().then((s) => {
      restoreState.value = s;
    }).catch(() => { /* no banner until SSE or a successful later fetch */ });

    // Probe whether a backup key already exists so the key button labels itself
    // correctly ("Show backup key" vs "Generate new backup key") without
    // revealing the secret or minting one. Best-effort startup probe: a failed
    // probe leaves keyExists null, and the click handler still resolves
    // correctly (reveal, falling back to generate on a 404) and surfaces any
    // real error there — so no toast is owed here.
    getBackupKeyExists()
      .then((r) => setKeyExists(r.exists))
      .catch(() => { /* label falls back to "Show"; the click handler resolves it */ });
  }, []);

  // Cancel any pending workspace-name validation debounce on unmount so a
  // late timer can't call setNameValidation on a detached component.
  useEffect(() => () => {
    if (nameDebounceRef.current) clearTimeout(nameDebounceRef.current);
  }, []);

  const loadedProviders = providersLoadable.status === 'loaded' ? providersLoadable.data : null;

  const providerOptions: { value: string; label: string }[] = (() => {
    if (loadedProviders) return loadedProviders.map((p) => ({ value: p.id, label: p.name }));
    if (providersLoadable.status === 'failed') return [{ value: '', label: 'Failed to load providers' }];
    if (providersLoadable.status === 'loading' && showProvidersLoading) return [{ value: '', label: 'Loading providers...' }];
    return [];
  })();

  const selectedReady = loadedProviders?.find((p) => p.id === selectedProvider)?.ready ?? false;

  // Backup progress (BackupProgress SSE) is now backup-only — restore has its
  // own RestoreProgress stream + restoreState, so there's no cross-labeling.
  const progress = backupProgress.value;

  // Restore UI is driven entirely by restoreState (seeded from getRestoreStatus
  // on load, kept current by Restore* SSE) so a live page and a reloaded page
  // render the identical phase/percent/result.
  const restore = restoreState.value;
  const restoring = restore?.status === 'running';
  const restoreLiveProgress = restore?.status === 'running'
    ? { phase: restore.phase, progress: restore.progress, total: restore.total }
    : null;
  const restoredWorkspace = restore?.status === 'completed'
    ? { workspace_path: restore.workspace_path, workspace_name: restore.workspace_name }
    : null;

  useEffect(() => {
    if (selectedProvider && selectedReady) {
      void loadBackups();
    }
  }, [selectedProvider, selectedReady, backupListVersion.value]);

  // Backup health card — refetch on provider change and after every terminal
  // backup SSE (backupStatusVersion bumps on BackupCompleted AND BackupFailed).
  useEffect(() => {
    if (selectedProvider && selectedReady) {
      void loadStatus();
    } else {
      setStatusLoadable({ status: 'not-loaded' });
    }
  }, [selectedProvider, selectedReady, backupStatusVersion.value]);

  // Poll while the engine still reports a backup running. The terminal SSE's
  // refetch can read running:true during the post-backup pruning window (the
  // engine clears backup_in_progress only after pruning), so without this the
  // card could wedge on "Backup in progress" after the backup already finished.
  useEffect(() => {
    if (!selectedProvider || !selectedReady) return;
    if (!shouldPollBackupStatus(statusLoadable, progress)) return;
    const t = setTimeout(() => { void loadStatus(); }, STATUS_POLL_MS);
    return () => clearTimeout(t);
  }, [statusLoadable, progress, selectedProvider, selectedReady]);

  function selectedProviderInfo(): BackupProviderInfo | undefined {
    return loadedProviders?.find((p) => p.id === selectedProvider);
  }

  async function handleGrantAccess() {
    if (!selectedProvider) return;
    const info = selectedProviderInfo();
    if (!info) return;
    const scopes = PROVIDER_SCOPES[info.id];
    if (!scopes) return;
    setGranting(true);
    const ok = await grantOAuthScope(info.id === 'google_drive' ? 'google' : info.id, scopes);
    if (ok) {
      // Refresh providers to update ready state
      try {
        const p = await getBackupProviders();
        setProvidersLoadable({ status: 'loaded', data: p });
      } catch (err) {
        showToast(`Failed to refresh providers: ${errorDetail(err)}`, 'error');
      }
    }
    setGranting(false);
  }

  async function handleBackup() {
    if (!selectedProvider) return;
    const info = selectedProviderInfo();
    if (info && !info.ready) {
      showToast(`${info.name} is not ready — grant access first`, 'error');
      return;
    }
    backupProgress.value = { phase: 'starting', progress: 0, total: 0 };
    try {
      await createBackup(selectedProvider);
    } catch (err) {
      backupProgress.value = null;
      if (err instanceof ApiError && err.httpCode === 409) {
        showToast('Another backup is already running', 'warning');
      } else {
        showToast(`Backup failed: ${errorDetail(err)}`, 'error');
      }
    }
  }

  async function handleKeyButton() {
    // Already revealed → just toggle visibility, no refetch.
    if (keyInfo) {
      setShowKey(!showKey);
      return;
    }
    // No key yet → generate one (the only user-facing mint path).
    if (keyExists === false) {
      await generateAndShowKey();
      return;
    }
    // A key exists (or existence is still unknown) → reveal it read-only.
    try {
      const resp = await getBackupKey();
      setKeyInfo(resp);
      setShowKey(true);
      setKeyExists(true);
    } catch (err) {
      // 404 = key vanished since the probe (or never existed) → generate it.
      // Any other error is a real failure the user must see.
      if (err instanceof ApiError && err.httpCode === 404) {
        await generateAndShowKey();
      } else {
        showToast(`Failed to show backup key: ${errorDetail(err)}`, 'error');
      }
    }
  }

  async function generateAndShowKey() {
    try {
      const resp = await generateBackupKey();
      setKeyInfo(resp);
      setShowKey(true);
      setKeyExists(true);
      // is_new is false if a key already existed (race) — only warn when we
      // actually minted one, since that's the moment the user must save it.
      if (resp.is_new) {
        showToast(
          'New backup key generated. Store it somewhere safe right now — you need it to restore, and it cannot be recovered.',
          'warning',
        );
      }
    } catch (err) {
      showToast(`Failed to generate backup key: ${errorDetail(err)}`, 'error');
    }
  }

  function copyKey() {
    if (keyInfo) {
      navigator.clipboard.writeText(keyInfo.key).then(() => {
        showToast('Key copied to clipboard', 'success');
      }).catch(() => {
        showToast('Failed to copy key to clipboard', 'error');
      });
    }
  }

  async function loadBackups() {
    if (!selectedProvider) return;
    setBackupsLoadable({ status: 'loading' });
    try {
      const list = await listBackups(selectedProvider);
      setBackupsLoadable({ status: 'loaded', data: list });
    } catch (err) {
      setBackupsLoadable(toFailed(err));
    }
  }

  async function loadStatus() {
    if (!selectedProvider) return;
    setStatusLoadable({ status: 'loading' });
    try {
      const status = await getBackupStatus(selectedProvider);
      setStatusLoadable({ status: 'loaded', data: status });
    } catch (err) {
      setStatusLoadable(toFailed(err));
    }
  }

  function handleNameChange(name: string) {
    setRestoreWorkspaceName(name);
    setNameValidation(null);
    if (nameDebounceRef.current) clearTimeout(nameDebounceRef.current);
    if (!name.trim()) {
      setNameValidating(false);
      return;
    }
    setNameValidating(true);
    const seq = ++nameSeqRef.current;
    nameDebounceRef.current = setTimeout(async () => {
      try {
        const result = await validateWorkspaceName(name.trim());
        if (nameSeqRef.current === seq) setNameValidation(result);
      } catch {
        if (nameSeqRef.current === seq) setNameValidation({ valid: false, reason: 'Validation failed' });
      } finally {
        if (nameSeqRef.current === seq) setNameValidating(false);
      }
    }, 300);
  }

  async function handleRestore() {
    if (!selectedProvider || !selectedBackupId || !restoreKey.trim() || !restoreWorkspaceName.trim()) return;
    if (nameValidation && !nameValidation.valid) return;
    // Optimistically show "starting" so the bar appears before the first SSE.
    // The engine immediately sets its own authoritative Running, and the
    // Restore* SSE (or a reload's getRestoreStatus) keep it in lockstep.
    restoreState.value = {
      status: 'running',
      workspace_name: restoreWorkspaceName.trim(),
      phase: 'starting',
      progress: 0,
      total: 100,
    };
    try {
      // 202 Accepted — the restore runs DETACHED on the engine and survives a
      // tab reload/close. The outcome arrives via RestoreCompleted/RestoreFailed
      // SSE; we must NOT await it here (that was the bug — a dropped request
      // cancelled the restore mid-download).
      await restoreBackup(selectedProvider, selectedBackupId, restoreKey.trim(), restoreWorkspaceName.trim());
    } catch (err) {
      showToast(`Restore failed: ${errorDetail(err)}`, 'error');
      // Reconcile with the engine's authoritative state — e.g. a 409 means a
      // real restore is already running; don't clobber it with a fake "failed".
      try {
        restoreState.value = await getRestoreStatus();
      } catch {
        restoreState.value = { status: 'idle' };
      }
    }
  }

  async function handleOpenWorkspace() {
    if (!restoredWorkspace) return;
    setStartingWorkspace(true);
    try {
      const result = await startWorkspace(restoredWorkspace.workspace_path);
      if (result.ready) {
        window.open(result.url, '_blank');
      } else {
        // Spawned but /health hasn't answered yet (first -b build can take
        // minutes). Don't open a blank tab — tell the user to retry.
        showToast('Workspace is still starting — click Open again in a moment.', 'warning');
      }
    } catch (err) {
      showToast(`Failed to start workspace: ${errorDetail(err)}`, 'error');
    } finally {
      setStartingWorkspace(false);
    }
  }

  async function handleDismissRestore() {
    const prev = restoreState.value;
    restoreState.value = { status: 'idle' };
    try {
      await clearRestoreStatus();
    } catch (err) {
      restoreState.value = prev;
      showToast(`Failed to dismiss: ${errorDetail(err)}`, 'error');
    }
  }

  async function handleScheduleChange(newSchedule: string) {
    if (!selectedProvider) return;
    setScheduleSaving(true);
    try {
      await setBackupSchedule(selectedProvider, newSchedule);
      setSchedule(newSchedule);
      if (newSchedule === 'off') {
        showToast('Automatic backups disabled', 'success');
      } else {
        showToast(`Automatic backups enabled: ${scheduleLabel(newSchedule)}`, 'success');
      }
    } catch (err) {
      showToast(`Failed to set schedule: ${errorDetail(err)}`, 'error');
    } finally {
      setScheduleSaving(false);
    }
  }

  async function handleRetentionChange(value: string) {
    const keep = parseInt(value, 10);
    if (isNaN(keep) || keep < 1) return;
    setRetentionSaving(true);
    try {
      await setBackupRetention(keep);
      setRetention(value);
      showToast(`Keeping latest ${keep} backup${keep === 1 ? '' : 's'}`, 'success');
    } catch (err) {
      showToast(`Failed to set retention: ${errorDetail(err)}`, 'error');
    } finally {
      setRetentionSaving(false);
    }
  }

  const providerInfo = selectedProviderInfo();
  const backingUp = progress !== null;

  return (
    <>
      <div class="settings-section">
        <div class="settings-section-title">Backup</div>

        {backupHealthCard({
          status: statusLoadable,
          liveProgress: progress,
          providerName: providerInfo?.name ?? selectedProvider,
        })}

        <div class="settings-row" data-search-anchor="backup:provider">
          <span class="settings-row-label">Provider</span>
          <Dropdown
            options={providerOptions}
            value={selectedProvider}
            disabled={!loadedProviders}
            onChange={(v) => {
              setSelectedProvider(v);
              setBackupsLoadable({ status: 'not-loaded' });
              setSelectedBackupId(null);
            }}
          />
          {providersLoadable.status === 'failed' && (
            <span class="error-text">Failed to load providers: {providersLoadable.error}</span>
          )}
          {providersLoadable.status === 'loading' && showProvidersLoading && (
            <span class="form-hint">Loading providers...</span>
          )}
        </div>

        {providerInfo && !providerInfo.connected && (
          <div style="font-size: 0.6875rem; color: var(--accent-red); margin-bottom: 0.5rem;">
            Connect your {providerInfo.name} account in Settings → Accounts to enable backups.
          </div>
        )}

        {providerInfo && providerInfo.connected && !providerInfo.ready && (
          <div style="display: flex; align-items: center; gap: 0.5rem; font-size: 0.6875rem; color: var(--accent-red);">
            <span>{providerInfo.name} access not granted.</span>
            <button
              class="action-btn action-btn-confirm"
              disabled={granting}
              onClick={handleGrantAccess}
            >
              {granting ? 'Waiting for browser...' : 'Grant access'}
            </button>
          </div>
        )}

        <div style="display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap;">
          <button
            class="action-btn action-btn-confirm"
            disabled={backingUp || !selectedProvider || !providerInfo?.ready}
            onClick={handleBackup}
          >
            {backingUp ? 'Backing up...' : 'Back up now'}
          </button>
          {scheduleLoaded && (
            <Dropdown
              options={SCHEDULE_OPTIONS}
              value={schedule}
              disabled={scheduleSaving || !selectedProvider}
              onChange={handleScheduleChange}
            />
          )}
          {retentionLoaded && (
            <Dropdown
              options={RETENTION_OPTIONS}
              value={retention}
              disabled={retentionSaving || !selectedProvider}
              onChange={handleRetentionChange}
            />
          )}
        </div>

        {/* Live backup progress is shown by the health card at the top of this
            section (backupHealthCard's running branch) — no duplicate bar here. */}

        <div style="display: flex; align-items: center; gap: 0.75rem; flex-wrap: wrap; margin-top: 0.5rem;">
          <button
            class="action-btn"
            onClick={handleKeyButton}
          >
            {backupKeyButtonLabel(keyExists, showKey)}
          </button>
          {showKey && keyInfo && (
            <>
              <span style="font-size: 0.6875rem; color: var(--text-muted); font-family: var(--font-mono); user-select: all;">
                {keyInfo.key}
              </span>
              <button class="action-btn" onClick={copyKey}>Copy</button>
            </>
          )}
        </div>
        {showKey && keyInfo && (
          <div style="font-size: 0.6875rem; color: var(--accent-red); margin-top: 0.25rem;">
            Store this key somewhere safe right now — you need it to restore, and it cannot be recovered.
          </div>
        )}
      </div>

      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="backup:restore">Restore from backup</div>

        {/* Restore status banner — driven entirely by restoreState (SSE + the
            on-load getRestoreStatus seed), so it renders the same whether the
            page watched the restore live or was reloaded mid-restore. Lives
            outside the per-row form so it survives losing the row selection. */}
        {restore && restore.status === 'running' && (
          <div class="backup-health-card" data-state="running" style="margin-bottom: 0.75rem;">
            <span class="backup-health-line">
              Restoring "{restore.workspace_name}" — {PHASE_LABELS[restore.phase] || restore.phase}
            </span>
            {restoreLiveProgress && progressBarFill(restoreLiveProgress)}
          </div>
        )}
        {restore && restore.status === 'completed' && (
          <div class="backup-health-card" data-state="running" style="margin-bottom: 0.75rem; display: flex; align-items: center; gap: 0.75rem; flex-wrap: wrap;">
            <span class="backup-health-line">
              Restored "{restore.workspace_name}" to {restore.workspace_path}
            </span>
            <button
              class="action-btn action-btn-confirm"
              disabled={startingWorkspace}
              onClick={handleOpenWorkspace}
            >
              {startingWorkspace ? (<><span class="mini-spinner" />{' Starting...'}</>) : 'Open workspace'}
            </button>
            <button class="action-btn" onClick={handleDismissRestore}>Dismiss</button>
          </div>
        )}
        {restore && restore.status === 'failed' && (
          <div class="backup-health-card" data-state="failed" style="margin-bottom: 0.75rem; display: flex; align-items: center; gap: 0.75rem; flex-wrap: wrap;">
            <span class="backup-health-line">Restore failed: {restore.error}</span>
            <button class="action-btn" onClick={handleDismissRestore}>Dismiss</button>
          </div>
        )}

        {backupsLoadable.status === 'loading' && showBackupsLoading && (
          <div class="loading-spinner" />
        )}

        {backupsLoadable.status === 'failed' && (
          <div class="error-text" style="padding: 0.5rem 0;">Failed to load backups: {backupsLoadable.error}</div>
        )}

        {backupsLoadable.status === 'loaded' && backupsLoadable.data.length === 0 && (
          <div class="empty-state">No backups found</div>
        )}

        {backupsLoadable.status === 'loaded' && backupsLoadable.data.length > 0 && (
          <div class="list-rows">
            {backupsLoadable.data.map((b) => {
              const isSelected = selectedBackupId === b.id;
              return (
                <Fragment key={b.id}>
                  <div
                    class={`list-row ${isSelected ? 'backup-selected' : ''}`}
                    onClick={() => {
                      if (isSelected) {
                        setSelectedBackupId(null);
                        setRestoreWorkspaceName('');
                        setNameValidation(null);
                      } else {
                        setSelectedBackupId(b.id);
                        const name = extractWorkspaceName(b.filename);
                        setRestoreWorkspaceName(name);
                        setNameValidation(null);
                        if (name) handleNameChange(name);
                      }
                    }}
                  >
                    <div class="list-row-info">
                      <div class="title">{b.filename}</div>
                      <div class="list-row-details">
                        <span>{formatDateTime(new Date(b.created_at))}</span>
                        {' \u00b7 '}
                        <span>{formatBytes(b.size_bytes)}</span>
                      </div>
                    </div>
                  </div>

                  {isSelected && (
                    <div style="padding: 0.5rem 0.75rem 0.75rem; display: flex; flex-direction: column; gap: 0.5rem;">
                      <div style="display: flex; align-items: center; gap: 0.5rem;">
                        <input
                          type="text"
                          class="backup-key-input"
                          placeholder="Workspace name"
                          value={restoreWorkspaceName}
                          onInput={(e) => handleNameChange((e.target as HTMLInputElement).value)}
                        />
                        {nameValidating && <span class="mini-spinner" />}
                        {nameValidation && !nameValidation.valid && (
                          <span style="font-size: 0.6875rem; color: var(--accent-red);">
                            {nameValidation.reason}
                          </span>
                        )}
                        {nameValidation?.valid && (
                          <span style="font-size: 0.6875rem; color: var(--accent-green);">Available</span>
                        )}
                      </div>
                      <div style="display: flex; align-items: center; gap: 0.75rem;">
                        <input
                          type="password"
                          class="backup-key-input"
                          placeholder="Enter backup key"
                          value={restoreKey}
                          onInput={(e) => setRestoreKey((e.target as HTMLInputElement).value)}
                        />
                        <button
                          class="action-btn action-btn-confirm"
                          disabled={restoring || !restoreKey.trim() || !restoreWorkspaceName.trim() || (nameValidation !== null && !nameValidation.valid)}
                          onClick={handleRestore}
                        >
                          {restoring ? 'Restoring...' : 'Restore'}
                        </button>
                      </div>
                      {/* Live progress + the completed/failed result render in the
                          restore-status banner above, so they survive a reload and
                          don't depend on this row staying selected. */}
                    </div>
                  )}
                </Fragment>
              );
            })}
          </div>
        )}
      </div>
    </>
  );
}
