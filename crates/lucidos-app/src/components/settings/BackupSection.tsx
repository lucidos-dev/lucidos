import { type ComponentChildren, type VNode } from 'preact';
import { useState, useEffect, useRef } from 'preact/hooks';
import {
  backupPreferencesVersion,
  backupProgress,
  backupStatusVersion,
  knownOAuthProviders,
  showToast,
} from '../../store/store';
import { grantOAuthScope, loadKnownOAuthProviders } from '../../store/actions/oauth';
import {
  ProviderPermissionsHint,
  reauthorizationHint,
} from '../credentials/providerConsoleHint';
import {
  PROVIDER_SCOPES,
  backupAccessLine,
  oauthProviderFor,
} from './backupProviderScopes';
import { backupSeed, refreshMayApply } from './backupSeeding';
import { openConnectedAccountsSettings } from '../../store/actions/menu';
import { handleNavigationRequest } from '../../store/actions/navigation-request';
import { formatTimeAgo } from '../../utils/formatTime';
import { formatBytes } from '../../utils/formatBytes';
import { copyToClipboard } from '../../utils/clipboard';
import { Dropdown, DropdownSkeleton } from '../shared/Dropdown';
import { Explainer } from '../shared/Explainer';
import { LoadingFade } from '../shared/LoadingFade';
import { SkeletonProvider, SkText } from '../shared/Skeleton';
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
import { useDelayedFlag, useDelayedLoading } from '../../hooks/useDelayedLoading';
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

/** Should the "Ask Lucidos to set this up" hand-off render at rest?
 *
 *  Only when the page KNOWS backups are not working yet, which is what makes the
 *  offer a next action rather than reference material. Working means exactly
 *  what `backupHealthCard` renders its plain idle state for: a ready
 *  destination, a backup that really is in the cloud, not stale, and no failed
 *  run since. Anything short of that is something the agent can still help with,
 *  so the offer stays.
 *
 *  **Every unknown answers no**, and that half is load-bearing rather than
 *  cautious. `loadStatus` blanks the status back to `loading` on mount and
 *  after each terminal backup SSE, so reading "not loaded" as "not working"
 *  would flash the offer onto a healthy page and shove the health card down for
 *  the length of each of those round trips. */
export function showBackupSetupOffer(
  providers: Loadable<BackupProviderInfo[]>,
  providerReady: boolean,
  status: Loadable<BackupStatus>,
): boolean {
  // Nothing is known about the destination until its registry lands.
  if (providers.status !== 'loaded') return false;
  // No usable destination is a verdict on its own, and the only one status
  // cannot supply: it is never fetched at all until a provider is ready.
  if (!providerReady) return true;
  if (status.status !== 'loaded') return false;
  const s = status.data;
  // An unreadable destination is not a verdict either way, and the card already
  // says so in its own line. A transient cloud outage must not read as "your
  // backups were never set up".
  if (s.list_error) return false;
  if (!s.latest_backup || s.stale) return true;
  return s.last_run != null && s.last_run.status !== 'success';
}

/**
 * The health card as a loading skeleton: its own box, with a shimmer standing in
 * for each of the two lines the settled card shows.
 *
 * Listing the cloud folder is the slowest read on this page and the card is at
 * the TOP of it, so without this the card arrives last and shoves everything
 * under it down. It is built from the same box and the same line class as the
 * real card, immediately beside it, so the placeholder cannot drift away from
 * what it stands in for (`.claude/rules/frontend.md` on self-skeletonizing
 * surfaces). `data-state` is the neutral idle hue: a skeleton must not
 * pre-announce a verdict, least of all the red one.
 */
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

/**
 * A dropdown that shows a skeleton while its own value is still being read.
 *
 * Every control on this page is fed by a separate request, and each one used to
 * be conditionally rendered on its own flag, so the row grew a control at a time
 * and the page under it stepped down with each arrival. The slot reserves the
 * space instead: the skeleton is the trigger's own box, so the real control lands
 * in it rather than beside it.
 *
 * The delay gate and the crossfade are the standard pair from
 * `.claude/rules/frontend.md`: a load that beats `SPINNER_DELAY_MS` shows no
 * skeleton at all, and a slower one dissolves rather than snapping.
 */
