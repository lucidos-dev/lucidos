import { PREF_BACKUP_RETENTION, PREF_BACKUP_SCHEDULE } from '@lucidos/preference-catalog';
import { type ComponentChildren, type VNode } from 'preact';
import { useState, useEffect, useRef } from 'preact/hooks';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { useVersionedRefresh } from '../../hooks/useVersionedRefresh';
import {
  backupPreferencesVersion,
  backupProgress,
  backupRecipientVersion,
  backupStatusVersion,
  knownOAuthProviders,
  showToast,
} from '../../store/store';
import { startBackup } from '../../store/actions/backup';
import { grantOAuthScope, loadKnownOAuthProviders } from '../../store/actions/oauth';
import {
  ProviderPermissionsHint,
  reauthorizationHint,
} from '../credentials/providerConsoleHint';
import {
  PROVIDER_ACCESS,
  PROVIDER_SCOPES,
  backupAccessLine,
  oauthProviderFor,
} from './backupProviderScopes';
import { backupSeed, refreshMayApply } from './backupSeeding';

import { openConnectedAccountsSettings } from '../../store/actions/menu';
import { handleNavigationRequest } from '../../store/actions/navigation-request';
import { formatTimeAgo } from '../../utils/formatTime';
import { formatBytes } from '../../utils/formatBytes';
import { Dropdown, DropdownSkeleton } from '../shared/Dropdown';
import { ProgressBar } from '../shared/progressBar';
import { Explainer } from '../shared/Explainer';
import { LoadingFade } from '../shared/LoadingFade';
import { SkeletonProvider, SkText } from '../shared/Skeleton';
import {
  getBackupProviders,
  getBackupKeys,
  getBackupSchedule,
  setBackupSchedule,
  getBackupRetention,
  setBackupRetention,
  getBackupStatus,
  type BackupProviderInfo,
  type BackupKeys,
  type BackupStatus,
  type BackupLastRun,
} from '../../api/client';
import type { Loadable } from '../../store/types';
import { toFailed } from '../../store/types';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { errorDetail } from '../../utils/errorDetail';
import { BackupKeyPanel, backupKeyPanelSkeleton, backupsCanSeal } from './BackupKeyPanel';

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
  { label: '1 backup', value: '1' },
  { label: '3 backups', value: '3' },
  { label: '5 backups', value: '5' },
  { label: '10 backups', value: '10' },
  { label: '20 backups', value: '20' },
  { label: '50 backups', value: '50' },
];

function scheduleLabel(cron: string): string {
  const match = SCHEDULE_OPTIONS.find((o) => o.value === cron);
  return match ? match.label : cron;
}

const STATUS_POLL_MS = 4000;

function askLucidosToSetUpBackups(): void {
  void handleNavigationRequest({
    target: 'new-chat',
    prompt:
      'Help me set up encrypted backups: pick a provider, connect the account, '
      + 'choose a schedule, and make sure I save the backup key.',
  });
}

type LiveProgress = { phase: string; progress: number; total: number } | null;

function staleMessage(ageSeconds: number | null): string {
  if (ageSeconds == null) return 'No cloud backup found — your data is not backed up';
  if (ageSeconds >= 72 * 3600) return 'No successful backup in over 3 days';
  if (ageSeconds >= 48 * 3600) return 'No successful backup in over 48 hours';
  return 'No successful backup in over 24 hours';
}

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

