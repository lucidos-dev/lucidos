import { engineVersionStatusVersion, subscribeEngineVersionStatus } from '../engineVersionStatus';
import { cancelEngineVersionReads, startEngineUpdateChecks, engineVersionStatusSlice } from './engine-update';
import { beginSliceRead, settleSliceRead, type SliceLanding } from '../bootstrapSliceLoads';
import type { EngineVersionStatus } from '../../api/client';
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../api/client', () => ({ engineVersionStatus: vi.fn(), rebuildEngine: vi.fn() }));
vi.mock('./chat-changes', () => ({
  initiateEngineRestart: vi.fn(),
  confirmAndRestartEngine: vi.fn(() => Promise.resolve()),
}));
const buildFailure = vi.hoisted(() => ({ announced: null as string | null, dismissed: null as string | null }));
vi.mock('../../hooks/sw-update', () => ({
  noteAnnouncedEngineVersion: vi.fn(),
  wasEngineVersionDismissed: vi.fn(() => false),
  markEngineVersionDismissed: vi.fn(),
  markSwUpdateDismissed: vi.fn(),
  noteAnnouncedBuildFailure: (id: string) => { buildFailure.announced = id; },
  markBuildFailureDismissed: () => { buildFailure.dismissed = buildFailure.announced; },
  wasBuildFailureDismissed: (id: string) => buildFailure.dismissed === id,
}));

import { engineBuildLogOpen } from '../engineBuildLog';
import { showEngineBuildLog, checkEngineVersion, strandedMessage, openEngineVersionToast, resetEngineVersionToastForTest, handleFrontendUpdateDeferred, handleFrontendUpdateStranded, handleEngineBuildStateChanged, handleFrontendRefreshStateChanged, handleEngineVersionStatusChanged, DEFERRED_HINT_STALE_AFTER_MS } from './engine-update';
import { engineVersionStatus, rebuildEngine } from '../../api/client';
import { confirmAndRestartEngine, initiateEngineRestart } from './chat-changes';
import type { BuildFailure, PendingCommits, QueuedBuild, DeferredBuild } from '../../api/client';
import { noteAnnouncedEngineVersion, wasEngineVersionDismissed, markEngineVersionDismissed } from '../../hooks/sw-update';
import { toastTap } from '../../components/shared/toastTap';
import { toasts, engineUnbuiltHead, engineVersionReady, engineVersionPending, engineBuilding, engineBuildDetail, frontendRefreshDetail, enginePendingCommits, engineRestarting, preferences, showToast, dismissToast, FRONTEND_UPDATE_DEFERRED_TOAST_KEY, FRONTEND_UPDATE_STRANDED_TOAST_KEY } from '../store';

const mockStatus = vi.mocked(engineVersionStatus);

function beginBootstrapRead(): (landed: SliceLanding<EngineVersionStatus>) => Promise<void> {
  const start = beginSliceRead('engine_version_status');
  return landed => settleSliceRead('engine_version_status', start, engineVersionStatusSlice, undefined, landed);
}
const bootstrapSnapshot = (body: EngineVersionStatus) => beginBootstrapRead()({ body });
const mockWasDismissed = vi.mocked(wasEngineVersionDismissed);
const mockNoteAnnounced = vi.mocked(noteAnnouncedEngineVersion);
const mockMarkDismissed = vi.mocked(markEngineVersionDismissed);
const mockRebuild = vi.mocked(rebuildEngine);

beforeEach(() => {
  buildFailure.announced = null;
  buildFailure.dismissed = null;
});

function status(over: Partial<{
  build_id: string;
  update_available: boolean;
  disk_build_id: string;
  packaged: boolean;
  build_state: 'idle' | 'building' | 'verifying' | 'ready' | 'failed' | 'deferred';
  build_deferred: DeferredBuild;
  source_behind_head: boolean;
  head_commit: string;
  shared_build_in_progress: boolean;
  build_elapsed_ms: number;
  pending_commits: PendingCommits;
  build_failure: BuildFailure;
  build_queued: QueuedBuild;
  frontend_refresh_elapsed_ms: number;
}> = {}) {
  return {
    build_id: 'eng123',
    update_available: false,
    disk_build_id: 'disk999',
    packaged: false,
    build_state: 'idle' as const,
    source_behind_head: false,
    head_commit: 'head777',
    shared_build_in_progress: false,
    ...over,
  };
}

function hasSwitchToast(): boolean {
  return toasts.value.some((t) => t.key === 'engine-new-version');
}

