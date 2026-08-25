import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../api/client', () => ({ engineVersionStatus: vi.fn(), rebuildEngine: vi.fn() }));
vi.mock('./chat-changes', () => ({ initiateEngineRestart: vi.fn() }));
// The engine-version dismissal is keyed on the ANNOUNCED VERSION id: the on-disk
// build when one is switchable, the checkout's HEAD when the version exists only
// in source. Mock it so each test controls what has been dismissed.
vi.mock('../../hooks/sw-update', () => ({
  noteAnnouncedEngineVersion: vi.fn(),
  wasEngineVersionDismissed: vi.fn(() => false),
  // store.ts (imported transitively by the store import below) pulls these from
  // the same module; markEngineVersionDismissed is exercised by the
  // dismiss-defer tests.
  markEngineVersionDismissed: vi.fn(),
  markSwUpdateDismissed: vi.fn(),
}));

import { checkEngineVersion, strandedMessage, openEngineVersionToast, resetEngineVersionToastForTest, handleFrontendUpdateDeferred, handleFrontendUpdateStranded, handleEngineBuildStateChanged, DEFERRED_HINT_STALE_AFTER_MS } from './engine-update';
import { engineVersionStatus, rebuildEngine } from '../../api/client';
// Type-only, so it is erased before the `vi.mock` above replaces that module.
import type { BuildFailure, PendingCommits } from '../../api/client';
import { noteAnnouncedEngineVersion, wasEngineVersionDismissed, markEngineVersionDismissed } from '../../hooks/sw-update';
import { toasts, engineVersionReady, engineVersionPending, engineRebuildWedged, engineBuilding, engineBuildDetail, engineRestarting, preferences, showToast, dismissToast, FRONTEND_UPDATE_DEFERRED_TOAST_KEY, FRONTEND_UPDATE_STRANDED_TOAST_KEY } from '../store';

const mockStatus = vi.mocked(engineVersionStatus);
const mockWasDismissed = vi.mocked(wasEngineVersionDismissed);
const mockNoteAnnounced = vi.mocked(noteAnnouncedEngineVersion);
const mockMarkDismissed = vi.mocked(markEngineVersionDismissed);
const mockRebuild = vi.mocked(rebuildEngine);

