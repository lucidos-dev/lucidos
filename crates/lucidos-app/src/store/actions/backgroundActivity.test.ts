import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  syncBackgroundActivityToast,
  openBackgroundActivityToast,
  applyEmbeddingModelStatus,
  loadEmbeddingModelStatus,
  resetBackgroundActivityToastForTest,
  beginTailscaleServeRun,
  clearTailscaleServeRun,
  applyTailscaleServeProgress,
  BACKGROUND_ACTIVITY_TOAST_KEY,
  SERVE_OUTCOME_TOAST_KEY,
} from './backgroundActivity';
import {
  toasts,
  dismissToast,
  engineBuilding,
  engineBuildDetail,
  embeddingModelStatus,
  engineRestarting,
  tailscaleServeRun,
} from '../store';
import type { EmbeddingModelLoadState } from '../../api/types';

// Hoisted by vitest, so the module under test sees the stub even though this
// sits below the imports.
const mockGetStatus = vi.fn();
vi.mock('../../api/client', () => ({
  getEmbeddingModelStatus: () => mockGetStatus(),
}));

function setModel(load_state: EmbeddingModelLoadState): void {
  embeddingModelStatus.value = { model_id: 'multilingual-e5-small', load_state };
}

function activityToast() {
  return toasts.value.find((t) => t.key === BACKGROUND_ACTIVITY_TOAST_KEY);
}

/** The Expose run's OUTCOME toast, which is a separate surface from the
 *  in-flight narration above. */
function outcomeToast() {
  return toasts.value.find((t) => t.key === SERVE_OUTCOME_TOAST_KEY);
}

function downloadFrame(downloaded: number): EmbeddingModelLoadState {
  return { kind: 'downloading', downloaded_bytes: downloaded, total_bytes: 1000 };
}

describe('background-activity toast', () => {
  beforeEach(() => {
    toasts.value = [];
    engineBuilding.value = false;
    engineBuildDetail.value = null;
    engineRestarting.value = false;
    embeddingModelStatus.value = null;
    resetBackgroundActivityToastForTest();
  });

  /** The auto-open the user asked for, expressed as "a real download started".
   *  A warm cache never enters that state, so this is what makes a fresh
   *  workspace announce itself without a first-run flag. */
  it('opens itself the first time a download is observed', () => {
    setModel(downloadFrame(100));
    syncBackgroundActivityToast();
    expect(activityToast()?.message).toContain('Downloading embedding model');
    expect(activityToast()?.progress).toBeCloseTo(0.1);
  });

  it('opens exactly once, then only updates in place', () => {
    setModel(downloadFrame(100));
    syncBackgroundActivityToast();
    const firstId = activityToast()?.id;

    setModel(downloadFrame(500));
    syncBackgroundActivityToast();
    expect(toasts.value.filter((t) => t.key === BACKGROUND_ACTIVITY_TOAST_KEY)).toHaveLength(1);
    expect(activityToast()?.id).toBe(firstId);
    expect(activityToast()?.progress).toBeCloseTo(0.5);
  });

  /** The failure mode this module exists to prevent: `showToast` with a key
   *  CREATES the toast when absent, so a bare per-frame call would pop the
   *  thing back up a few hundred milliseconds after every dismissal. */
  it('stays dismissed once the user closes it', () => {
    setModel(downloadFrame(100));
    syncBackgroundActivityToast();
    dismissToast(BACKGROUND_ACTIVITY_TOAST_KEY);
    expect(activityToast()).toBeUndefined();

    setModel(downloadFrame(600));
    syncBackgroundActivityToast();
    setModel(downloadFrame(900));
    syncBackgroundActivityToast();
    expect(activityToast()).toBeUndefined();
  });

  it('reopens on demand when the badge is tapped, and tracks again after that', () => {
    setModel(downloadFrame(100));
    syncBackgroundActivityToast();
    dismissToast(BACKGROUND_ACTIVITY_TOAST_KEY);

    openBackgroundActivityToast();
    expect(activityToast()).toBeDefined();

    setModel(downloadFrame(750));
    syncBackgroundActivityToast();
    expect(activityToast()?.progress).toBeCloseTo(0.75);
  });

  /** An existing workspace loads from a warm cache and never downloads, so it
   *  must stay completely silent. */
  it('never opens for a warm-cache boot', () => {
    setModel({ kind: 'loading' });
    syncBackgroundActivityToast();
    expect(activityToast()).toBeUndefined();

    setModel({ kind: 'ready' });
    syncBackgroundActivityToast();
    expect(activityToast()).toBeUndefined();
  });

  /** A rebuild is not unsolicited news the way a silently-disabled memory index
   *  is, and it already has the version toast. It may only update an open one. */
  it('never opens for an engine rebuild alone', () => {
    engineBuilding.value = true;
    syncBackgroundActivityToast();
    expect(activityToast()).toBeUndefined();

    openBackgroundActivityToast();
    expect(activityToast()?.message).toBe('Building new version');
  });

  it('resolves an open toast when the model lands', () => {
    setModel(downloadFrame(100));
    syncBackgroundActivityToast();

    setModel({ kind: 'ready' });
    syncBackgroundActivityToast();
    expect(activityToast()?.message).toContain('ready');
    expect(activityToast()?.type).toBe('success');
    // Settled means no spinner: the operation is over.
    expect(activityToast()?.spinning).toBe(false);
  });

  it('reports a terminal failure into an open toast instead of freezing mid-download', () => {
    setModel(downloadFrame(430));
    syncBackgroundActivityToast();

    setModel({ kind: 'failed', message: 'the cache is corrupt' });
    syncBackgroundActivityToast();
    expect(activityToast()?.message).toContain('the cache is corrupt');
  });

  it('clears the toast when there is nothing left to say', () => {
    setModel(downloadFrame(100));
    syncBackgroundActivityToast();
    expect(activityToast()).toBeDefined();

    embeddingModelStatus.value = null;
    syncBackgroundActivityToast();
    expect(activityToast()).toBeUndefined();
  });
});