describe('checkEngineVersion — new-version surface (arrival coupled, INV-C; dismiss defers)', () => {
  beforeEach(() => {
    mockStatus.mockReset();
    resetEngineVersionToastForTest();
    mockWasDismissed.mockReset();
    mockWasDismissed.mockReturnValue(false);
    mockNoteAnnounced.mockReset();
    mockMarkDismissed.mockReset();
    mockRebuild.mockReset();
    mockRebuild.mockResolvedValue(undefined);
    toasts.value = [];
    engineVersionReady.value = false;
    engineVersionPending.value = false;
    engineBuilding.value = false;
    engineRestarting.value = false;
    enginePendingCommits.value = null;
    vi.mocked(confirmAndRestartEngine).mockClear();
    vi.mocked(initiateEngineRestart).mockClear();
    preferences.value = { status: 'loaded', data: {} };
  });

  it('reads once at startup without scheduling successful status reads', async () => {
    vi.useFakeTimers();
    try {
      mockStatus.mockResolvedValue(status({}));
      startEngineUpdateChecks();
      await vi.advanceTimersByTimeAsync(60_000);
      expect(mockStatus).toHaveBeenCalledOnce();
    } finally { vi.useRealTimers(); }
  });

  it('accepts a bootstrap snapshot over an older narrow response', async () => {
    let resolveOlder!: (value: ReturnType<typeof status>) => void;
    mockStatus.mockImplementationOnce(() => new Promise(resolve => { resolveOlder = resolve; }));
    const older = checkEngineVersion();
    await bootstrapSnapshot(status({ update_available: true }));
    resolveOlder(status({ build_state: 'building' }));
    await older;
    expect(engineVersionReady.value).toBe(true);
    expect(engineBuilding.value).toBe(false);
  });

  it('applies a narrow read begun after a bootstrap snapshot left over it', async () => {
    const settleBootstrap = beginBootstrapRead();
    mockStatus.mockResolvedValueOnce(status({ build_state: 'building' }));
    await checkEngineVersion();
    await settleBootstrap({ body: status({ update_available: true }) });
    expect(engineBuilding.value).toBe(true);
    expect(engineVersionReady.value).toBe(false);
  });

  it('lets a newer narrow read settle the slice when an older bootstrap read fails', async () => {
    const settleBootstrap = beginBootstrapRead();
    let resolveNarrow!: (value: ReturnType<typeof status>) => void;
    mockStatus.mockImplementationOnce(() => new Promise(resolve => { resolveNarrow = resolve; }));
    const narrow = checkEngineVersion();
    await settleBootstrap({ error: new Error('slice failed') });
    expect(mockStatus).toHaveBeenCalledOnce();
    resolveNarrow(status({ update_available: true }));
    await narrow;
    expect(engineVersionReady.value).toBe(true);
  });

  it('resets consecutive failures when a bootstrap snapshot succeeds', async () => {
    vi.useFakeTimers();
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => {});
    try {
      await bootstrapSnapshot(status({}));
      mockStatus.mockRejectedValue(new Error('offline'));
      const first = checkEngineVersion();
      await vi.advanceTimersByTimeAsync(250);
      await bootstrapSnapshot(status({}));
      const second = checkEngineVersion();
      await vi.advanceTimersByTimeAsync(250);
      expect(toasts.value.some(toast => toast.message === 'Could not refresh engine version status. Retrying.')).toBe(false);
      await bootstrapSnapshot(status({}));
      await vi.advanceTimersByTimeAsync(1_000);
      await Promise.all([first, second]);
    } finally { warning.mockRestore(); vi.useRealTimers(); }
  });

  it('recovers a transient status-read failure without another event', async () => {
    vi.useFakeTimers();
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => {});
    try {
      mockStatus.mockRejectedValueOnce(new Error('temporary outage'))
        .mockResolvedValueOnce(status({ update_available: true, disk_build_id: 'retry-build' }));
      const read = checkEngineVersion();
      await vi.advanceTimersByTimeAsync(250);
      await read;
      expect(mockStatus).toHaveBeenCalledTimes(2);
      expect(engineVersionReady.value).toBe(true);
      await vi.advanceTimersByTimeAsync(10_000);
      expect(mockStatus).toHaveBeenCalledTimes(2);
    } finally { warning.mockRestore(); vi.useRealTimers(); }
  });

  it('backs off through a longer outage and cancels pending retries when stopped', async () => {
    vi.useFakeTimers();
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => {});
    try {
      mockStatus.mockRejectedValue(new Error('offline'));
      const recovering = checkEngineVersion();
      await vi.advanceTimersByTimeAsync(10_000);
      expect(mockStatus.mock.calls.length).toBeGreaterThan(3);
      expect(toasts.value.filter(toast => toast.message === 'Could not refresh engine version status. Retrying.')).toHaveLength(1);
      mockStatus.mockResolvedValue(status({ update_available: true }));
      await vi.advanceTimersByTimeAsync(30_000);
      await recovering;
      expect(engineVersionReady.value).toBe(true);
      expect(toasts.value.some(toast => toast.message === 'Could not refresh engine version status. Retrying.')).toBe(false);
      const recoveredCalls = mockStatus.mock.calls.length;
      await vi.advanceTimersByTimeAsync(60_000);
      expect(mockStatus).toHaveBeenCalledTimes(recoveredCalls);
      mockStatus.mockRejectedValue(new Error('offline'));
      const stopped = checkEngineVersion();
      await vi.advanceTimersByTimeAsync(0);
      cancelEngineVersionReads();
      await vi.advanceTimersByTimeAsync(10_000);
      await stopped;
      expect(mockStatus).toHaveBeenCalledTimes(recoveredCalls + 1);
    } finally { warning.mockRestore(); vi.useRealTimers(); }
  });

  it('caps repeated outage backoff at thirty seconds and cancels it on a newer read', async () => {
    vi.useFakeTimers();
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => {});
    try {
      mockStatus.mockRejectedValue(new Error('offline'));
      const pending = checkEngineVersion();
      await vi.advanceTimersByTimeAsync(0);
      let calls = 1;
      for (const delay of [250, 1_000, 4_000, 16_000, 30_000, 30_000]) {
        await vi.advanceTimersByTimeAsync(delay - 1);
        expect(mockStatus).toHaveBeenCalledTimes(calls);
        await vi.advanceTimersByTimeAsync(1);
        expect(mockStatus).toHaveBeenCalledTimes(++calls);
      }
      mockStatus.mockResolvedValue(status({ update_available: true }));
      await checkEngineVersion();
      await vi.advanceTimersByTimeAsync(60_000);
      await pending;
      expect(mockStatus).toHaveBeenCalledTimes(calls + 1);
    } finally {
      cancelEngineVersionReads();
      warning.mockRestore();
      vi.useRealTimers();
    }
  });

  it('reads version status once after a stream gap and stops reading after disposal', async () => {
    const { repairGapState } = await import('../eventGap');
    mockStatus.mockResolvedValue(status({ update_available: true, disk_build_id: 'gap-build' }));
    const stop = subscribeEngineVersionStatus(() => { void checkEngineVersion(); });
    try {
      expect(mockStatus).not.toHaveBeenCalled();
      repairGapState();
      await vi.waitFor(() => expect(engineVersionReady.value).toBe(true));
      expect(mockStatus).toHaveBeenCalledOnce();
      stop();
      repairGapState();
      expect(mockStatus).toHaveBeenCalledOnce();
    } finally {
      stop();
    }
  });

  it('keeps the final build result when an older read arrives last', async () => {
    let resolveOlder!: (value: ReturnType<typeof status>) => void;
    mockStatus.mockImplementationOnce(() => new Promise(resolve => { resolveOlder = resolve; }));
    const olderRead = checkEngineVersion();
    mockStatus.mockResolvedValueOnce(status({ build_state: 'ready', update_available: true, disk_build_id: 'new-build' }));
    await checkEngineVersion();
    expect(engineVersionReady.value).toBe(true);
    resolveOlder(status({ build_state: 'building' }));
    await olderRead;
    expect(engineVersionReady.value).toBe(true);
    expect(engineBuilding.value).toBe(false);
  });

  it('narrates a queued build as waiting, with what holds the slots', async () => {
    mockStatus.mockResolvedValue(
      status({ build_state: 'building', build_elapsed_ms: 1_000, build_queued: { holders: ['make lint'] } }),
    );
    await checkEngineVersion();
    expect(engineBuilding.value).toBe(true);
    expect(engineBuildDetail.value?.queuedBehind).toEqual(['make lint']);
  });

  it('never lets a queued field dress up a failed build as a running one', async () => {
    mockStatus.mockResolvedValue(
      status({
        build_state: 'failed',
        build_failure: { summary: 'error: boom', repeatable: false },
        build_queued: { holders: ['make lint'] },
      }),
    );
    await checkEngineVersion();
    expect(engineBuilding.value).toBe(false);
    expect(engineBuildDetail.value).toBeNull();
  });

  it('never lets a queued field hold back a version that is ready to switch onto', async () => {
    mockStatus.mockResolvedValue(
      status({ update_available: true, build_state: 'ready', build_queued: { holders: ['make lint'] } }),
    );
    await checkEngineVersion();
    expect(engineBuilding.value).toBe(false);
    expect(engineVersionReady.value).toBe(true);
    expect(hasSwitchToast()).toBe(true);
  });

  it('sets the badge AND the Switch toast together when a newer build is ready', async () => {
    mockStatus.mockResolvedValue(status({ update_available: true, build_state: 'ready' }));
    await checkEngineVersion();
    expect(engineVersionReady.value).toBe(true);
    const toast = toasts.value.find((t) => t.key === 'engine-new-version');
    expect(toast?.action?.label).toBe('Switch');
    expect(toast?.secondaryAction?.label).toBe('Later');
    expect(mockNoteAnnounced).toHaveBeenCalledWith('disk999');
  });

  it('opens the confirm rather than restarting, so the version can be read first', async () => {
    mockStatus.mockResolvedValue(status({ update_available: true, build_state: 'ready' }));
    await checkEngineVersion();
    toasts.value.find((t) => t.key === 'engine-new-version')?.action?.onClick();
    expect(confirmAndRestartEngine).toHaveBeenCalledOnce();
    expect(initiateEngineRestart).not.toHaveBeenCalled();
  });

  it('holds the pending range where the confirm can read it, with no build in flight', async () => {
    const commits: PendingCommits = {
      total: 1,
      groups: [{ kind: 'new', total: 1, descriptions: ['a thing'] }],
    };
    mockStatus.mockResolvedValue(
      status({ update_available: true, build_state: 'ready', pending_commits: commits }),
    );
    await checkEngineVersion();
    expect(engineBuilding.value).toBe(false);
    expect(engineBuildDetail.value).toBeNull();
    expect(enginePendingCommits.value).toEqual(commits);
  });

  it('clears the pending range when the engine stops reporting one', async () => {
    enginePendingCommits.value = { total: 4, groups: [] };
    mockStatus.mockResolvedValue(status());
    await checkEngineVersion();
    expect(enginePendingCommits.value).toBeNull();
  });

  it('drops a pre-grouping payload rather than reading `groups` off it', async () => {
    enginePendingCommits.value = { total: 1, groups: [] };
    mockStatus.mockResolvedValue({
      ...status({ update_available: true, build_state: 'ready' }),
      pending_commits: { total: 3, subjects: ['legacy'] } as unknown as PendingCommits,
    });
    await checkEngineVersion();
    expect(enginePendingCommits.value).toBeNull();
  });

  it('does not surface a new version while a build is still in progress, even if the on-disk binary already differs', async () => {
    mockStatus.mockResolvedValue(status({ update_available: true, build_state: 'building' }));
    await checkEngineVersion();
    expect(engineVersionReady.value).toBe(false);
    expect(hasSwitchToast()).toBe(false);
    expect(engineBuilding.value).toBe(true);
  });

  it('clears the building badge once the rebuild is ready', async () => {
    engineBuilding.value = true;
    mockStatus.mockResolvedValue(status({ update_available: true, build_state: 'ready' }));
    await checkEngineVersion();
    expect(engineBuilding.value).toBe(false);
    expect(engineVersionReady.value).toBe(true);
  });

  it('clears badge AND toast together when a new build starts (sticky action toast must not stay clickable mid-build)', async () => {
    engineVersionReady.value = true;
    showToast('New version available.', 'info', {
      key: 'engine-new-version',
      action: { label: 'Switch', onClick: () => {} },
    });
    expect(hasSwitchToast()).toBe(true);
    mockStatus.mockResolvedValue(status({ update_available: true, build_state: 'building' }));
    await checkEngineVersion();
    expect(engineVersionReady.value).toBe(false);
    expect(hasSwitchToast()).toBe(false);
  });

  it('keeps the badge lit but suppresses the toast for an on-disk build already dismissed', async () => {
    mockWasDismissed.mockImplementation((id) => id === 'disk999');
    mockStatus.mockResolvedValue(status({ update_available: true, build_state: 'ready', disk_build_id: 'disk999' }));
    await checkEngineVersion();
    expect(engineVersionReady.value).toBe(true);
    expect(hasSwitchToast()).toBe(false);
  });

  it('dismissing defers: removes the Switch toast, keeps the badge lit, remembers the build', async () => {
    mockStatus.mockResolvedValue(status({ update_available: true, build_state: 'ready', disk_build_id: 'disk999' }));
    await checkEngineVersion();
    expect(hasSwitchToast()).toBe(true);
    expect(engineVersionReady.value).toBe(true);
    dismissToast('engine-new-version');
    expect(hasSwitchToast()).toBe(false);
    expect(engineVersionReady.value).toBe(true);
    expect(mockMarkDismissed).toHaveBeenCalled();
  });

  it('re-surfaces badge + toast for a genuinely newer on-disk build after a prior dismiss', async () => {
    mockWasDismissed.mockImplementation((id) => id === 'disk-old');
    mockStatus.mockResolvedValue(status({ update_available: true, build_state: 'ready', disk_build_id: 'disk-new' }));
    await checkEngineVersion();
    expect(engineVersionReady.value).toBe(true);
    expect(hasSwitchToast()).toBe(true);
    expect(mockNoteAnnounced).toHaveBeenCalledWith('disk-new');
  });

  it('shows a build-failed error toast and does not light the badge on a failed rebuild', async () => {
    engineBuilding.value = true;
    mockStatus.mockResolvedValue(status({ build_state: 'failed' }));
    await checkEngineVersion();
    expect(engineVersionReady.value).toBe(false);
    expect(engineBuilding.value).toBe(false);
    const toast = toasts.value.find((t) => t.key === 'engine-build-failed');
    expect(toast?.type).toBe('error');
  });

  it('is a no-op in a packaged build (release updater owns that path)', async () => {
    engineBuilding.value = true;
    mockStatus.mockResolvedValue(status({ packaged: true, update_available: true, build_state: 'ready' }));
    await checkEngineVersion();
    expect(engineVersionReady.value).toBe(false);
    expect(hasSwitchToast()).toBe(false);
    expect(engineBuilding.value).toBe(false);
  });

  it('does not poll while a switch is already in flight', async () => {
    engineRestarting.value = true;
    await checkEngineVersion();
    expect(mockStatus).not.toHaveBeenCalled();
  });

  it('skips entirely until preferences load (durable global dismissal not yet known)', async () => {
    preferences.value = { status: 'loading' };
    await checkEngineVersion();
    expect(mockStatus).not.toHaveBeenCalled();
    expect(hasSwitchToast()).toBe(false);
    expect(engineVersionReady.value).toBe(false);
  });

  it('clears the badge when no newer version is available', async () => {
    engineVersionReady.value = true;
    mockStatus.mockResolvedValue(status({ update_available: false, build_state: 'idle' }));
    await checkEngineVersion();
    expect(engineVersionReady.value).toBe(false);
  });

  it('announces nothing for a finished rebuild that produced no newer binary (no phantom Switch)', async () => {
    mockStatus.mockResolvedValue(status({ update_available: false, build_state: 'ready' }));
    await checkEngineVersion();
    expect(hasSwitchToast()).toBe(false);
    expect(engineVersionReady.value).toBe(false);
  });

  it('keeps the build action when a finished rebuild left the version still pending', async () => {
    mockStatus.mockResolvedValue(status({
      source_behind_head: true,
      update_available: false,
      build_state: 'ready',
      shared_build_in_progress: false,
    }));
    await checkEngineVersion();
    const toast = toasts.value.find((t) => t.key === 'engine-new-version');
    expect(toast?.action?.label).toBe('Build new version');
    expect(engineVersionReady.value).toBe(false);
  });

  it('surfaces a pending-version toast with a build action when source is behind but no fresh binary exists', async () => {
    mockStatus.mockResolvedValue(status({ source_behind_head: true, update_available: false, build_state: 'idle' }));
    await checkEngineVersion();
    const toast = toasts.value.find((t) => t.key === 'engine-new-version');
    expect(toast?.action?.label).toBe('Build new version');
    expect(toast?.action?.label).not.toBe('Switch');
    expect(engineVersionReady.value).toBe(false);
  });

  it('pending-version build action joins and never switches automatically', async () => {
    mockStatus.mockResolvedValue(status({ source_behind_head: true, build_state: 'idle' }));
    await checkEngineVersion();
    toasts.value.find((t) => t.key === 'engine-new-version')?.action?.onClick();
    expect(mockRebuild).toHaveBeenCalledWith('join');
    expect(confirmAndRestartEngine).not.toHaveBeenCalled();
    toasts.value.find((t) => t.key === 'engine-new-version')?.action?.onClick();
    expect(mockRebuild).toHaveBeenLastCalledWith('join');
  });

  it('does NOT nag a build toast when a ready binary exists but its Switch was dismissed (source still behind)', async () => {
    mockWasDismissed.mockImplementation((id) => id === 'disk999');
    mockStatus.mockResolvedValue(status({ source_behind_head: true, update_available: true, build_state: 'ready', disk_build_id: 'disk999' }));
    await checkEngineVersion();
    expect(engineVersionReady.value).toBe(true);
    expect(hasSwitchToast()).toBe(false);
  });

  it('does not surface the pending toast while the self-heal rebuild is building (spinner covers it)', async () => {
    mockStatus.mockResolvedValue(status({ source_behind_head: true, build_state: 'building' }));
    await checkEngineVersion();
    expect(hasSwitchToast()).toBe(false);
    expect(engineBuilding.value).toBe(true);
  });

  it('shows the building spinner (not a pending build toast) when a co-located peer is building the shared binary', async () => {
    mockStatus.mockResolvedValue(status({
      source_behind_head: true,
      update_available: false,
      build_state: 'idle',
      shared_build_in_progress: true,
    }));
    await checkEngineVersion();
    expect(hasSwitchToast()).toBe(false);
    expect(engineBuilding.value).toBe(true);
    expect(engineVersionReady.value).toBe(false);
  });

  it('still shows the pending build toast (no spinner) when nothing is building', async () => {
    mockStatus.mockResolvedValue(status({
      source_behind_head: true,
      update_available: false,
      build_state: 'idle',
      shared_build_in_progress: false,
    }));
    await checkEngineVersion();
    const toast = toasts.value.find((t) => t.key === 'engine-new-version');
    expect(toast?.action?.label).toBe('Build new version');
    expect(engineBuilding.value).toBe(false);
  });

  it('a failed rebuild offers a Retry action (not a dead-end)', async () => {
    mockStatus.mockResolvedValue(status({ source_behind_head: true, build_state: 'failed' }));
    await checkEngineVersion();
    const toast = toasts.value.find((t) => t.key === 'engine-build-failed');
    expect(toast?.type).toBe('error');
    expect(toast?.action?.label).toBe('Retry build');
    toast?.action?.onClick();
    expect(mockRebuild).toHaveBeenCalled();
  });
});