function status(over: Partial<{
  build_id: string;
  update_available: boolean;
  disk_build_id: string;
  packaged: boolean;
  build_state: 'idle' | 'building' | 'ready' | 'failed';
  source_behind_head: boolean;
  head_commit: string;
  rebuild_wedged: boolean;
  shared_build_in_progress: boolean;
  build_elapsed_ms: number;
  pending_commits: PendingCommits;
  build_failure: BuildFailure;
}> = {}) {
  return {
    build_id: 'eng123',
    update_available: false,
    disk_build_id: 'disk999',
    packaged: false,
    build_state: 'idle' as const,
    source_behind_head: false,
    head_commit: 'head777',
    rebuild_wedged: false,
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
    engineRebuildWedged.value = false;
    engineBuilding.value = false;
    engineRestarting.value = false;
    // The switch dismissal is a global preference, so checkEngineVersion skips
    // until preferences load. Seed loaded so the surface-behavior tests run; the
    // gated-while-loading case has its own test below.
    preferences.value = { status: 'loaded', data: {} };
  });

  it('sets the badge AND the Switch toast together when a newer build is ready', async () => {
    mockStatus.mockResolvedValue(status({ update_available: true, build_state: 'ready' }));
    await checkEngineVersion();
    expect(engineVersionReady.value).toBe(true); // badge
    const toast = toasts.value.find((t) => t.key === 'engine-new-version');
    expect(toast?.action?.label).toBe('Switch to new version'); // toast
    // "Later" is the explicit defer affordance (dismisses; badge stays lit).
    expect(toast?.secondaryAction?.label).toBe('Later');
    // Records the on-disk build so a later dismiss pins the right id.
    expect(mockNoteAnnounced).toHaveBeenCalledWith('disk999');
  });

  it('does not surface a new version while a build is still in progress, even if the on-disk binary already differs', async () => {
    mockStatus.mockResolvedValue(status({ update_available: true, build_state: 'building' }));
    await checkEngineVersion();
    expect(engineVersionReady.value).toBe(false);
    expect(hasSwitchToast()).toBe(false);
    // The spinning-build brand badge lights while the rebuild is in flight.
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
      action: { label: 'Switch to new version', onClick: () => {} },
    });
    expect(hasSwitchToast()).toBe(true);
    mockStatus.mockResolvedValue(status({ update_available: true, build_state: 'building' }));
    await checkEngineVersion();
    expect(engineVersionReady.value).toBe(false);
    expect(hasSwitchToast()).toBe(false);
  });

  it('keeps the badge lit but suppresses the toast for an on-disk build already dismissed', async () => {
    // Dismiss defers: the toast is gone for THIS build, but the badge stays lit
    // (build is ready) so the user can still switch from the reload badge.
    mockWasDismissed.mockImplementation((id) => id === 'disk999');
    mockStatus.mockResolvedValue(status({ update_available: true, build_state: 'ready', disk_build_id: 'disk999' }));
    await checkEngineVersion();
    expect(engineVersionReady.value).toBe(true); // badge persists
    expect(hasSwitchToast()).toBe(false); // toast deferred
  });

  it('dismissing defers: removes the Switch toast, keeps the badge lit, remembers the build', async () => {
    mockStatus.mockResolvedValue(status({ update_available: true, build_state: 'ready', disk_build_id: 'disk999' }));
    await checkEngineVersion();
    expect(hasSwitchToast()).toBe(true);
    expect(engineVersionReady.value).toBe(true);
    // User clicks X or "Later" → dismissToast('engine-new-version').
    dismissToast('engine-new-version');
    expect(hasSwitchToast()).toBe(false); // toast deferred away
    expect(engineVersionReady.value).toBe(true); // badge persists (update from badge)
    expect(mockMarkDismissed).toHaveBeenCalled(); // build remembered (durable)
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
    // A failed build clears the spinning-build badge too.
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
    // Packaged never runs a background build → the spinning badge is never lit.
    expect(engineBuilding.value).toBe(false);
  });

  it('does not poll while a switch is already in flight', async () => {
    engineRestarting.value = true;
    await checkEngineVersion();
    expect(mockStatus).not.toHaveBeenCalled();
  });

  it('skips entirely until preferences load (durable global dismissal not yet known)', async () => {
    // Before preferences load, the global switch-dismissal is unknown — surfacing
    // would flash an already-dismissed toast on cold start. Skip without even
    // fetching; useStartup re-runs checkEngineVersion after loadPreferences.
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
    // The 2026-07-26 loop: the ~10s self-heal driver cycles build_state
    // idle → building → ready, and a `ready` that changed nothing used to be
    // announced as a new version on its own — re-showing the toast every cycle
    // and offering a Switch that respawns onto the same binary. `ready` now
    // requires update_available (which the engine derives direction-honestly).
    mockStatus.mockResolvedValue(status({ update_available: false, build_state: 'ready' }));
    await checkEngineVersion();
    expect(hasSwitchToast()).toBe(false);
    expect(engineVersionReady.value).toBe(false);
  });

  it('keeps the Rebuild escape hatch when a finished rebuild left the version still pending', async () => {
    // Same wedge, but source IS behind: silence would hide a real pending
    // version AND its manual escape, so the pending branch admits 'ready'
    // (only 'building' suppresses it). INV-5b.
    mockStatus.mockResolvedValue(status({
      source_behind_head: true,
      update_available: false,
      build_state: 'ready',
      shared_build_in_progress: false,
    }));
    await checkEngineVersion();
    const toast = toasts.value.find((t) => t.key === 'engine-new-version');
    expect(toast?.action?.label).toBe('Rebuild');
    expect(engineVersionReady.value).toBe(false);
  });

  it('surfaces a pending-version toast with a Rebuild action when source is behind but no fresh binary exists', async () => {
    // The wedge: engine source is behind HEAD (a new version exists) but the
    // on-disk binary hasn't advanced (rebuild failed / not run). The Switch isn't
    // ready, so instead of silence the user gets an actionable "Rebuild".
    mockStatus.mockResolvedValue(status({ source_behind_head: true, update_available: false, build_state: 'idle' }));
    await checkEngineVersion();
    const toast = toasts.value.find((t) => t.key === 'engine-new-version');
    expect(toast?.action?.label).toBe('Rebuild');
    // Not a Switch — there's no built binary to switch onto yet.
    expect(toast?.action?.label).not.toBe('Switch to new version');
    expect(engineVersionReady.value).toBe(false);
  });

  it('pending-version Rebuild action triggers a background rebuild', async () => {
    mockStatus.mockResolvedValue(status({ source_behind_head: true, build_state: 'idle' }));
    await checkEngineVersion();
    toasts.value.find((t) => t.key === 'engine-new-version')?.action?.onClick();
    expect(mockRebuild).toHaveBeenCalled();
  });

  it('does NOT nag a Rebuild toast when a ready binary exists but its Switch was dismissed (source still behind)', async () => {
    // After a successful rebuild both update_available (disk≠running) AND
    // source_behind_head (running commit still behind HEAD) are true. Dismissing
    // the Switch must not re-surface as a "Rebuild" pending toast — a switchable
    // binary already exists; that's the ready branch's per-build dismissal, not here.
    mockWasDismissed.mockImplementation((id) => id === 'disk999');
    mockStatus.mockResolvedValue(status({ source_behind_head: true, update_available: true, build_state: 'ready', disk_build_id: 'disk999' }));
    await checkEngineVersion();
    expect(engineVersionReady.value).toBe(true); // badge persists (a binary IS ready)
    expect(hasSwitchToast()).toBe(false); // neither the Switch toast (dismissed) nor a Rebuild nag
  });

  it('does not surface the pending toast while the self-heal rebuild is building (spinner covers it)', async () => {
    mockStatus.mockResolvedValue(status({ source_behind_head: true, build_state: 'building' }));
    await checkEngineVersion();
    expect(hasSwitchToast()).toBe(false);
    expect(engineBuilding.value).toBe(true);
  });

  it('shows the building spinner (not a Rebuild toast) when a co-located peer is building the shared binary', async () => {
    // Multi-workspace: this workspace lost the shared build lock so its own
    // rebuild SkippedLocked → build_state fell back to 'idle', but a peer's build
    // IS in flight and will advance the shared binary. Show the spinner, withhold
    // the misleading manual "Rebuild".
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

  it('still shows the Rebuild toast (no spinner) when genuinely stuck — nothing is building', async () => {
    // The complement of the peer-building case: source behind, no fresh binary,
    // and NO build in flight (shared lock free) → genuinely stuck, so the manual
    // Rebuild escape hatch must still appear and the spinner must stay off.
    mockStatus.mockResolvedValue(status({
      source_behind_head: true,
      update_available: false,
      build_state: 'idle',
      shared_build_in_progress: false,
    }));
    await checkEngineVersion();
    const toast = toasts.value.find((t) => t.key === 'engine-new-version');
    expect(toast?.action?.label).toBe('Rebuild');
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

/** What a failed build TELLS the user.
 *
 *  The copy this replaced said "see the engine log" and offered Retry, which is
 *  two dead ends at once on a phone: the log is unreachable there, and for a
 *  deterministic failure Retry replays the same cached artifact forever. */
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
    engineRebuildWedged.value = false;
    engineBuilding.value = false;
    engineRestarting.value = false;
    preferences.value = { status: 'loaded', data: {} };
  });

  const failedWith = (build_failure?: BuildFailure) =>
    status({ source_behind_head: true, build_state: 'failed', build_failure });
  const failedToast = () => toasts.value.find((t) => t.key === 'engine-build-failed');

  it('names the actual error instead of pointing at a log', async () => {
    mockStatus.mockResolvedValue(failedWith({
      summary: 'error[E0308]: mismatched types',
      repeatable: false,
    }));
    await checkEngineVersion();
    expect(failedToast()?.message).toContain('error[E0308]: mismatched types');
    expect(failedToast()?.message).not.toContain('engine log');
  });

  it('withholds Retry when retrying is proved futile, and names the fix', async () => {
    // The reported loop: cargo calls the broken artifact fresh, so every tap
    // reruns it byte for byte. The button has to go, exactly as it does for a
    // wedged successful build.
    mockStatus.mockResolvedValue(failedWith({
      summary: 'Failed to create default VERSION: NotFound',
      remedy: 'cargo clean -p lucidos-engine',
      repeatable: true,
    }));
    await checkEngineVersion();
    const toast = failedToast();
    expect(toast?.type).toBe('warning');
    expect(toast?.action?.label).toBe('OK');
    expect(toast?.message).toContain('retrying cannot help');
    expect(toast?.message).toContain('cargo clean -p lucidos-engine');
    toast?.action?.onClick();
    expect(mockRebuild).not.toHaveBeenCalled();
  });

  it('OK actually puts the repeatable toast away, and the 4s poll leaves it away', async () => {
    // The button has to mean something. `showToast` recreates a keyed toast the
    // instant it is gone. So without an acknowledgement, OK on an
    // un-dismissable toast brought it straight back on the next poll, forever.
    mockStatus.mockResolvedValue(failedWith({ summary: 'stuck', repeatable: true }));
    await checkEngineVersion();
    failedToast()?.action?.onClick();
    expect(failedToast()).toBeUndefined();
    await checkEngineVersion();
    expect(failedToast()).toBeUndefined();
  });

  it('a DIFFERENT failure surfaces again after one was acknowledged', async () => {
    // The acknowledgement is keyed on the cause, so it silences that failure
    // and not the surface. A new thing going wrong is a new thing to say.
    mockStatus.mockResolvedValue(failedWith({ summary: 'stuck', repeatable: true }));
    await checkEngineVersion();
    failedToast()?.action?.onClick();
    mockStatus.mockResolvedValue(failedWith({ summary: 'stuck differently', repeatable: true }));
    await checkEngineVersion();
    expect(failedToast()?.message).toContain('stuck differently');
  });

  it('a build that starts or succeeds re-arms the acknowledged failure', async () => {
    // Clearing on the way out matters: the next failure is a new event even
    // when it reads identically, and the user has not seen THAT one.
    mockStatus.mockResolvedValue(failedWith({ summary: 'stuck', repeatable: true }));
    await checkEngineVersion();
    failedToast()?.action?.onClick();
    mockStatus.mockResolvedValue(status({ build_state: 'building' }));
    await checkEngineVersion();
    mockStatus.mockResolvedValue(failedWith({ summary: 'stuck', repeatable: true }));
    await checkEngineVersion();
    expect(failedToast()?.message).toContain('stuck');
  });

  it('still offers a way forward when there is no exact remedy to name', async () => {
    mockStatus.mockResolvedValue(failedWith({ summary: 'something deterministic', repeatable: true }));
    await checkEngineVersion();
    // A summary with no full stop must not run into the instruction after it.
    expect(failedToast()?.message).toContain('something deterministic. Ask a coding agent');
  });

  it('keeps Retry when the cause could not be read at all', async () => {
    // An unreadable failure is UNKNOWN, never "proved repeatable". Retiring the
    // button here would strand a build that the next attempt would have passed.
    mockStatus.mockResolvedValue(failedWith(undefined));
    await checkEngineVersion();
    const toast = failedToast();
    expect(toast?.type).toBe('error');
    expect(toast?.action?.label).toBe('Retry build');
    expect(toast?.message).toContain('could not read the build output');
  });

  it('an old engine that cannot describe its failure still gets a usable toast', async () => {
    // Cross-version: the frontend republishes seconds after Apply while the old
    // engine binary keeps serving until Switch, so it answers without the field.
    mockStatus.mockResolvedValue(status({ source_behind_head: true, build_state: 'failed' }));
    await checkEngineVersion();
    expect(failedToast()?.action?.label).toBe('Retry build');
  });
});

