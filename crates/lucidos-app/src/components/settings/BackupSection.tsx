import { type VNode } from 'preact';
import { useState, useEffect } from 'preact/hooks';
import { backupProgress, backupStatusVersion, showToast } from '../../store/store';
import { grantOAuthScope } from '../../store/actions/oauth';
import { PROVIDER_SCOPES, oauthProviderFor } from './backupProviderScopes';
import { openConnectedAccountsSettings } from '../../store/actions/menu';
import { handleNavigationRequest } from '../../store/actions/navigation-request';
import { formatTimeAgo } from '../../utils/formatTime';
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
  getBackupStatus,
  ApiError,
  type BackupProviderInfo,
  type BackupKeyResponse,
  type BackupStatus,
  type BackupLastRun,
} from '../../api/client';
import type { Loadable } from '../../store/types';
import { toFailed } from '../../store/types';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { errorDetail } from '../../utils/errorDetail';

// Restore lives in the workspace picker now (it provisions the new workspace);
// this section is backup creation + key + scheduling only. These are the phases
// the backup pipeline reports via BackupProgress.
const PHASE_LABELS: Record<string, string> = {
  starting: 'Starting...',
  estimating: 'Estimating...',
  dumping_db: 'Dumping database...',
  compressing: 'Compressing...',
  encrypting: 'Encrypting...',
  uploading: 'Uploading...',
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

function scheduleLabel(cron: string): string {
  const match = SCHEDULE_OPTIONS.find((o) => o.value === cron);
  return match ? match.label : cron;
}

/** How often to re-poll /backup/status while a backup is still running. */
const STATUS_POLL_MS = 4000;

/** Hand backup setup to the agent, with a prompt that names the parts the page
 *  can't do for the user (connecting the provider account, saving the key).
 *  Routed through `handleNavigationRequest` rather than poking compose directly
 *  so it clears the settings overlay, allocates a fresh draft and focuses the
 *  prompt exactly like every other new-chat entry point. */
function askLucidosToSetUpBackups(): void {
  handleNavigationRequest({
    target: 'new-chat',
    prompt:
      'Help me set up encrypted backups: pick a provider, connect the account, '
      + 'choose a schedule, and make sure I save the encryption key.',
  });
}

type LiveProgress = { phase: string; progress: number; total: number } | null;

/** Progress-bar fill for the health card's running state. Null when total is
 *  unknown (0) so the bar doesn't render a 0%/NaN width. */
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
  // Severity drives the card hue. A failed last run escalates the whole card to
  // the error (red) state so a failure never reads as a cheerful yellow box;
  // a merely-stale-but-not-failed state stays a soft yellow warning.
  const lastRunFailed = s.last_run != null && s.last_run.status !== 'success';
  const state = lastRunFailed ? 'error' : s.stale ? 'stale' : 'idle';
  return (
    <div class="backup-health-card" data-state={state}>
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
  const [statusLoadable, setStatusLoadable] = useState<Loadable<BackupStatus>>({ status: 'not-loaded' });
  const [schedule, setSchedule] = useState<string>('off');
  const [scheduleLoaded, setScheduleLoaded] = useState(false);
  const [scheduleSaving, setScheduleSaving] = useState(false);
  const [retention, setRetention] = useState<string>('5');
  const [retentionLoaded, setRetentionLoaded] = useState(false);
  const [retentionSaving, setRetentionSaving] = useState(false);
  const [granting, setGranting] = useState(false);

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

  const loadedProviders = providersLoadable.status === 'loaded' ? providersLoadable.data : null;

  const providerOptions: { value: string; label: string }[] = (() => {
    if (loadedProviders) return loadedProviders.map((p) => ({ value: p.id, label: p.name }));
    if (providersLoadable.status === 'failed') return [{ value: '', label: 'Failed to load providers' }];
    if (providersLoadable.status === 'loading' && showProvidersLoading) return [{ value: '', label: 'Loading providers...' }];
    return [];
  })();

  const selectedReady = loadedProviders?.find((p) => p.id === selectedProvider)?.ready ?? false;

  const progress = backupProgress.value;

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
    if (!scopes) {
      // A provider with no scope entry cannot be granted anything, and a button
      // that quietly does nothing is worse than one that says why.
      showToast(`No backup permissions are defined for ${info.name}`, 'error');
      return;
    }
    setGranting(true);
    const ok = await grantOAuthScope(oauthProviderFor(info.id), scopes);
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
    <div class="settings-section">
      {/* Backup setup is spread over two pages (the schedule here, the provider
          account in Settings → Accounts) and one irreversible obligation (save
          the encryption key). That is exactly the shape the agent is good at
          walking someone through, and it can do the connecting itself, so offer
          the hand-off instead of leaving the page as the only route. */}
      <p class="settings-section-desc">
        Encrypted backups of this workspace, uploaded to your own cloud storage. Restore
        happens from the workspace picker, not from here.{' '}
        <button class="accent-link" onClick={askLucidosToSetUpBackups}>
          Ask Lucidos to set this up
        </button>{' '}
        if you would rather be walked through it.
      </p>

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
          onChange={(v) => setSelectedProvider(v)}
        />
        {providersLoadable.status === 'failed' && (
          <span class="error-text">Failed to load providers: {providersLoadable.error}</span>
        )}
        {providersLoadable.status === 'loading' && showProvidersLoading && (
          <span class="form-hint">Loading providers...</span>
        )}
      </div>

      {providerInfo && !providerInfo.connected && (
        // Still an error state (nothing uploads until an account is connected),
        // but now with a way out of it. This was static prose naming a path,
        // which left the user to walk it themselves; the button lands them on
        // the Connected accounts section directly.
        <div class="backup-not-connected-row">
          <span>No {providerInfo.name} account connected, so backups cannot upload.</span>
          <button class="action-btn action-btn-confirm" onClick={openConnectedAccountsSettings}>
            Connect {providerInfo.name}
          </button>
        </div>
      )}

      {providerInfo && providerInfo.connected && !providerInfo.ready && (
        <div style="display: flex; align-items: center; gap: 0.5rem; font-size: var(--font-size-xs); color: var(--accent-red);">
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

      {providerInfo?.ready && providerInfo.folder_url && (
        <div class="backup-folder-link-row">
          <a
            class="accent-link"
            href={providerInfo.folder_url}
            target="_blank"
            rel="noopener noreferrer"
          >
            View backups folder in {providerInfo.name} ↗
          </a>
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
      {scheduleLoaded && schedule !== 'off' && (
        <div style="font-size: var(--font-size-xs); color: var(--text-muted); margin-top: 0.25rem;">
          Scheduled backups run at the selected time in your timezone (set in Settings → Locale).
        </div>
      )}

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
            <span style="font-size: var(--font-size-xs); color: var(--text-muted); font-family: var(--font-mono); user-select: all;">
              {keyInfo.key}
            </span>
            <button class="action-btn" onClick={copyKey}>Copy</button>
          </>
        )}
      </div>
      {showKey && keyInfo && (
        <div style="font-size: var(--font-size-xs); color: var(--accent-red); margin-top: 0.25rem;">
          Store this key somewhere safe right now — you need it to restore, and it cannot be recovered.
          You restore a backup from the workspace picker.
        </div>
      )}
    </div>
  );
}