describe('snapshot vs live-frame freshness', () => {
  beforeEach(() => {
    toasts.value = [];
    engineBuilding.value = false;
    engineRestarting.value = false;
    embeddingModelStatus.value = null;
    resetBackgroundActivityToastForTest();
    mockGetStatus.mockReset();
  });

  /** The regression this counter exists for. The snapshot read and the SSE
   *  stream race, and the loader emits its terminal `ready` frame and then
   *  RETURNS, so there is no later frame to undo a stale write: a `downloading`
   *  body resolving after `ready` would spin the badge for the rest of the
   *  session and hold the toast at whatever percentage it had reached. */
  it('discards a snapshot that a live frame overtook', async () => {
    let resolveRead: (v: unknown) => void = () => {};
    mockGetStatus.mockImplementation(
      () => new Promise((resolve) => { resolveRead = resolve; }),
    );

    const inFlight = loadEmbeddingModelStatus();

    // The download finishes over SSE while the HTTP read is still open.
    applyEmbeddingModelStatus({
      model_id: 'multilingual-e5-small',
      load_state: { kind: 'ready' },
    });

    // ...and only THEN does the stale body land.
    resolveRead({
      model_id: 'multilingual-e5-small',
      load_state: { kind: 'downloading', downloaded_bytes: 400, total_bytes: 1000 },
    });
    await inFlight;

    expect(embeddingModelStatus.value?.load_state).toEqual({ kind: 'ready' });
  });

  it('applies a snapshot that nothing overtook', async () => {
    mockGetStatus.mockResolvedValue({
      model_id: 'multilingual-e5-small',
      load_state: { kind: 'downloading', downloaded_bytes: 400, total_bytes: 1000 },
    });

    await loadEmbeddingModelStatus();

    expect(embeddingModelStatus.value?.load_state).toEqual({
      kind: 'downloading',
      downloaded_bytes: 400,
      total_bytes: 1000,
    });
    // And it still drives the auto-open, which is the whole reason a client that
    // connected mid-download reads the snapshot at all.
    expect(activityToast()?.message).toContain('Downloading embedding model');
  });

  /** A failed read must not disturb the state a live frame already established:
   *  it is an unsolicited probe, and SSE remains the newer truth. */
  it('leaves the live state alone when the read fails', async () => {
    applyEmbeddingModelStatus({
      model_id: 'multilingual-e5-small',
      load_state: { kind: 'ready' },
    });
    mockGetStatus.mockRejectedValue(new Error('offline'));

    await loadEmbeddingModelStatus();

    expect(embeddingModelStatus.value?.load_state).toEqual({ kind: 'ready' });
  });
});