/** The pending version, given the shape every other version surface already
 *  has: a toast the user can put away, and a badge that stays lit after they do.
 *
 *  Before this it was the only one with neither. Its X called `removeToast` and
 *  the 4s poll drew it again, because there was no id to pin a dismissal to (a
 *  pending version has no on-disk build by definition). The engine now names the
 *  HEAD, so the dismissal has something to be about. */
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
    engineRebuildWedged.value = false;
    engineBuilding.value = false;
    engineRestarting.value = false;
    preferences.value = { status: 'loaded', data: {} };
  });

  it('lights the pending badge and pins the announcement to the checkout HEAD', async () => {
    mockStatus.mockResolvedValue(status({ ...PENDING, head_commit: 'head777' }));
    await checkEngineVersion();
    expect(engineVersionPending.value).toBe(true);
    expect(engineRebuildWedged.value).toBe(false);
    // The HEAD is the pending version's identity, standing in for the on-disk
    // build id the ready branch pins to.
    expect(mockNoteAnnounced).toHaveBeenCalledWith('head777');
  });

  it('offers a Later beside the Rebuild, like the Switch toast does', async () => {
    mockStatus.mockResolvedValue(status(PENDING));
    await checkEngineVersion();
    const toast = toasts.value.find((t) => t.key === 'engine-new-version');
    expect(toast?.action?.label).toBe('Rebuild');
    expect(toast?.secondaryAction?.label).toBe('Later');
  });

  /** The reported bug, and the one that matters: the X used to buy 4 seconds. */
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
    // ...and the badge is still lit, which is what makes putting it away safe.
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

    // The poll may UPDATE a toast on screen but never create one; without that
    // half, the re-opened toast would vanish again within 4s.
    await checkEngineVersion();
    expect(hasSwitchToast()).toBe(true);
  });

  it('the badge opens nothing when nothing is pending', () => {
    openEngineVersionToast();
    expect(hasSwitchToast()).toBe(false);
  });

  /** The dismissal is a workspace-GLOBAL preference on purpose, so putting the
   *  toast away on the phone must put it away on the laptop too. The re-open
   *  exemption is therefore keyed on a tap taken HERE, not on the toast merely
   *  being on screen, which every device with it up would satisfy. */
  it('a dismissal from another device closes a toast this one still has up', async () => {
    mockStatus.mockResolvedValue(status({ ...PENDING, head_commit: 'head777' }));
    await checkEngineVersion();
    expect(hasSwitchToast()).toBe(true);

    // The peer's dismiss arrives as a PreferencesChanged reload, not as a local
    // dismissToast, so nothing on this device removed the toast.
    mockWasDismissed.mockImplementation((id) => id === 'head777');
    await checkEngineVersion();
    expect(hasSwitchToast()).toBe(false);
    expect(engineVersionPending.value).toBe(true);
  });

  /** ...and the exemption retires with the toast, rather than making the
   *  re-opened one permanently immune to its own dismissal. */
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

  /** The second reported bug: Rebuild ran a few-second no-op build and the toast
   *  came straight back. The engine now says a rebuild for this HEAD already
   *  proved futile, so the button that loops is withheld. */
  it('a wedged rebuild withholds the button that loops and names the fix', async () => {
    mockStatus.mockResolvedValue(status({
      ...PENDING,
      build_state: 'ready',
      rebuild_wedged: true,
    }));
    await checkEngineVersion();
    const toast = toasts.value.find((t) => t.key === 'engine-new-version');
    expect(toast?.type).toBe('warning');
    expect(toast?.action?.label).not.toBe('Rebuild');
    expect(toast?.message).toMatch(/relaunch/i);
    expect(engineRebuildWedged.value).toBe(true);
  });

  it('a wedged toast is acknowledged with OK rather than a bare X', async () => {
    mockStatus.mockResolvedValue(status({ ...PENDING, build_state: 'ready', rebuild_wedged: true }));
    await checkEngineVersion();
    const toast = toasts.value.find((t) => t.key === 'engine-new-version');
    expect(toast?.dismissable).toBe(false);
    expect(toast?.action?.label).toBe('OK');
    toast?.action?.onClick();
    expect(hasSwitchToast()).toBe(false);
  });

  it('keeps Rebuild for a completed build that says nothing about this HEAD', async () => {
    // build_state 'ready' looks like the wedge and is not: only the engine knows
    // which HEAD that build was started from, so the verdict is its call. With
    // the flag false, the escape hatch stays offered.
    mockStatus.mockResolvedValue(status({ ...PENDING, build_state: 'ready', rebuild_wedged: false }));
    await checkEngineVersion();
    expect(toasts.value.find((t) => t.key === 'engine-new-version')?.action?.label).toBe('Rebuild');
    expect(engineRebuildWedged.value).toBe(false);
  });

  it.each([
    ['a build is in flight', status({ ...PENDING, build_state: 'building' })],
    ['a co-located peer is building', status({ ...PENDING, shared_build_in_progress: true })],
    ['a switchable binary landed', status({ source_behind_head: true, update_available: true, build_state: 'ready' })],
    ['nothing is pending at all', status({})],
    ['this is a packaged build', status({ ...PENDING, packaged: true })],
  ])('drops the pending badge once %s', async (_case, next) => {
    mockStatus.mockResolvedValue(status({ ...PENDING, rebuild_wedged: true, build_state: 'ready' }));
    await checkEngineVersion();
    expect(engineVersionPending.value).toBe(true);
    expect(engineRebuildWedged.value).toBe(true);

    mockStatus.mockResolvedValue(next);
    await checkEngineVersion();
    expect(engineVersionPending.value).toBe(false);
    // The wedged flag is written by the same helper, so it cannot outlive the
    // state it describes and tint a badge for a workspace that is merely busy.
    expect(engineRebuildWedged.value).toBe(false);
  });

  it('the pending badge stands down for a failed build, which owns its own toast and Retry', async () => {
    mockStatus.mockResolvedValue(status({ source_behind_head: true, build_state: 'failed' }));
    await checkEngineVersion();
    expect(engineVersionPending.value).toBe(false);
    expect(toasts.value.find((t) => t.key === 'engine-build-failed')?.action?.label).toBe('Retry build');
  });
});