describe('the build-failed toast says what broke, and what to do', () => {
  beforeEach(() => {
    mockStatus.mockReset();
    resetEngineVersionToastForTest();
    mockWasDismissed.mockReset();
    mockWasDismissed.mockReturnValue(false);
    mockRebuild.mockReset();
    mockRebuild.mockResolvedValue(undefined);
    toasts.value = [];
    engineVersionReady.value = false;
    engineVersionPending.value = false;
    engineBuilding.value = false;
    engineRestarting.value = false;
    preferences.value = { status: 'loaded', data: {} };
  });

  const failedWith = (build_failure?: BuildFailure) =>
    status({ source_behind_head: true, build_state: 'failed', build_failure });
  const failedToast = () => toasts.value.find((t) => t.key === 'engine-build-failed');

  it('stays dismissed while the same failure stands', async () => {
    mockStatus.mockResolvedValue(failedWith({ summary: 'error: boom', repeatable: false }));
    await checkEngineVersion();
    dismissToast('engine-build-failed');
    await checkEngineVersion();
    expect(failedToast()).toBeUndefined();
  });

  it('shows a retry that fails the same way, when the user never dismissed it', async () => {
    const failed = failedWith({ summary: 'error: boom', repeatable: false });
    mockStatus.mockResolvedValue(failed);
    await checkEngineVersion();
    mockStatus.mockResolvedValue(status({ source_behind_head: true, build_state: 'building' }));
    await checkEngineVersion();
    expect(failedToast()).toBeUndefined();
    mockStatus.mockResolvedValue(failed);
    await checkEngineVersion();
    expect(failedToast()?.message).toContain('error: boom');
  });

  it.each([
    ['a different cause', { head_commit: 'head777', summary: 'error: other' }],
    ['a newer source', { head_commit: 'head888', summary: 'error: boom' }],
  ])('comes back for %s', async (_, next) => {
    mockStatus.mockResolvedValue(failedWith({ summary: 'error: boom', repeatable: false }));
    await checkEngineVersion();
    dismissToast('engine-build-failed');
    mockStatus.mockResolvedValue({
      ...failedWith({ summary: next.summary, repeatable: false }),
      head_commit: next.head_commit,
    });
    await checkEngineVersion();
    expect(failedToast()?.message).toContain(next.summary);
  });

  it('names the actual error instead of pointing at a log', async () => {
    mockStatus.mockResolvedValue(failedWith({
      summary: 'error[E0308]: mismatched types',
      repeatable: false,
    }));
    await checkEngineVersion();
    expect(failedToast()?.message).toContain('error[E0308]: mismatched types');
    expect(failedToast()?.message).not.toContain('engine log');
  });

  it.each([false, true])('keeps Retry for repeatable=%s', async (repeatable) => {
    mockStatus.mockResolvedValue(failedWith({ summary: 'Publish failed', repeatable }));
    await checkEngineVersion();
    expect(failedToast()?.action?.label).toBe('Retry build');
    expect(failedToast()?.secondaryAction?.label).toBe('Show build log');
    failedToast()?.action?.onClick();
    expect(mockRebuild).toHaveBeenCalledOnce();
  });

  it('keeps Retry when the cause was not reported at all', async () => {
    mockStatus.mockResolvedValue(failedWith(undefined));
    await checkEngineVersion();
    const toast = failedToast();
    expect(toast?.type).toBe('error');
    expect(toast?.action?.label).toBe('Retry build');
    expect(toast?.message).toContain('the engine did not report why');
    expect(toast?.message).not.toContain('could not read');
  });

  it('an old engine that cannot describe its failure still gets a usable toast', async () => {
    mockStatus.mockResolvedValue(status({ source_behind_head: true, build_state: 'failed' }));
    await checkEngineVersion();
    expect(failedToast()?.action?.label).toBe('Retry build');
  });
});