describe('auto-open one-shot', () => {
  beforeEach(() => {
    toasts.value = [];
    engineBuilding.value = false;
    engineBuildDetail.value = null;
    engineRestarting.value = false;
    embeddingModelStatus.value = null;
    resetBackgroundActivityToastForTest();
  });

  /** `showToast` drops everything while the engine restarts. Spending the single
   *  auto-open on a call that rendered nothing would leave the document with no
   *  announcement at all, so the one-shot is only burned once the toast is
   *  actually on screen. */
  it('is not spent on a render the restart overlay swallowed', () => {
    engineRestarting.value = true;
    setModel(downloadFrame(100));
    syncBackgroundActivityToast();
    expect(activityToast()).toBeUndefined();

    engineRestarting.value = false;
    setModel(downloadFrame(300));
    syncBackgroundActivityToast();
    expect(activityToast()?.message).toContain('Downloading embedding model');
  });
});

describe('engine-build state reaches an open toast on every exit path', () => {
  beforeEach(() => {
    toasts.value = [];
    engineRestarting.value = false;
    embeddingModelStatus.value = null;
    resetBackgroundActivityToastForTest();
  });

  /** `checkEngineVersion` clears `engineBuilding` on several EARLY returns
   *  (packaged, build failed). Syncing only at its happy-path end left an open
   *  toast reading "Building new version" after the build had already failed,
   *  with nothing to correct it on a workspace whose model is long since cached.
   *  This pins the toast side of that contract: whatever moves the flag, the
   *  open toast follows. */
  it('drops a finished build out of the open toast', () => {
    engineBuilding.value = true;
    openBackgroundActivityToast();
    expect(activityToast()?.message).toBe('Building new version');

    engineBuilding.value = false;
    syncBackgroundActivityToast();
    expect(activityToast()).toBeUndefined();
  });

  /** The reported bug, as the user hit it on 2026-08-03. On a warm-cache
   *  workspace the model is simply `ready` and always was, so a toast opened on
   *  the spinning badge to watch a rebuild must CLEAR when the rebuild ends.
   *  It used to resolve into "Embedding model ready. Everything you create from
   *  now on is searchable in memory." right after a Switch to new version:
   *  a claim about work this document never narrated, in a toast opened to read
   *  about something else. */
  it('does not resolve a build toast into an unrelated embedding-model claim', () => {
    setModel({ kind: 'ready' });
    engineBuilding.value = true;
    openBackgroundActivityToast();
    expect(activityToast()?.message).toBe('Building new version');

    // The switch lands: the build is over and the new engine re-reports the
    // same warm-cache `ready` it booted with.
    engineBuilding.value = false;
    applyEmbeddingModelStatus({
      model_id: 'multilingual-e5-small',
      load_state: { kind: 'ready' },
    });
    expect(activityToast()).toBeUndefined();
  });

  /** The other half of the same rule: a download this document DID watch still
   *  gets its resolution, even though the toast on screen is now also carrying
   *  a build. */
  it('still resolves a download it narrated, build or no build', () => {
    setModel({ kind: 'downloading', downloaded_bytes: 100, total_bytes: 1000 });
    syncBackgroundActivityToast();
    engineBuilding.value = true;
    syncBackgroundActivityToast();

    engineBuilding.value = false;
    setModel({ kind: 'ready' });
    syncBackgroundActivityToast();
    expect(activityToast()?.message).toContain('ready');
    expect(activityToast()?.type).toBe('success');
  });
});