/** `engineBuilding` and `engineBuildDetail` are written by one helper, because
 *  `pollEngineVersion` decides "not building" on three separate paths and two
 *  independent assignments is how a stale narration outlives its build. That
 *  already happened here once: a toast reading "Building new version" survived a
 *  build that had already failed. */
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
    // Anchored on the CLIENT clock at receipt, which is what lets the counter
    // advance between polls without comparing two machines' clocks.
    expect(engineBuildDetail.value?.anchoredAt).toBeLessThanOrEqual(Date.now());
  });

  /** The window this toast exists for IS the cross-version window: an Apply
   *  republishes `dist/` in seconds while the engine keeps serving the old
   *  binary until the Switch, so a new frontend routinely polls an engine that
   *  predates the grouped shape and answers `{ total, subjects }`. Reading
   *  `.length` off its absent `groups` threw inside the badge's render. */
  it('survives an engine that predates the grouped commit shape', async () => {
    mockStatus.mockResolvedValue(status({
      build_state: 'building',
      build_elapsed_ms: 42_000,
      // The pre-grouping wire shape, exactly as an older engine sends it.
      pending_commits: { total: 79, subjects: ["Merge branch 'main' into x"] } as unknown as PendingCommits,
    }));
    await checkEngineVersion();
    expect(engineBuilding.value).toBe(true);
    // The timer still runs; the commits are simply not described, which is what
    // the toast did before grouping existed.
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

  /** A co-located peer's build spins the badge, but the engine reports no
   *  elapsed for it (its own `build_state` is idle). The detail exists so the
   *  commit list still shows; the timer does not. */
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
    // Pops unsolicited → must not steal focus.
    expect(shown[0].noAutofocus).toBe(true);
  });

  it('offers an OK dismiss but NO Switch action — the deferral can fire mid-build, when switching is unsafe', () => {
    // The OK just acknowledges/dismisses; the Switch affordance stays with the
    // version-status poll's guarded toast/badge (checkEngineVersion), which
    // withholds it until build_state is ready.
    handleFrontendUpdateDeferred({ sent_at_ms: Date.now() });
    const shown = deferredToasts()[0];
    expect(shown.action?.label).toBe('OK');
    // No Switch/restart affordance (neither primary nor secondary).
    expect(shown.action?.label).not.toBe('Switch to new version');
    expect(shown.secondaryAction).toBeUndefined();
  });

  it('renders no close X — the OK is the sole dismiss (dismissable: false)', () => {
    handleFrontendUpdateDeferred({ sent_at_ms: Date.now() });
    expect(deferredToasts()[0].dismissable).toBe(false);
  });

  it('the OK action dismisses the sticky hint', () => {
    handleFrontendUpdateDeferred({ sent_at_ms: Date.now() });
    expect(deferredToasts()).toHaveLength(1);
    deferredToasts()[0].action?.onClick();
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
    // The handler is a poke — it re-runs checkEngineVersion (authoritative GET).
    await Promise.resolve();
    await Promise.resolve();
    expect(mockStatus).toHaveBeenCalled();
    expect(engineBuilding.value).toBe(true);
    expect(engineVersionReady.value).toBe(false);
  });

  it('does NOT spin on a stale poke once the authoritative status says the build finished (only spins for a real build)', async () => {
    // A late/duplicate EngineBuildStateChanged must never force a spin — the
    // spinner follows the authoritative build_state, not the event.
    engineBuilding.value = true;
    mockStatus.mockResolvedValue(status({ update_available: true, build_state: 'ready' }));
    handleEngineBuildStateChanged();
    await Promise.resolve();
    await Promise.resolve();
    expect(engineBuilding.value).toBe(false);
    expect(engineVersionReady.value).toBe(true);
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
    expect(shown[0].noAutofocus).toBe(true);
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
    // A build slower than the engine's wait, or a briefly-stopped watch, still
    // lands and the ~10s peer sync advances the snapshot on its own. Telling the
    // user their change will not appear would be false there.
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
    // The worktree case genuinely cannot self-heal, so it must say so.
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
    // The failure this replaced: the user was told to "check the build-watch"
    // while the answer sat in its log file.
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
    // A build slower than the wait still lands, so this must not claim the
    // change is lost.
    const message = strandedMessage({ ...base, served_in_worktree: false });
    expect(message).toContain("hasn't rebuilt");
    expect(message).not.toContain('will not appear');
  });

  it('lets the worktree case keep its own permanent advice', () => {
    // That one can never receive the rebuild, whatever the build says.
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