describe('checkEngineVersion: the pending version is dismissable, and the badge is the way back', () => {
  const PENDING = { source_behind_head: true, update_available: false, build_state: 'idle' as const };

  beforeEach(() => {
    mockStatus.mockReset();
    resetEngineVersionToastForTest();
    mockWasDismissed.mockReset();
    mockWasDismissed.mockReturnValue(false);
    mockNoteAnnounced.mockReset();
    mockMarkDismissed.mockReset();
    mockRebuild.mockReset();
    mockRebuild.mockResolvedValue(undefined);
    toasts.value = [];
    engineVersionReady.value = false;
    engineVersionPending.value = false;
    engineBuilding.value = false;
    engineRestarting.value = false;
    preferences.value = { status: 'loaded', data: {} };
  });

  it('an unknown build state retains the older pending-version escape hatch', async () => {
    mockStatus.mockResolvedValue(status({ ...PENDING, build_state: 'future-state' as 'idle' }));
    await checkEngineVersion();
    expect(toasts.value.find((t) => t.key === 'engine-new-version')?.action?.label).toBe('Build new version');
  });

  it('explains the hold and lets Build now explicitly join', async () => {
    mockStatus.mockResolvedValue(status({ ...PENDING, head_commit: 'pending-head', build_state: 'deferred',
      build_deferred: { coding_threads: 2, threads: [], deferred_ms: 840000 } }));
    await checkEngineVersion();
    const toast = toasts.value.find((t) => t.key === 'engine-new-version');
    expect(engineUnbuiltHead.value).toBe('pending-head');
    expect(toast?.message).toBe('New engine version waits for 2 coding threads to finish.');
    expect(toast?.action?.label).toBe('Build now');
    toast?.action?.onClick();
    expect(mockRebuild).toHaveBeenCalledWith('join');
  });

  it('keeps Switch when a newer build is held behind a ready binary', async () => {
    engineUnbuiltHead.value = 'pending-head';
    mockStatus.mockResolvedValue(status({ ...PENDING, update_available: true, build_state: 'ready',
      build_deferred: { coding_threads: 1, threads: [], deferred_ms: 100 } }));
    await checkEngineVersion();
    expect(toasts.value.find((t) => t.key === 'engine-new-version')?.action?.label).toBe('Switch');
    expect(engineUnbuiltHead.value).toBeNull();
  });

  it('lights the pending badge and pins the announcement to the checkout HEAD', async () => {
    mockStatus.mockResolvedValue(status({ ...PENDING, head_commit: 'head777' }));
    await checkEngineVersion();
    expect(engineVersionPending.value).toBe(true);
    expect(mockNoteAnnounced).toHaveBeenCalledWith('head777');
  });

  it('offers a Later beside the Rebuild, like the Switch toast does', async () => {
    mockStatus.mockResolvedValue(status(PENDING));
    await checkEngineVersion();
    const toast = toasts.value.find((t) => t.key === 'engine-new-version');
    expect(toast?.action?.label).toBe('Build new version');
    expect(toast?.secondaryAction?.label).toBe('Later');
  });

  it('a dismissal survives repeated polls at the same HEAD', async () => {
    mockStatus.mockResolvedValue(status({ ...PENDING, head_commit: 'head777' }));
    await checkEngineVersion();
    expect(hasSwitchToast()).toBe(true);

    dismissToast('engine-new-version');
    expect(mockMarkDismissed).toHaveBeenCalled();
    mockWasDismissed.mockImplementation((id) => id === 'head777');

    await checkEngineVersion();
    await checkEngineVersion();
    await checkEngineVersion();
    expect(hasSwitchToast()).toBe(false);
    expect(engineVersionPending.value).toBe(true);
  });

  it('new commits re-announce it: the dismissal was about the old HEAD', async () => {
    mockWasDismissed.mockImplementation((id) => id === 'head-old');
    mockStatus.mockResolvedValue(status({ ...PENDING, head_commit: 'head-new' }));
    await checkEngineVersion();
    expect(hasSwitchToast()).toBe(true);
    expect(mockNoteAnnounced).toHaveBeenCalledWith('head-new');
  });

  it('the badge brings a dismissed toast back, and the next poll leaves it alone', async () => {
    mockWasDismissed.mockImplementation((id) => id === 'head777');
    mockStatus.mockResolvedValue(status({ ...PENDING, head_commit: 'head777' }));
    await checkEngineVersion();
    expect(hasSwitchToast()).toBe(false);

    openEngineVersionToast();
    expect(hasSwitchToast()).toBe(true);

    await checkEngineVersion();
    expect(hasSwitchToast()).toBe(true);
  });

  it('re-opening shows nothing when nothing is pending', () => {
    openEngineVersionToast();
    expect(hasSwitchToast()).toBe(false);
  });

  it('a dismissal from another device closes a toast this one still has up', async () => {
    mockStatus.mockResolvedValue(status({ ...PENDING, head_commit: 'head777' }));
    await checkEngineVersion();
    expect(hasSwitchToast()).toBe(true);

    mockWasDismissed.mockImplementation((id) => id === 'head777');
    await checkEngineVersion();
    expect(hasSwitchToast()).toBe(false);
    expect(engineVersionPending.value).toBe(true);
  });

  it('dismissing a re-opened toast sticks', async () => {
    mockWasDismissed.mockImplementation((id) => id === 'head777');
    mockStatus.mockResolvedValue(status({ ...PENDING, head_commit: 'head777' }));
    await checkEngineVersion();

    openEngineVersionToast();
    expect(hasSwitchToast()).toBe(true);

    dismissToast('engine-new-version');
    await checkEngineVersion();
    await checkEngineVersion();
    expect(hasSwitchToast()).toBe(false);
  });

  it.each([false, true])('verification stays progress with update_available=%s', async (update_available) => {
    mockStatus.mockResolvedValue(status({ ...PENDING, build_state: 'verifying', update_available }));
    await checkEngineVersion();
    expect(engineBuilding.value).toBe(true);
    expect(engineVersionReady.value).toBe(false);
    expect(engineVersionPending.value).toBe(false);
    expect(hasSwitchToast()).toBe(false);
  });

  it.each([
    ['a build is in flight', status({ ...PENDING, build_state: 'building' })],
    ['a co-located peer is building', status({ ...PENDING, shared_build_in_progress: true })],
    ['a switchable binary landed', status({ source_behind_head: true, update_available: true, build_state: 'ready' })],
    ['nothing is pending at all', status({})],
    ['this is a packaged build', status({ ...PENDING, packaged: true })],
  ])('drops the pending badge once %s', async (_case, next) => {
    mockStatus.mockResolvedValue(status(PENDING));
    await checkEngineVersion();
    expect(engineVersionPending.value).toBe(true);

    mockStatus.mockResolvedValue(next);
    await checkEngineVersion();
    expect(engineVersionPending.value).toBe(false);
  });

  it('the pending badge stands down for a failed build, which owns its own toast and Retry', async () => {
    mockStatus.mockResolvedValue(status({ source_behind_head: true, build_state: 'failed' }));
    await checkEngineVersion();
    expect(engineVersionPending.value).toBe(false);
    expect(toasts.value.find((t) => t.key === 'engine-build-failed')?.action?.label).toBe('Retry build');
  });
});