describe('the Expose run through the shared toast', () => {
  const APPROVAL_URL = 'https://login.tailscale.com/f/serve?node=nodeidEXAMPLE1234';

  beforeEach(() => {
    toasts.value = [];
    engineBuilding.value = false;
    engineRestarting.value = false;
    embeddingModelStatus.value = null;
    tailscaleServeRun.value = null;
    resetBackgroundActivityToastForTest();
  });

  /** The one place this run differs from the embedding-model download beside
   *  it. That download is unsolicited news, so it announces itself once per
   *  document; this is a button the user just pressed, so it narrates every
   *  time. Pressing Expose twice in a session must not go silent the second
   *  time. */
  it('opens its toast on every run, not once per document', () => {
    beginTailscaleServeRun();
    expect(activityToast()?.message).toContain('Setting up Tailscale access');

    applyTailscaleServeProgress({ phase: 'done', url: 'https://mymac.tailnet-name.ts.net' });
    dismissToast(BACKGROUND_ACTIVITY_TOAST_KEY);

    beginTailscaleServeRun();
    expect(activityToast()?.message).toContain('Setting up Tailscale access');
  });

  it('narrates each step in place, without stacking toasts', () => {
    beginTailscaleServeRun();
    const firstId = activityToast()?.id;

    applyTailscaleServeProgress({ phase: 'configuring' });
    expect(activityToast()?.message).toContain('Configuring tailscale serve');

    applyTailscaleServeProgress({ phase: 'waiting-for-https' });
    expect(activityToast()?.message).toContain('Waiting for HTTPS');

    expect(toasts.value.filter((t) => t.key === BACKGROUND_ACTIVITY_TOAST_KEY)).toHaveLength(1);
    expect(activityToast()?.id).toBe(firstId);
    // Indeterminate throughout: a spinner, never a fabricated bar.
    expect(activityToast()?.spinning).toBe(true);
    expect(activityToast()?.progress).toBeNull();
  });

  /** The reported failure, as it should now read: the link the CLI printed,
   *  offered as a button, with a way out of the wait. */
  it('turns the tailnet-approval step into a real button', () => {
    beginTailscaleServeRun();
    applyTailscaleServeProgress({ phase: 'awaiting-tailnet-approval', url: APPROVAL_URL });
    expect(activityToast()?.action?.label).toBe('Enable in Tailscale');
    expect(activityToast()?.secondaryAction?.label).toBe('Cancel');
    expect(activityToast()?.spinning).toBe(true);
  });

  it('reports the address on success, and lets the badge stop', () => {
    beginTailscaleServeRun();
    applyTailscaleServeProgress({ phase: 'done', url: 'https://mymac.tailnet-name.ts.net' });
    expect(outcomeToast()?.message).toContain('https://mymac.tailnet-name.ts.net');
    expect(outcomeToast()?.type).toBe('success');
    expect(tailscaleServeRun.value).toBeNull();
    // The in-flight narration is over, so it goes.
    expect(activityToast()).toBeUndefined();
  });

  /** A failure reaches the user even if they had closed the narration, because
   *  it is the outcome of something they pressed. */
  it('reports a failure even after the narration was closed', () => {
    beginTailscaleServeRun();
    dismissToast(BACKGROUND_ACTIVITY_TOAST_KEY);

    applyTailscaleServeProgress({ phase: 'failed', message: 'no MagicDNS name' });
    expect(outcomeToast()?.message).toBe('no MagicDNS name');
    expect(outcomeToast()?.type).toBe('error');
    expect(tailscaleServeRun.value).toBeNull();
  });

  it('clears everything on a cancel, with nothing to read', () => {
    beginTailscaleServeRun();
    applyTailscaleServeProgress({ phase: 'cancelled' });
    expect(activityToast()).toBeUndefined();
    expect(outcomeToast()).toBeUndefined();
    expect(tailscaleServeRun.value).toBeNull();
  });

  /** Both halves of the bug that routing the outcome through the SHARED toast
   *  caused. That toast only reaches its terminal branch when nothing is in
   *  flight, so a concurrent download swallowed the outcome entirely, and a
   *  cancel took the download's own narration down with it. */
  it('reports its outcome without disturbing a concurrent download', () => {
    setModel(downloadFrame(100));
    syncBackgroundActivityToast();
    beginTailscaleServeRun();

    applyTailscaleServeProgress({ phase: 'failed', message: 'no MagicDNS name' });
    // The failure reached the user...
    expect(outcomeToast()?.message).toBe('no MagicDNS name');
    // ...and the download is still being narrated.
    expect(activityToast()?.message).toContain('Downloading embedding model');
    expect(activityToast()?.spinning).toBe(true);
  });

  it('leaves a concurrent download narrating when the run is cancelled', () => {
    setModel(downloadFrame(100));
    syncBackgroundActivityToast();
    beginTailscaleServeRun();
    // Both are in flight, so the toast lists them.
    expect(activityToast()?.message).toContain('• Downloading embedding model');
    expect(activityToast()?.message).toContain('• Setting up Tailscale access');

    applyTailscaleServeProgress({ phase: 'cancelled' });
    expect(activityToast()?.message).toContain('Downloading embedding model');
    expect(outcomeToast()).toBeUndefined();
  });

  /** Same dismissal rule as the download: a toast the user closed mid-run is
   *  not resurrected by later frames. The still-spinning badge is how they get
   *  it back. */
  it('stays dismissed for mid-run frames after the user closes it', () => {
    beginTailscaleServeRun();
    dismissToast(BACKGROUND_ACTIVITY_TOAST_KEY);

    applyTailscaleServeProgress({ phase: 'configuring' });
    applyTailscaleServeProgress({ phase: 'awaiting-tailnet-approval', url: APPROVAL_URL });
    expect(activityToast()).toBeUndefined();
    // But the run is still live, so the badge is still spinning and a tap on it
    // brings the narration straight back.
    expect(tailscaleServeRun.value?.phase).toBe('awaiting-tailnet-approval');
    openBackgroundActivityToast();
    expect(activityToast()?.action?.label).toBe('Enable in Tailscale');
  });

  /** For what Rust could not report at all: a rejected invoke, an ACL denial, a
   *  dead bridge. The page shows the error; this only has to stop the badge. */
  it('clears the run without narrating when no frame ever arrived', () => {
    beginTailscaleServeRun();
    clearTailscaleServeRun();
    expect(tailscaleServeRun.value).toBeNull();
    expect(activityToast()).toBeUndefined();
  });
});