function DropdownSlot({
  pending,
  skeletonWidth,
  children,
}: {
  pending: boolean;
  /** Width of the widest label this slot will settle on. */
  skeletonWidth: string;
  children: ComponentChildren;
}) {
  const showSkeleton = useDelayedFlag(pending);
  return (
    <LoadingFade class="dropdown-slot" showSkeleton={showSkeleton} skeleton={<DropdownSkeleton w={skeletonWidth} />}>
      {/* Withheld only while genuinely unknown. Once the read settles the
          control renders immediately, and the skeleton fades out over it. */}
      {pending ? null : children}
    </LoadingFade>
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
  // SETTLED, not "loaded": set on both the success and the failure path, because
  // it answers "is the read still in flight?" (which drives the skeleton) rather
  // than "is the value known?". Its neighbour `scheduleLoaded` deliberately
  // means the other thing: a failed schedule read leaves it false so the 'off'
  // default is never mistaken for a real setting and written back.
  const [retentionSettled, setRetentionSettled] = useState(false);
  const [retentionSaving, setRetentionSaving] = useState(false);
  const [granting, setGranting] = useState(false);
  const [providerSaving, setProviderSaving] = useState(false);
  // `PUT /backup/schedule` writes backup_provider AND backup_schedule together,
  // and each handler sends the other half from captured state. So an in-flight
  // write of either one has to disable BOTH controls: change the provider, then
  // the schedule before the first lands, and the later response overwrites one
  // choice with its stale counterpart. It also gates the SSE refresh below,
  // which would clobber the same pair from the other side.
  const backupPairSaving = providerSaving || scheduleSaving;

  /** What an in-flight background refresh has to consult at APPLY time.
   *
   *  An effect closure captures the state of the render that created it, which
   *  is exactly the wrong vintage here: the refresh is asynchronous, and a local
   *  write that starts while its reads are in the air is newer than anything
   *  they can return. `writes` is bumped by every write handler, so a refresh
   *  can tell that one happened under it and drop its result rather than
   *  flipping the control the user just moved. */
  const live = useRef({ provider: '', providers: [] as BackupProviderInfo[], writes: 0 });
  live.current.provider = selectedProvider;
  live.current.providers =
    providersLoadable.status === 'loaded' ? providersLoadable.data : [];

  useEffect(() => {
    setProvidersLoadable({ status: 'loading' });

    // The registry and the configured destination are fetched concurrently but
    // applied TOGETHER. Seeding from whichever settled first is what let the
    // registry's first entry (always Google Drive) override a real
    // `backup_provider`; settling both first also means the dropdown never
    // renders one provider and then flips to another.
    void (async () => {
      const [providers, schedule] = await Promise.allSettled([
        getBackupProviders(),
        getBackupSchedule(),
      ]);
      const seed = backupSeed(providers, schedule, { provider: '', providers: [] });

      // The failed LIST is a mount-only outcome: the user just opened the page,
      // so an empty dropdown needs a reason in it. A background refresh keeps
      // whatever was already loaded instead.
      setProvidersLoadable(
        providers.status === 'fulfilled'
          ? { status: 'loaded', data: providers.value }
          : toFailed(providers.reason),
      );

      // `scheduleLoaded` means the value is KNOWN, not that the request
      // settled. A failed read leaves `schedule` at its 'off' default, and one
      // endpoint writes the schedule and the provider together, so treating
      // that default as known would let a provider pick silently disable a real
      // nightly backup. Unknown therefore hides the schedule control and blocks
      // the provider write instead of guessing.
      if (schedule.status === 'rejected') {
        showToast(`Failed to load backup schedule: ${errorDetail(schedule.reason)}`, 'error');
      } else if (seed.schedule !== null) {
        setSchedule(seed.schedule);
        setScheduleLoaded(true);
      }

      // Either request failing degrades the seed rather than skipping it: the
      // page still has to select something, and both failures are already
      // surfaced above.
      setSelectedProvider(seed.provider);
    })();

    getBackupRetention().then((r) => {
      setRetention(String(r.keep));
      setRetentionSettled(true);
    }).catch((err) => {
      setRetentionSettled(true);
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

    // The *OAuth provider registry*, for the console guidance beside *Grant
    // access*. Shared with Settings > Accounts through the same signal, so
    // arriving here after visiting that page costs no second request, and an
    // empty or failed registry simply shows no guidance.
    if (knownOAuthProviders.value.status === 'not-loaded') void loadKnownOAuthProviders();
  }, []);

  // A backup preference changed somewhere this page cannot see: the agent wrote
  // `backup_provider`, or the same settings page is open on another device or in
  // another tab. `PreferencesChanged` reloads the preferences cache, but these
  // three controls are not fed from that cache (they come from /backup/schedule,
  // /backup/providers and /backup/retention, which also carry the connected /
  // ready verdict), so without this the dropdown kept the old destination until
  // a manual reload.
  //
  // Three things separate it from the mount path above, all deliberate:
  //   1. It is SILENT, the best-effort carve-out in `.claude/rules/frontend.md`:
  //      no user intent is on this line, so a toast would arrive out of nowhere
  //      for someone who did nothing. A failed read leaves the last known good
  //      values on screen and recovers on its own, since the next change to any
  //      of these preferences runs the whole refresh again and reopening the
  //      page re-reads from scratch. Anything the user then DOES touch goes
  //      through a handler that toasts its own failure.
  //   2. It never claims a value it did not read. A failed schedule read leaves
  //      `scheduleLoaded` alone rather than treating the 'off' default as known.
  //   3. A local action wins. Both halves matter: one in flight blocks the
  //      refresh from starting, and one that starts while the reads are in the
  //      air discards them. *Grant access* counts, and is the likeliest to
  //      overlap: it waits on a browser round trip, and the ready verdict it
  //      comes back to change is one of the values these reads carry.
  const appliedPreferencesVersion = useRef(backupPreferencesVersion.value);
  useEffect(() => {
    const version = backupPreferencesVersion.value;
    if (version <= appliedPreferencesVersion.current) return;
    // Not marked applied, so this effect picks the change up when the local
    // action finishes (every flag is a dependency) instead of dropping it.
    if (backupPairSaving || retentionSaving || granting) return;
    const writesAtStart = live.current.writes;
    void (async () => {
      const [providers, schedule, retention] = await Promise.allSettled([
        getBackupProviders(),
        getBackupSchedule(),
        getBackupRetention(),
      ]);
      // Two refreshes can be in the air at once (one user action writes two
      // preferences), and a local action can start under either. See
      // `refreshMayApply` for what each half is protecting.
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
      if (retention.status === 'fulfilled') {
        setRetention(String(retention.value.keep));
        setRetentionSettled(true);
      }
    })();
  }, [backupPreferencesVersion.value, backupPairSaving, retentionSaving, granting]);

  const loadedProviders = providersLoadable.status === 'loaded' ? providersLoadable.data : null;

  // No "Loading providers..." entry any more: while the read is in flight the
  // row shows a skeleton in place of the whole control, so a dropdown carrying
  // its own progress report as its only option would be saying it twice.
  const providerOptions: { value: string; label: string }[] = (() => {
    if (loadedProviders) return loadedProviders.map((p) => ({ value: p.id, label: p.name }));
    if (providersLoadable.status === 'failed') return [{ value: '', label: 'Failed to load providers' }];
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
    // A REFRESH of a destination already on screen, so it keeps what is there.
    // Blanking would swap the running card for the loading skeleton every 4s for
    // as long as pruning takes, and the card it is replacing is the one thing
    // on the page saying a backup is still going.
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
      // A provider with no scope entry cannot be granted anything, and a button
      // that quietly does nothing is worse than one that says why.
      showToast(`No backup permissions are defined for ${info.name}`, 'error');
      return;
    }
    // A grant changes the ready verdict an in-flight background refresh is
    // already reading, and it takes as long as the user needs in the browser.
    live.current.writes++;
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
    if (keyInfo) copyToClipboard(keyInfo.key, 'Key copied to clipboard');
  }

  /** `keepPrevious` leaves whatever is on screen there while the new answer is
   *  fetched, for a poll that is re-reading the SAME destination. The default
   *  blanks, and that is what the other two callers need: a provider change
   *  would otherwise report one destination's health under another's name, and
   *  a terminal-backup SSE refetch would hold the pre-backup verdict as if it
   *  still stood. Blanking is what the health card's skeleton keys off, so this
   *  flag is also the choice between "shimmer" and "leave it alone". */
  async function loadStatus({ keepPrevious = false }: { keepPrevious?: boolean } = {}) {
    if (!selectedProvider) return;
    if (!keepPrevious) setStatusLoadable({ status: 'loading' });
    try {
      const status = await getBackupStatus(selectedProvider);
      setStatusLoadable({ status: 'loaded', data: status });
    } catch (err) {
      setStatusLoadable(toFailed(err));
    }
  }

  /** Picking a destination is configuration, so it is persisted immediately.
   *
   *  It used to be view-only state, written to `backup_provider` only as a side
   *  effect of changing the schedule. That is how the page and the preference
   *  came to disagree in the first place.
   *
   *  The current schedule travels with it because one endpoint writes both
   *  keys; sending anything else here would turn a destination change into a
   *  silent schedule change. */
  async function handleProviderChange(newProvider: string) {
    const previous = selectedProvider;
    if (!newProvider || newProvider === previous) return;
    // The write carries the schedule too, so it cannot proceed on a guess. The
    // dropdown is disabled in this state; the guard is here because the state
    // is what makes the write unsafe, not the control.
    if (!scheduleLoaded) return;
    // Claim the controls before anything is awaited: an SSE refresh already in
    // flight has to know its reads predate this pick.
    live.current.writes++;
    setSelectedProvider(newProvider);
    setProviderSaving(true);
    try {
      try {
        await setBackupSchedule(newProvider, schedule);
      } catch (err) {
        // ONLY a failed write rolls back. Leaving the dropdown on a destination
        // the engine refused would make every control below it wrong again.
        setSelectedProvider(previous);
        showToast(`Failed to set backup provider: ${errorDetail(err)}`, 'error');
        return;
      }
      // Written and persisted. Connected / ready is per provider, so the
      // verdict has to be re-read or the page keeps answering Grant access and
      // Back up now for the provider the user just navigated away from. A
      // failure HERE is a stale verdict, not a failed write: rolling back would
      // show a destination the engine is no longer configured for, and report a
      // write that in fact succeeded as failed.
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
  // Guidance for the console the *Grant access* button cannot reach into. Only
  // resolves for a connected provider that is genuinely short of something, so
  // a ready backup destination shows nothing.
  const registryLoadable = knownOAuthProviders.value;
  const consoleRow = reauthorizationHint(
    registryLoadable.status === 'loaded' ? registryLoadable.data.providers : [],
    providerInfo ? oauthProviderFor(providerInfo.id) : '',
    !!providerInfo?.connected && !providerInfo.ready,
  );
  const backingUp = progress !== null;
  const offerSetup = showBackupSetupOffer(providersLoadable, !!providerInfo?.ready, statusLoadable);

  // Which reads are still in flight, and therefore which controls show a
  // skeleton. The destination registry and the schedule are ONE
  // `Promise.allSettled` that is applied together, so they are in flight at
  // exactly the same times and share a flag rather than each guessing at the
  // other's. Retention is its own request and answers for itself. The
  // background refresh on `backupPreferencesVersion` deliberately does NOT go
  // through here: it keeps what is already loaded, so no settled control ever
  // falls back to a skeleton under the user.
  const providersPending =
    providersLoadable.status === 'not-loaded' || providersLoadable.status === 'loading';
  const schedulePending = providersPending;

  const healthCard = backupHealthCard({
    status: statusLoadable,
    liveProgress: progress,
    providerName: providerInfo?.name ?? selectedProvider,
  });
  // Gated on the card having nothing to draw, not merely on the status being in
  // flight: a refetch while a backup runs keeps rendering the live progress
  // card, and stacking a skeleton in the same grid cell would smear the two.
  const showHealthCardSkeleton = useDelayedLoading(statusLoadable) && healthCard === null;

  return (
    <div class="settings-section">
      {/* What backups ARE is static reference material, so it lives behind the
          icon rather than permanently above the controls (see `<Explainer>` and
          `docs/plans/2026-08-09-shared-explainer-info-icon.md`). The section
          carries its own title purely so the icon has something to hang on, the
          same shape Debugging uses under the System switcher. */}
      <div class="settings-section-title">
        Backup
        <Explainer title="Backup">
          <p>Encrypted backups of this workspace, uploaded to your own cloud storage.</p>
          <p>Restore happens from the workspace picker, not from here.</p>
        </Explainer>
      </div>
      {/* Backup setup is spread over two pages (the schedule here, the provider
          account in Settings → Accounts) and one irreversible obligation (save
          the encryption key). That is exactly the shape the agent is good at
          walking someone through, and it can do the connecting itself, so offer
          the hand-off instead of leaving the page as the only route.

          It is a NEXT ACTION, not an explanation, which is why it stays at rest
          instead of joining the copy behind the icon: it is worth a line of the
          page only while there is something left to set up. Once the health card
          below is green there is nothing to walk anyone through, and the offer
          is what made a working page still read as a wall of text. */}
      {offerSetup && (
        <p class="settings-section-desc">
          {/* The `{' '}` after the button stays: its label and the clause
              following it are one sentence, and a bare JSX newline there
              collapses to nothing. */}
          <button class="accent-link" onClick={askLucidosToSetUpBackups}>
            Ask Lucidos to set this up
          </button>{' '}
          if you would rather be walked through it.
        </p>
      )}

      {/* Listing the cloud folder is the slowest read here, and the card is at
          the TOP, so its late arrival pushed the whole page down. The skeleton
          holds its place. Only when the card itself has nothing to draw: while
          a backup runs, `liveProgress` fills the card from SSE and a shimmer
          over it would be hiding live information behind a placeholder. */}
      <LoadingFade showSkeleton={showHealthCardSkeleton} skeleton={backupHealthCardSkeleton()}>
        {healthCard}
      </LoadingFade>

      <div class="settings-row" data-search-anchor="backup:provider">
        <span class="settings-row-label">Provider</span>
        <DropdownSlot pending={providersPending} skeletonWidth="5rem">
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
        // Still an error state (nothing uploads until an account is connected),
        // but now with a way out of it. This was static prose naming a path,
        // which left the user to walk it themselves; the button lands them on
        // the Connected accounts section directly.
        <div class="backup-blocked-state">
          <span>No {providerInfo.name} account connected, so backups cannot upload.</span>
          {/* The deep-link carries the provider AND the scopes an upload needs,
              so the one consent screen covers signing in and granting access.
              Handing over only the provider left the user back here facing
              *Grant access*: a second trip through the same screen for what
              they asked for once. */}
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
        // The other blocked state, and deliberately the same layout: an account
        // exists, so the fix is a re-authorization rather than a connection.
        // The line names the permissions the grant is short whenever the engine
        // reports them, because a bare refusal sentence reads identically after
        // a completed authorization and before one.
        <div class="backup-blocked-state">
          <span>{backupAccessLine(providerInfo.name, providerInfo.missing_scopes)}</span>
          {/* The console step, from the OAuth provider registry. *Grant access*
              re-runs the authorization, and an authorization can only narrow
              what the provider's own console permits: press it before the
              permission is enabled there and it grants the same narrow set
              again, which reads as the button doing nothing. */}
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

      <div class="backup-actions-row">
        <button
          class="action-btn action-btn-confirm"
          disabled={backingUp || !selectedProvider || !providerInfo?.ready}
          onClick={handleBackup}
        >
          {backingUp ? 'Backing up...' : 'Back up now'}
        </button>
        {/* The schedule picker keeps its "render nothing once we know there is
            nothing to show" behaviour: a failed read leaves no control at all,
            because the 'off' default is not a value we may claim. So the SLOT
            has to disappear with it rather than staying as an empty child, or
            the row's gap would open twice where the control used to be. The
            skeleton covers only the window before that verdict exists. */}
        {(schedulePending || scheduleLoaded) && (
          <DropdownSlot pending={schedulePending} skeletonWidth="6.5rem">
            <Dropdown
              options={SCHEDULE_OPTIONS}
              value={schedule}
              disabled={backupPairSaving || !selectedProvider}
              onChange={handleScheduleChange}
            />
          </DropdownSlot>
        )}
        <DropdownSlot pending={!retentionSettled} skeletonWidth="3rem">
          <Dropdown
            options={RETENTION_OPTIONS}
            value={retention}
            disabled={retentionSaving || !selectedProvider}
            onChange={handleRetentionChange}
          />
        </DropdownSlot>
      </div>
      {scheduleLoaded && schedule !== 'off' && (
        <div class="backup-schedule-hint">
          Scheduled backups run at the selected time in your timezone (set in Settings → Locale).
        </div>
      )}

      {/* Live backup progress is shown by the health card at the top of this
          section (backupHealthCard's running branch) — no duplicate bar here. */}

      <div class="backup-key-row">
        <button
          class="action-btn"
          onClick={handleKeyButton}
        >
          {backupKeyButtonLabel(keyExists, showKey)}
        </button>
        {showKey && keyInfo && (
          <>
            <span class="backup-key-value">{keyInfo.key}</span>
            <button class="action-btn" onClick={copyKey}>Copy</button>
          </>
        )}
      </div>
      {showKey && keyInfo && (
        <div class="backup-key-warning">
          Store this key somewhere safe right now — you need it to restore, and it cannot be recovered.
          You restore a backup from the workspace picker.
        </div>
      )}
    </div>
  );
}