describe('checkEngineVersion: the build narration cannot outlive the build', () => {
  beforeEach(() => {
    mockStatus.mockReset();
    resetEngineVersionToastForTest();
    mockWasDismissed.mockReset();
    mockWasDismissed.mockReturnValue(false);
    mockNoteAnnounced.mockReset();
    mockRebuild.mockReset();
    mockRebuild.mockResolvedValue(undefined);
    toasts.value = [];
    engineVersionReady.value = false;
    engineBuilding.value = false;
    engineBuildDetail.value = null;
    engineRestarting.value = false;
    preferences.value = { status: 'loaded', data: {} };
  });

  it('carries the elapsed time and the commits while a build runs', async () => {
    mockStatus.mockResolvedValue(status({
      build_state: 'building',
      source_behind_head: true,
      build_elapsed_ms: 42_000,
      pending_commits: {
        total: 2,
        groups: [
          { kind: 'fixed', total: 1, descriptions: ['one'] },
          { kind: 'housekeeping', total: 1, descriptions: [] },
        ],
      },
    }));
    await checkEngineVersion();
    expect(engineBuilding.value).toBe(true);
    expect(engineBuildDetail.value?.elapsedMs).toBe(42_000);
    expect(engineBuildDetail.value?.pendingCommits?.total).toBe(2);
    expect(engineBuildDetail.value?.anchoredAt).toBeLessThanOrEqual(Date.now());
  });

  it('survives an engine that predates the grouped commit shape', async () => {
    mockStatus.mockResolvedValue(status({
      build_state: 'building',
      build_elapsed_ms: 42_000,
      pending_commits: { total: 79, subjects: ["Merge branch 'main' into x"] } as unknown as PendingCommits,
    }));
    await checkEngineVersion();
    expect(engineBuilding.value).toBe(true);
    expect(engineBuildDetail.value?.elapsedMs).toBe(42_000);
    expect(engineBuildDetail.value?.pendingCommits).toBeNull();
  });

  it.each([
    ['the build failed', status({ source_behind_head: true, build_state: 'failed' })],
    ['the build finished', status({ build_state: 'idle' })],
    ['this is a packaged build', status({ packaged: true })],
  ])('clears both halves once %s', async (_case, next) => {
    mockStatus.mockResolvedValue(status({
      build_state: 'building',
      build_elapsed_ms: 42_000,
      pending_commits: {
        total: 1,
        groups: [{ kind: 'fixed', total: 1, descriptions: ['one'] }],
      },
    }));
    await checkEngineVersion();
    expect(engineBuildDetail.value).not.toBeNull();

    mockStatus.mockResolvedValue(next);
    await checkEngineVersion();
    expect(engineBuilding.value).toBe(false);
    expect(engineBuildDetail.value).toBeNull();
  });

  it("reports a peer's build with commits but no timer", async () => {
    mockStatus.mockResolvedValue(status({
      build_state: 'idle',
      shared_build_in_progress: true,
      source_behind_head: true,
      update_available: false,
      pending_commits: {
        total: 1,
        groups: [{ kind: 'fixed', total: 1, descriptions: ['one'] }],
      },
    }));
    await checkEngineVersion();
    expect(engineBuilding.value).toBe(true);
    expect(engineBuildDetail.value?.elapsedMs).toBeNull();
    expect(engineBuildDetail.value?.pendingCommits?.groups).toEqual([
      { kind: 'fixed', total: 1, descriptions: ['one'] },
    ]);
  });
});