/** The build timer is the one number here that nothing pushes: the engine emits
 *  only build-state transitions, so between the ~4s version-status polls the
 *  seconds have to be counted locally. A counter that only moved every 4s (or
 *  not at all) is what "the time should update in the toast, not stand still"
 *  was about. */
describe('background-activity toast: the build timer ticks', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    toasts.value = [];
    engineBuilding.value = false;
    engineBuildDetail.value = null;
    engineRestarting.value = false;
    embeddingModelStatus.value = null;
    tailscaleServeRun.value = null;
    resetBackgroundActivityToastForTest();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  /** Start a build whose age the engine has just reported. `anchoredAt` is the
   *  client clock, which the fake timers control, so advancing them advances the
   *  counter exactly as wall time would. */
  function startBuild(elapsedMs: number | null = 8_000): void {
    engineBuilding.value = true;
    engineBuildDetail.value = { elapsedMs, anchoredAt: Date.now(), pendingCommits: null };
  }

  it('advances the counter every second with no new poll', () => {
    startBuild();
    openBackgroundActivityToast();
    expect(activityToast()?.message).toBe('Building new version, 8s');

    vi.advanceTimersByTime(1000);
    expect(activityToast()?.message).toBe('Building new version, 9s');
    vi.advanceTimersByTime(3000);
    expect(activityToast()?.message).toBe('Building new version, 12s');
  });

  /** The rule every update path in this module obeys, and the one the ticker
   *  could most easily break: `render` CREATES the toast when it is absent, so a
   *  tick that skipped the open check would pop a dismissed toast back up once a
   *  second for the rest of the build. */
  it('never resurrects a toast the user dismissed', () => {
    startBuild();
    openBackgroundActivityToast();
    dismissToast(BACKGROUND_ACTIVITY_TOAST_KEY);

    vi.advanceTimersByTime(10_000);
    expect(activityToast()).toBeUndefined();
  });

  it('stops ticking once the build ends', () => {
    startBuild();
    openBackgroundActivityToast();
    vi.advanceTimersByTime(2000);
    expect(activityToast()?.message).toBe('Building new version, 10s');

    // The poll's single writer clears both halves together.
    engineBuilding.value = false;
    engineBuildDetail.value = null;
    syncBackgroundActivityToast();
    expect(activityToast()).toBeUndefined();

    // And nothing is left running to bring it back.
    vi.advanceTimersByTime(10_000);
    expect(activityToast()).toBeUndefined();
    expect(vi.getTimerCount()).toBe(0);
  });

  /** A co-located peer's build has no elapsed of ours to count, so there is
   *  nothing to tick and no timer is armed. */
  it('arms no timer for a build with no elapsed time', () => {
    startBuild(null);
    openBackgroundActivityToast();
    expect(activityToast()?.message).toBe('Building new version');
    expect(vi.getTimerCount()).toBe(0);
  });

  /** A build running behind a CLOSED toast must not tick either: there is
   *  nothing on screen to update, and the badge is how the user asks for it. */
  it('arms no timer while the toast is closed', () => {
    startBuild();
    syncBackgroundActivityToast();
    expect(activityToast()).toBeUndefined();
    expect(vi.getTimerCount()).toBe(0);
  });
});
