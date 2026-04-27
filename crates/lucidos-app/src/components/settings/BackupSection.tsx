import { useState, useEffect, useRef } from 'preact/hooks';
import { backupProgress, showToast } from '../../store/store';
import { grantOAuthScope } from '../../store/actions/oauth';
import { formatDateTime } from '../../utils/formatTime';
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
  type BackupEntry,
  type BackupProviderInfo,
  type BackupKeyResponse,
  type RestoredWorkspace,
  type ValidateNameResult,
} from '../../api/client';
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

function formatSize(bytes: number): string {
  return (bytes / 1024 / 1024).toFixed(1) + ' MB';
}

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
  const [providers, setProviders] = useState<BackupProviderInfo[]>([]);
  const [selectedProvider, setSelectedProvider] = useState<string>('');
  const [backingUp, setBackingUp] = useState(false);
  const [keyInfo, setKeyInfo] = useState<BackupKeyResponse | null>(null);
  const [showKey, setShowKey] = useState(false);
  const [backups, setBackups] = useState<BackupEntry[] | null>(null);
  const [backupsLoading, setBackupsLoading] = useState(false);
  const [backupsError, setBackupsError] = useState<string | null>(null);
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
    getBackupProviders().then((p) => {
      setProviders(p);
      if (p.length > 0) setSelectedProvider(p[0].id);
    }).catch((err) => {
      showToast(`Failed to load backup providers: ${errorDetail(err)}`, 'error');
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

  // Auto-load backups when provider changes and is ready
  const selectedReady = providers.find((p) => p.id === selectedProvider)?.ready ?? false;
  useEffect(() => {
    if (selectedProvider && selectedReady) {
      loadBackups();
    }
  }, [selectedProvider, selectedReady]);

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
        setProviders(p);
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
    setBackingUp(true);
    try {
      const entry = await createBackup(selectedProvider);
      showToast(`Backup created: ${entry.filename} (${formatSize(entry.size_bytes)})`, 'success');
      loadBackups();
    } catch (err) {
      showToast(`Backup failed: ${errorDetail(err)}`, 'error');
    } finally {
      setBackingUp(false);
      backupProgress.value = null;
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
    setBackupsLoading(true);
    setBackupsError(null);
    try {
      const list = await listBackups(selectedProvider);
      setBackups(list);
    } catch (err) {
      setBackupsError(errorDetail(err));
      setBackups(null);
    } finally {
      setBackupsLoading(false);
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
              setBackups(null);
              setSelectedBackupId(null);
            }}
          />
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

        {backupsLoading && (
          <div class="loading-spinner" />
        )}

        {backupsError && (
          <div class="error-text" style="padding: 0.5rem 0;">Failed to load backups: {backupsError}</div>
        )}

        {!backupsLoading && backups !== null && backups.length === 0 && (
          <div class="empty-state">No backups found</div>
        )}

        {backups !== null && backups.length > 0 && (
          <div class="list-rows">
            {backups.map((b) => (
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
                    <span>{formatSize(b.size_bytes)}</span>
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