describe('handleFrontendUpdateDeferred — deferral hint (keyed, freshness-gated)', () => {
  function deferredToasts() {
    return toasts.value.filter((t) => t.key === FRONTEND_UPDATE_DEFERRED_TOAST_KEY);
  }

  beforeEach(() => {
    toasts.value = [];
  });

  it('shows a keyed info toast for a fresh event', () => {
    handleFrontendUpdateDeferred({ sent_at_ms: Date.now() });
    const shown = deferredToasts();
    expect(shown).toHaveLength(1);
    expect(shown[0].type).toBe('info');
  });

  it('offers NO Switch action, since the deferral can fire mid-build, when switching is unsafe', () => {
    handleFrontendUpdateDeferred({ sent_at_ms: Date.now() });
    const shown = deferredToasts()[0];
    expect(shown.action).toBeUndefined();
    expect(shown.secondaryAction).toBeUndefined();
  });

  it('is a passive hint: a tap on the card does nothing, so it reads as no link', () => {
    handleFrontendUpdateDeferred({ sent_at_ms: Date.now() });
    const shown = deferredToasts()[0];
    expect(toastTap(shown)).toBeNull();
    expect(shown.persistent).toBe(true);
    dismissToast(FRONTEND_UPDATE_DEFERRED_TOAST_KEY);
    expect(deferredToasts()).toHaveLength(0);
  });

  it('coalesces repeated frontend-only applies into a single toast (keyed)', () => {
    handleFrontendUpdateDeferred({ sent_at_ms: Date.now() });
    handleFrontendUpdateDeferred({ sent_at_ms: Date.now() });
    handleFrontendUpdateDeferred({ sent_at_ms: Date.now() });
    expect(deferredToasts()).toHaveLength(1);
  });

  it('drops a stale event (late SSE-queue flush after the Switch already happened)', () => {
    handleFrontendUpdateDeferred({ sent_at_ms: Date.now() - (DEFERRED_HINT_STALE_AFTER_MS + 1000) });
    expect(deferredToasts()).toHaveLength(0);
  });
});