export function backupHealthCard(props: {
  status: Loadable<BackupStatus>;
  liveProgress: LiveProgress;
  providerName: string;
}): VNode | null {
  const { status, liveProgress, providerName } = props;

  const statusRunning = status.status === 'loaded' && status.data.running;
  if (liveProgress || statusRunning) {
    const phase = liveProgress ? (PHASE_LABELS[liveProgress.phase] || liveProgress.phase) : 'Working...';
    return (
      <div class="backup-health-card" data-state="running">
        <span class="backup-health-line">Backup in progress — {phase}</span>
        {liveProgress && <ProgressBar done={liveProgress.progress} total={liveProgress.total} />}
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
  if (status.status !== 'loaded') return null;

  const s = status.data;
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

export function showBackupSetupOffer(
  providers: Loadable<BackupProviderInfo[]>,
  providerReady: boolean,
  status: Loadable<BackupStatus>,
): boolean {
  if (providers.status !== 'loaded') return false;
  if (!providerReady) return true;
  if (status.status !== 'loaded') return false;
  const s = status.data;
  if (s.list_error) return false;
  if (!s.latest_backup || s.stale) return true;
  return s.last_run != null && s.last_run.status !== 'success';
}

export function backupHealthCardSkeleton(): VNode {
  return (
    <SkeletonProvider>
      <div class="backup-health-card" data-state="idle" aria-hidden="true">
        <SkText class="backup-health-line" w="12rem" />
        <SkText class="backup-health-line" w="15rem" />
      </div>
    </SkeletonProvider>
  );
}

function DropdownSlot({
  pending,
  showSkeleton,
  skeletonWidth,
  children,
}: {
  pending: boolean;
  showSkeleton: boolean;
  skeletonWidth: string;
  children: ComponentChildren;
}) {
  return (
    <LoadingFade class="dropdown-slot" showSkeleton={showSkeleton} skeleton={<DropdownSkeleton w={skeletonWidth} />}>
      {pending ? null : children}
    </LoadingFade>
  );
}

export function backupSlotsPending(
  providers: Loadable<BackupProviderInfo[]>,
  selectedReady: boolean,
  status: Loadable<BackupStatus>,
): { controls: boolean; status: boolean } {
  const controls = providers.status === 'not-loaded' || providers.status === 'loading';
  return {
    controls,
    status:
      controls
      || (selectedReady && status.status !== 'loaded' && status.status !== 'failed'),
  };
}

export function shouldPollBackupStatus(status: Loadable<BackupStatus>, liveProgress: LiveProgress): boolean {
  return status.status === 'loaded' && status.data.running && !liveProgress;
}

export function BackupSection() {
  const [providersLoadable, setProvidersLoadable] = useState<Loadable<BackupProviderInfo[]>>({ status: 'not-loaded' });
  const [selectedProvider, setSelectedProvider] = useState<string>('');
  const [keysLoadable, setKeysLoadable] = useState<Loadable<BackupKeys>>({ status: 'not-loaded' });
  const [statusLoadable, setStatusLoadable] = useState<Loadable<BackupStatus>>({ status: 'not-loaded' });
  const [schedule, setSchedule] = useState<string>(PREF_BACKUP_SCHEDULE.fallback);
  const [scheduleLoaded, setScheduleLoaded] = useState(false);
  const [scheduleSaving, setScheduleSaving] = useState(false);
  const [retention, setRetention] = useState<string>(PREF_BACKUP_RETENTION.fallback);
  const [retentionSaving, setRetentionSaving] = useState(false);
  const [granting, setGranting] = useState(false);
  const [providerSaving, setProviderSaving] = useState(false);
  const backupPairSaving = providerSaving || scheduleSaving;

  const live = useRef({ provider: '', providers: [] as BackupProviderInfo[], writes: 0 });
  live.current.provider = selectedProvider;
  live.current.providers =
    providersLoadable.status === 'loaded' ? providersLoadable.data : [];

  useEffect(() => {
    setProvidersLoadable({ status: 'loading' });

    void (async () => {
      const [providers, schedule, retention] = await Promise.allSettled([
        getBackupProviders(),
        getBackupSchedule(),
        getBackupRetention(),
      ]);
      const seed = backupSeed(providers, schedule, { provider: '', providers: [] });

      setProvidersLoadable(
        providers.status === 'fulfilled'
          ? { status: 'loaded', data: providers.value }
          : toFailed(providers.reason),
      );

      if (schedule.status === 'rejected') {
        showToast(`Failed to load backup schedule: ${errorDetail(schedule.reason)}`, 'error');
      } else if (seed.schedule !== null) {
        setSchedule(seed.schedule);
        setScheduleLoaded(true);
      }

      setSelectedProvider(seed.provider);

      if (retention.status === 'fulfilled') {
        setRetention(String(retention.value.keep));
      } else {
        showToast(`Failed to load backup retention: ${errorDetail(retention.reason)}`, 'error');
      }
    })();

    void loadKeys();

    if (knownOAuthProviders.value.status === 'not-loaded') void loadKnownOAuthProviders();
  }, []);

  const appliedPreferencesVersion = useRef(backupPreferencesVersion.value);
  const preferenceReads = useRef<Array<{ version: number; settle: () => void }>>([]);
  function settlePreferenceReads(upTo: number): void {
    const waiting = preferenceReads.current;
    preferenceReads.current = waiting.filter((w) => w.version > upTo);
    for (const w of waiting) if (w.version <= upTo) w.settle();
  }
  useEffect(() => {
    const version = backupPreferencesVersion.value;
    if (version <= appliedPreferencesVersion.current) return;
    if (backupPairSaving || retentionSaving || granting) return;
    const writesAtStart = live.current.writes;
    void (async () => {
      const [providers, schedule, retention] = await Promise.allSettled([
        getBackupProviders(),
        getBackupSchedule(),
        getBackupRetention(),
      ]);
      settlePreferenceReads(version);
      if (
        !refreshMayApply({
          version,
          applied: appliedPreferencesVersion.current,
          writesAtStart,
          writesNow: live.current.writes,
        })
      ) {
        return;
      }
      appliedPreferencesVersion.current = version;

      const seed = backupSeed(providers, schedule, {
        provider: live.current.provider,
        providers: live.current.providers,
      });
      if (seed.providers) setProvidersLoadable({ status: 'loaded', data: seed.providers });
      if (seed.schedule !== null) {
        setSchedule(seed.schedule);
        setScheduleLoaded(true);
      }
      setSelectedProvider(seed.provider);
      if (retention.status === 'fulfilled') setRetention(String(retention.value.keep));
    })();
  }, [backupPreferencesVersion.value, backupPairSaving, retentionSaving, granting]);

  const loadedProviders = providersLoadable.status === 'loaded' ? providersLoadable.data : null;

  const providerOptions: { value: string; label: string }[] = (() => {
    if (loadedProviders) return loadedProviders.map((p) => ({ value: p.id, label: p.name }));
    if (providersLoadable.status === 'failed') return [{ value: '', label: 'Failed to load providers' }];
    return [];
  })();

  const selectedReady = loadedProviders?.find((p) => p.id === selectedProvider)?.ready ?? false;

  const writing = backupPairSaving || retentionSaving || granting;
  usePanelRefresh('backup', writing ? null : () => {
    const version = backupPreferencesVersion.peek() + 1;
    const preferences = new Promise<void>((settle) => preferenceReads.current.push({ version, settle }));
    backupPreferencesVersion.value = version;
    return Promise.all([
      preferences,
      loadKeys(),
      selectedProvider && selectedReady ? loadStatus({ keepPrevious: true }) : undefined,
    ]);
  });

  useVersionedRefresh(backupRecipientVersion.value, false, () => { void loadKeys(); });

  const progress = backupProgress.value;

  useEffect(() => {
    if (selectedProvider && selectedReady) {
      void loadStatus();
    } else {
      setStatusLoadable({ status: 'not-loaded' });
    }
  }, [selectedProvider, selectedReady, backupStatusVersion.value]);

  useEffect(() => {
    if (!selectedProvider || !selectedReady) return;
    if (!shouldPollBackupStatus(statusLoadable, progress)) return;
    const t = setTimeout(() => { void loadStatus({ keepPrevious: true }); }, STATUS_POLL_MS);
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
      showToast(`No backup permissions are defined for ${info.name}`, 'error');
      return;
    }
    live.current.writes++;
    setGranting(true);
    const ok = await grantOAuthScope(oauthProviderFor(info.id), scopes);
    if (ok) {
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
    await startBackup(selectedProvider);
  }

  async function loadKeys() {
    setKeysLoadable((prev) => (prev.status === 'loaded' ? prev : { status: 'loading' }));
    try {
      setKeysLoadable({ status: 'loaded', data: await getBackupKeys() });
    } catch (err) {
      setKeysLoadable(toFailed(err));
    }
  }

  async function loadStatus({ keepPrevious = false }: { keepPrevious?: boolean } = {}) {
    const provider = selectedProvider;
    if (!provider) return;
    if (!keepPrevious) setStatusLoadable({ status: 'loading' });
    try {
      const status = await getBackupStatus(provider);
      if (live.current.provider === provider) setStatusLoadable({ status: 'loaded', data: status });
    } catch (err) {
      if (live.current.provider === provider) setStatusLoadable(toFailed(err));
    }
  }

  async function handleProviderChange(newProvider: string) {
    const previous = selectedProvider;
    if (!newProvider || newProvider === previous) return;
    if (!scheduleLoaded) return;
    live.current.writes++;
    setSelectedProvider(newProvider);
    setProviderSaving(true);
    try {
      try {
        await setBackupSchedule(newProvider, schedule);
      } catch (err) {
        setSelectedProvider(previous);
        showToast(`Failed to set backup provider: ${errorDetail(err)}`, 'error');
        return;
      }
      try {
        const p = await getBackupProviders();
        setProvidersLoadable({ status: 'loaded', data: p });
      } catch (err) {
        showToast(`Backup provider saved, but its status could not be refreshed: ${errorDetail(err)}`, 'error');
      }
    } finally {
      setProviderSaving(false);
    }
  }

  async function handleScheduleChange(newSchedule: string) {
    if (!selectedProvider) return;
    live.current.writes++;
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
    live.current.writes++;
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
  const registryLoadable = knownOAuthProviders.value;
  const consoleRow = reauthorizationHint(
    registryLoadable.status === 'loaded' ? registryLoadable.data.providers : [],
    providerInfo ? oauthProviderFor(providerInfo.id) : '',
    !!providerInfo?.connected && !providerInfo.ready,
  );
  const backingUp = progress !== null;
  const keysMissing = keysLoadable.status === 'loaded' && !backupsCanSeal(keysLoadable.data);
  const offerSetup = showBackupSetupOffer(providersLoadable, selectedReady, statusLoadable);

  const pending = backupSlotsPending(providersLoadable, selectedReady, statusLoadable);

  const keysPending = keysLoadable.status === 'not-loaded' || keysLoadable.status === 'loading';
  const skeletonsVisible = useDelayedFlag(pending.controls || pending.status || keysPending);
  const showControlSkeleton = pending.controls && skeletonsVisible;

  const healthCard = backupHealthCard({
    status: statusLoadable,
    liveProgress: progress,
    providerName: providerInfo?.name ?? selectedProvider,
  });
  const showHealthCardSkeleton = pending.status && skeletonsVisible && healthCard === null;

  return (
    <div class="settings-section">
      <div class="settings-section-title">
        Backup
        <Explainer title="Backup">
          <p>Encrypted backups of this workspace, uploaded to your own cloud storage.</p>
          <p>Restore happens from the workspace picker, not from here.</p>
        </Explainer>
      </div>
      {offerSetup && (
        <p class="settings-section-desc">
          <button class="accent-link" onClick={askLucidosToSetUpBackups}>
            Ask Lucidos to set this up
          </button>{' '}
          if you would rather be walked through it.
        </p>
      )}

      <LoadingFade showSkeleton={showHealthCardSkeleton} skeleton={backupHealthCardSkeleton()}>
        {healthCard}
      </LoadingFade>

      <div class="settings-row" data-search-anchor="backup:provider">
        <span class="settings-row-label">
          Provider
          {PROVIDER_ACCESS[selectedProvider] && (
            <Explainer title="Provider">
              <p>{PROVIDER_ACCESS[selectedProvider]}</p>
            </Explainer>
          )}
        </span>
        <DropdownSlot pending={pending.controls} showSkeleton={showControlSkeleton} skeletonWidth="5rem">
          <Dropdown
            options={providerOptions}
            value={selectedProvider}
            disabled={!loadedProviders || !scheduleLoaded || backupPairSaving}
            onChange={(v) => void handleProviderChange(v)}
          />
        </DropdownSlot>
        {providersLoadable.status === 'failed' && (
          <span class="error-text">Failed to load providers: {providersLoadable.error}</span>
        )}
      </div>

      {providerInfo && !providerInfo.connected && (
        <div class="backup-blocked-state">
          <span>No {providerInfo.name} account connected, so backups cannot upload.</span>
          <button
            class="action-btn action-btn-confirm"
            onClick={() =>
              openConnectedAccountsSettings(
                oauthProviderFor(providerInfo.id),
                PROVIDER_SCOPES[providerInfo.id],
              )
            }
          >
            Connect {providerInfo.name}
          </button>
        </div>
      )}

      {providerInfo && providerInfo.connected && !providerInfo.ready && (
        <div class="backup-blocked-state">
          <span>{backupAccessLine(providerInfo.name, providerInfo.missing_scopes)}</span>
          {consoleRow && <ProviderPermissionsHint row={consoleRow} />}
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

      {(pending.controls || scheduleLoaded) && (
        <div class="settings-row" data-search-anchor="backup:schedule">
          <span class="settings-row-label">
            Schedule
            <Explainer title="Schedule">
              <p>Scheduled backups run at the selected time in your timezone (set in Settings → Locale).</p>
            </Explainer>
          </span>
          <DropdownSlot pending={pending.controls} showSkeleton={showControlSkeleton} skeletonWidth="6.5rem">
            <Dropdown
              options={SCHEDULE_OPTIONS}
              value={schedule}
              disabled={backupPairSaving || !selectedProvider}
              onChange={handleScheduleChange}
            />
          </DropdownSlot>
        </div>
      )}
      <div class="settings-row" data-search-anchor="backup:retention">
        <span class="settings-row-label">Keep</span>
        <DropdownSlot pending={pending.controls} showSkeleton={showControlSkeleton} skeletonWidth="4.5rem">
          <Dropdown
            options={RETENTION_OPTIONS}
            value={retention}
            disabled={retentionSaving || !selectedProvider}
            onChange={handleRetentionChange}
          />
        </DropdownSlot>
      </div>
      <div class="backup-actions-row">
        <button
          class="action-btn action-btn-confirm"
          disabled={backingUp || !selectedProvider || !providerInfo?.ready || keysMissing}
          onClick={handleBackup}
        >
          {backingUp ? 'Backing up...' : 'Back up now'}
        </button>
      </div>


      <LoadingFade showSkeleton={keysPending && skeletonsVisible} skeleton={backupKeyPanelSkeleton()}>
        {keysLoadable.status === 'loaded' && (
          <BackupKeyPanel
            keys={keysLoadable.data}
            onKeysChanged={(keys) => setKeysLoadable({ status: 'loaded', data: keys })}
          />
        )}
        {keysLoadable.status === 'failed' && (
          <div class="backup-key">
            <span class="error-text">Failed to load the backup key status: {keysLoadable.error}</span>
          </div>
        )}
      </LoadingFade>
    </div>
  );
}
