import { useState, useEffect, useRef } from 'preact/hooks';
import { backupProgress, backupListVersion, showToast } from '../../store/store';
import { grantOAuthScope } from '../../store/actions/oauth';
import { formatDateTime } from '../../utils/formatTime';
import { formatBytes } from '../../utils/formatBytes';
import { Dropdown } from '../shared/Dropdown';
import {
  getBackupProviders,
  getBackupKey,
  getBackupSchedule,
  setBackupSchedule,
  getBackupRetention,
  setBackupRetention,
  createBackup,
  listBackups,
  restoreBackup,
  validateWorkspaceName,
  startWorkspace,
  ApiError,
  type BackupEntry,
  type BackupProviderInfo,
  type BackupKeyResponse,
  type RestoredWorkspace,
  type ValidateNameResult,
} from '../../api/client';
import type { Loadable } from '../../store/types';
import { toFailed } from '../../store/types';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { errorDetail } from '../../utils/errorDetail';

const PHASE_LABELS: Record<string, string> = {
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

function formatDate(iso: string): string {
  return formatDateTime(new Date(iso));
}

function scheduleLabel(cron: string): string {
  const match = SCHEDULE_OPTIONS.find((o) => o.value === cron);
  return match ? match.label : cron;
}

/** Map backup provider IDs to the OAuth scopes needed for backup. */
const PROVIDER_SCOPES: Record<string, string> = {
  google_drive: 'https://www.googleapis.com/auth/drive.file',
};

export function BackupSection() {
  const [providersLoadable, setProvidersLoadable] = useState<Loadable<BackupProviderInfo[]>>({ status: 'not-loaded' });
  const showProvidersLoading = useDelayedLoading(providersLoadable);
  const [selectedProvider, setSelectedProvider] = useState<string>('');
  const [submittingBackup, setSubmittingBackup] = useState(false);
  const [keyInfo, setKeyInfo] = useState<BackupKeyResponse | null>(null);
  const [showKey, setShowKey] = useState(false);
  const [backupsLoadable, setBackupsLoadable] = useState<Loadable<BackupEntry[]>>({ status: 'not-loaded' });
  const showBackupsLoading = useDelayedLoading(backupsLoadable);
  const [selectedBackupId, setSelectedBackupId] = useState<string | null>(null);
  const [restoreKey, setRestoreKey] = useState('');
  const [restoring, setRestoring] = useState(false);
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
  const [restoredWorkspace, setRestoredWorkspace] = useState<RestoredWorkspace | null>(null);
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
  }, []);

  const providers = providersLoadable.status === 'loaded' ? providersLoadable.data : [];

  const selectedReady = providers.find((p) => p.id === selectedProvider)?.ready ?? false;
  useEffect(() => {
    if (selectedProvider && selectedReady) {
      loadBackups();
    }
  }, [selectedProvider, selectedReady, backupListVersion.value]);

  function selectedProviderInfo(): BackupProviderInfo | undefined {
    return providers.find((p) => p.id === selectedProvider);
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
    setSubmittingBackup(true);
    try {
      await createBackup(selectedProvider);
    } catch (err) {
      backupProgress.value = null;
      if (err instanceof ApiError && err.httpCode === 409) {
        showToast('Another backup is already running', 'warning');
      } else {
        showToast(`Backup failed: ${errorDetail(err)}`, 'error');
      }
    } finally {
      setSubmittingBackup(false);
    }
  }

  async function handleShowKey() {
    if (keyInfo) {
      setShowKey(!showKey);
      return;
    }
    try {
      const resp = await getBackupKey();
      setKeyInfo(resp);
      setShowKey(true);
      if (resp.is_new) {
        showToast('New backup key generated. Save it -- you need it to restore. It cannot be recovered.', 'warning');
      }
    } catch (err) {
      showToast(`Failed to get backup key: ${errorDetail(err)}`, 'error');
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
    setRestoring(true);
    try {
      const result = await restoreBackup(
        selectedProvider,
        selectedBackupId,
        restoreKey.trim(),
        restoreWorkspaceName.trim(),
      );
      setRestoredWorkspace(result);
      showToast(`Workspace restored: ${result.workspace_name}`, 'success');
    } catch (err) {
      showToast(`Restore failed: ${errorDetail(err)}`, 'error');
    } finally {
      setRestoring(false);
      backupProgress.value = null;
    }
  }

  async function handleOpenWorkspace() {
    if (!restoredWorkspace) return;
    setStartingWorkspace(true);
    try {
      const result = await startWorkspace(restoredWorkspace.workspace_path);
      window.open(result.url, '_blank');
    } catch (err) {
      showToast(`Failed to start workspace: ${errorDetail(err)}`, 'error');
    } finally {
      setStartingWorkspace(false);
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
  const progress = backupProgress.value;
  const progressLabel = progress ? (PHASE_LABELS[progress.phase] || progress.phase) : null;
  // `submittingBackup` covers the POST-to-first-progress gap; SSE owns the rest.
  const backingUp = submittingBackup || progress !== null;

  return (
    <>
      <div class="settings-section">
        <div class="settings-section-title">Backup</div>

        <div class="settings-row" data-search-anchor="backup:provider">
          <span class="settings-row-label">Provider</span>
          <Dropdown
            options={providers.map((p) => ({ value: p.id, label: p.name }))}
            value={selectedProvider}
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

        {backingUp && progress && (
          <div style="margin-top: 0.5rem;">
            {progress.total > 0 && (
              <div class="progress-bar">
                <div
                  class="progress-bar-fill"
                  style={`width: ${Math.round((progress.progress / progress.total) * 100)}%`}
                />
              </div>
            )}
            <span class="progress-label">{progressLabel}</span>
          </div>
        )}

        <div style="display: flex; align-items: center; gap: 0.75rem; flex-wrap: wrap; margin-top: 0.5rem;">
          <button
            class="action-btn"
            onClick={handleShowKey}
          >
            {showKey ? 'Hide backup key' : 'Show backup key'}
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
        {showKey && keyInfo?.is_new && (
          <div style="font-size: 0.6875rem; color: var(--accent-red); margin-top: 0.25rem;">
            Save this key — you need it to restore. It cannot be recovered.
          </div>
        )}
      </div>

      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="backup:restore">Restore from backup</div>

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
            {backupsLoadable.data.map((b) => (
              <div
                class={`list-row ${selectedBackupId === b.id ? 'backup-selected' : ''}`}
                key={b.id}
                onClick={() => {
                  if (selectedBackupId === b.id) {
                    setSelectedBackupId(null);
                    setRestoreWorkspaceName('');
                    setNameValidation(null);
                    setRestoredWorkspace(null);
                  } else {
                    setSelectedBackupId(b.id);
                    const name = extractWorkspaceName(b.filename);
                    setRestoreWorkspaceName(name);
                    setNameValidation(null);
                    setRestoredWorkspace(null);
                    if (name) handleNameChange(name);
                  }
                }}
              >
                <div class="list-row-info">
                  <div class="title">{b.filename}</div>
                  <div class="list-row-details">
                    <span>{formatDate(b.created_at)}</span>
                    {' \u00b7 '}
                    <span>{formatBytes(b.size_bytes)}</span>
                  </div>
                </div>
              </div>
            ))}
          </div>
        )}

        {selectedBackupId && !restoredWorkspace && (
          <div style="margin-top: 0.5rem; display: flex; flex-direction: column; gap: 0.5rem;">
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
          </div>
        )}

        {restoredWorkspace && (
          <div style="margin-top: 0.5rem; display: flex; align-items: center; gap: 0.75rem;">
            <span style="font-size: 0.6875rem; color: var(--text-muted);">
              Restored to {restoredWorkspace.workspace_path}
            </span>
            <button
              class="action-btn action-btn-confirm"
              disabled={startingWorkspace}
              onClick={handleOpenWorkspace}
            >
              {startingWorkspace ? (
                <>
                  <span class="mini-spinner" />
                  {' Starting...'}
                </>
              ) : (
                'Open workspace'
              )}
            </button>
          </div>
        )}

        {restoring && progress && (
          <div style="margin-top: 0.5rem;">
            {progress.total > 0 && (
              <div class="progress-bar">
                <div
                  class="progress-bar-fill"
                  style={`width: ${Math.round((progress.progress / progress.total) * 100)}%`}
                />
              </div>
            )}
            <span class="progress-label">{progressLabel}</span>
          </div>
        )}
      </div>
    </>
  );
}