describe('handleEngineBuildStateChanged — SSE poke re-runs the authoritative check', () => {
  beforeEach(() => {
    mockStatus.mockReset();
    resetEngineVersionToastForTest();
    mockWasDismissed.mockReset();
    mockWasDismissed.mockReturnValue(false);
    toasts.value = [];
    engineVersionReady.value = false;
    engineBuilding.value = false;
    engineRestarting.value = false;
    preferences.value = { status: 'loaded', data: {} };
  });

  it('lights the building spinner when the authoritative status reports a real build', async () => {
    mockStatus.mockResolvedValue(status({ update_available: true, build_state: 'building' }));
    handleEngineBuildStateChanged();
    await Promise.resolve();
    await Promise.resolve();
    expect(mockStatus).toHaveBeenCalled();
    expect(engineBuilding.value).toBe(true);
    expect(engineVersionReady.value).toBe(false);
  });

  it('does NOT spin on a stale poke once the authoritative status says the build finished (only spins for a real build)', async () => {
    engineBuilding.value = true;
    mockStatus.mockResolvedValue(status({ update_available: true, build_state: 'ready' }));
    handleEngineBuildStateChanged();
    await Promise.resolve();
    await Promise.resolve();
    expect(engineBuilding.value).toBe(false);
    expect(engineVersionReady.value).toBe(true);
  });
});

describe('frontend refresh after a frontend-only Apply', () => {
  beforeEach(() => {
    mockStatus.mockReset();
    resetEngineVersionToastForTest();
    mockWasDismissed.mockReset();
    mockWasDismissed.mockReturnValue(false);
    toasts.value = [];
    engineBuilding.value = false;
    engineBuildDetail.value = null;
    frontendRefreshDetail.value = null;
    engineRestarting.value = false;
    preferences.value = { status: 'loaded', data: {} };
  });

  it('carries the elapsed time while the engine waits for the rebuild', async () => {
    mockStatus.mockResolvedValue(status({ frontend_refresh_elapsed_ms: 7_000 }));
    await checkEngineVersion();
    expect(frontendRefreshDetail.value?.elapsedMs).toBe(7_000);
    expect(frontendRefreshDetail.value?.anchoredAt).toBeLessThanOrEqual(Date.now());
    expect(engineBuilding.value).toBe(false);
  });

  it.each([
    ['the wait ended', status()],
    ['this is a packaged build', status({ packaged: true, frontend_refresh_elapsed_ms: 7_000 })],
  ])('clears once %s', async (_case, next) => {
    mockStatus.mockResolvedValue(status({ frontend_refresh_elapsed_ms: 7_000 }));
    await checkEngineVersion();
    expect(frontendRefreshDetail.value).not.toBeNull();
    mockStatus.mockResolvedValue(next);
    await checkEngineVersion();
    expect(frontendRefreshDetail.value).toBeNull();
  });

  it('keeps reporting it while an ENGINE build has failed', async () => {
    mockStatus.mockResolvedValue(
      status({ build_state: 'failed', frontend_refresh_elapsed_ms: 3_000 }),
    );
    await checkEngineVersion();
    expect(frontendRefreshDetail.value?.elapsedMs).toBe(3_000);
  });

  it('follows the re-read on a poke, never the event itself', async () => {
    mockStatus.mockResolvedValue(status({ frontend_refresh_elapsed_ms: 1_000 }));
    handleFrontendRefreshStateChanged();
    await Promise.resolve();
    await Promise.resolve();
    expect(mockStatus).toHaveBeenCalled();
    expect(frontendRefreshDetail.value?.elapsedMs).toBe(1_000);
  });
});

describe('handleFrontendUpdateStranded — the change is NOT coming', () => {
  const WORKTREE = '/w/dev/.lucidos/worktrees/thread-abc/crates/lucidos-app/dist';

  function strandedToasts() {
    return toasts.value.filter((t) => t.key === FRONTEND_UPDATE_STRANDED_TOAST_KEY);
  }

  beforeEach(() => {
    toasts.value = [];
  });

  it('warns (not info) — a silently-absent applied change is a broken state, not a queued one', () => {
    handleFrontendUpdateStranded({
      served_dir: WORKTREE, served_in_worktree: true, sent_at_ms: Date.now(),
    });
    const shown = strandedToasts();
    expect(shown).toHaveLength(1);
    expect(shown[0].type).toBe('warning');
  });

  it('never claims the change arrives on Switch — that is the deferred hint, and here it would be false', () => {
    handleFrontendUpdateStranded({
      served_dir: WORKTREE, served_in_worktree: true, sent_at_ms: Date.now(),
    });
    expect(strandedToasts()[0].message).not.toMatch(/switch/i);
  });

  it('uses its own key so it cannot coalesce with the deferred hint', () => {
    handleFrontendUpdateDeferred({ sent_at_ms: Date.now() });
    handleFrontendUpdateStranded({
      served_dir: WORKTREE, served_in_worktree: true, sent_at_ms: Date.now(),
    });
    expect(toasts.value.filter((t) => t.key === FRONTEND_UPDATE_DEFERRED_TOAST_KEY)).toHaveLength(1);
    expect(strandedToasts()).toHaveLength(1);
  });

  it('names the served path and, for a worktree, the corrective action', () => {
    handleFrontendUpdateStranded({
      served_dir: WORKTREE, served_in_worktree: true, sent_at_ms: Date.now(),
    });
    const msg = strandedToasts()[0].message;
    expect(msg).toContain(WORKTREE);
    expect(msg).toMatch(/real checkout/i);
  });

  it('points at the build-watch instead when the cause is not a worktree', () => {
    handleFrontendUpdateStranded({
      served_dir: '/Users/me/projects/lucidos/crates/lucidos-app/dist',
      served_in_worktree: false,
      sent_at_ms: Date.now(),
    });
    const msg = strandedToasts()[0].message;
    expect(msg).toMatch(/build-watch/i);
    expect(msg).not.toMatch(/worktree/i);
  });

  it('only claims permanence for the worktree case — a slow build is recoverable', () => {
    handleFrontendUpdateStranded({
      served_dir: '/Users/me/projects/lucidos/crates/lucidos-app/dist',
      served_in_worktree: false,
      sent_at_ms: Date.now(),
    });
    const recoverable = strandedToasts()[0].message;
    expect(recoverable).not.toMatch(/will not appear|never/i);
    expect(recoverable).toMatch(/yet|on its own/i);

    toasts.value = [];
    handleFrontendUpdateStranded({
      served_dir: WORKTREE, served_in_worktree: true, sent_at_ms: Date.now(),
    });
    expect(strandedToasts()[0].message).toMatch(/will not appear/i);
  });

  it('drops a stale event — the stack may already have been fixed', () => {
    handleFrontendUpdateStranded({
      served_dir: WORKTREE,
      served_in_worktree: true,
      sent_at_ms: Date.now() - DEFERRED_HINT_STALE_AFTER_MS - 1,
    });
    expect(strandedToasts()).toHaveLength(0);
  });

  it('the OK action dismisses it', () => {
    handleFrontendUpdateStranded({
      served_dir: WORKTREE, served_in_worktree: true, sent_at_ms: Date.now(),
    });
    strandedToasts()[0].action?.onClick();
    expect(strandedToasts()).toHaveLength(0);
  });
});

describe('strandedMessage: the reason, when the build-watch knows it', () => {
  const base = { served_dir: '/repo/crates/lucidos-app/dist', sent_at_ms: 0 };

  it('names the build failure instead of guessing', () => {
    const message = strandedMessage({
      ...base,
      served_in_worktree: false,
      build_error: 'Rollup failed to resolve import "jsqr"',
    });
    expect(message).toContain('the build is failing');
    expect(message).toContain('jsqr');
    expect(message).not.toContain('check the build-watch');
  });

  it('keeps the recoverable wording when no reason was reported', () => {
    const message = strandedMessage({ ...base, served_in_worktree: false });
    expect(message).toContain("hasn't rebuilt");
    expect(message).not.toContain('will not appear');
  });

  it('lets the worktree case keep its own permanent advice', () => {
    const message = strandedMessage({
      ...base,
      served_in_worktree: true,
      build_error: 'Rollup failed to resolve import "jsqr"',
    });
    expect(message).toContain('will not appear');
    expect(message).toContain('Relaunch the stack');
  });

  it('ignores a blank reason rather than showing an empty sentence', () => {
    const message = strandedMessage({ ...base, served_in_worktree: false, build_error: '  ' });
    expect(message).toContain("hasn't rebuilt");
  });
});

describe('strandedMessage: a stopped build-watch is not recoverable', () => {
  const base = { served_dir: '/repo/crates/lucidos-app/dist', sent_at_ms: 0 };

  it('says nothing is rebuilding, and never that it will appear on its own', () => {
    const message = strandedMessage({
      ...base,
      served_in_worktree: false,
      build_watch_stopped: true,
    });
    expect(message).toContain('nothing is rebuilding');
    expect(message).toContain('not running');
    expect(message).toContain('Relaunch the stack');
    expect(message).not.toContain('appear on its own');
  });

  it('outranks a build error, which a stopped watch will never retry', () => {
    const message = strandedMessage({
      ...base,
      served_in_worktree: false,
      build_watch_stopped: true,
      build_error: 'Rollup failed to resolve import "jsqr"',
    });
    expect(message).toContain('not running');
    expect(message).not.toContain('jsqr');
  });

  it('still yields to the worktree case, which no watch can fix', () => {
    const message = strandedMessage({
      ...base,
      served_in_worktree: true,
      build_watch_stopped: true,
    });
    expect(message).toContain('will not appear');
    expect(message).toContain('coding-agent worktree');
  });

  it('keeps the recoverable wording when the watch is running', () => {
    const message = strandedMessage({
      ...base,
      served_in_worktree: false,
      build_watch_stopped: false,
    });
    expect(message).toContain("hasn't rebuilt");
    expect(message).toContain('appear on its own');
  });

  it('treats an older engine that omits the field as no claim', () => {
    const message = strandedMessage({ ...base, served_in_worktree: false });
    expect(message).toContain("hasn't rebuilt");
    expect(message).not.toContain('undefined');
  });
});

it('opens build output inside the authenticated app', () => {
  engineBuildLogOpen.value = false;
  showEngineBuildLog();
  expect(engineBuildLogOpen.value).toBe(true);
  engineBuildLogOpen.value = false;
});



describe('version status invalidation', () => {
  it('invalidates on outside-build pokes and stream gaps', async () => {
    const { repairGapState } = await import('../eventGap');
    const before = engineVersionStatusVersion.value;
    handleEngineVersionStatusChanged();
    expect(engineVersionStatusVersion.value).toBe(before + 1);
    repairGapState();
    expect(engineVersionStatusVersion.value).toBe(before + 2);
  });
});
