import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../api/client', () => ({ engineVersionStatus: vi.fn(), rebuildEngine: vi.fn() }));
vi.mock('./chat-changes', () => ({ initiateEngineRestart: vi.fn() }));
// Engine-switch dismissal is keyed on the on-disk build id (INV-C). Mock it so
// each test controls whether THIS build was dismissed.
vi.mock('../../hooks/sw-update', () => ({
  noteSwitchBuildId: vi.fn(),
  wasSwitchDismissed: vi.fn(() => false),
  // store.ts (imported transitively by the store import below) pulls these from
  // the same module; markSwitchDismissed is exercised by the dismiss-defer test.
  markSwitchDismissed: vi.fn(),
  markSwUpdateDismissed: vi.fn(),
}));

import { checkEngineVersion, handleFrontendUpdateDeferred, handleFrontendUpdateStranded, handleEngineBuildStateChanged, DEFERRED_HINT_STALE_AFTER_MS } from './engine-update';
import { engineVersionStatus, rebuildEngine } from '../../api/client';
import { noteSwitchBuildId, wasSwitchDismissed, markSwitchDismissed } from '../../hooks/sw-update';
import { toasts, engineVersionReady, engineBuilding, engineRestarting, preferences, showToast, dismissToast, FRONTEND_UPDATE_DEFERRED_TOAST_KEY, FRONTEND_UPDATE_STRANDED_TOAST_KEY } from '../store';

const mockStatus = vi.mocked(engineVersionStatus);
const mockWasSwitchDismissed = vi.mocked(wasSwitchDismissed);
const mockNoteSwitchBuildId = vi.mocked(noteSwitchBuildId);
const mockMarkSwitchDismissed = vi.mocked(markSwitchDismissed);
const mockRebuild = vi.mocked(rebuildEngine);

function status(over: Partial<{
  build_id: string;
  update_available: boolean;
  disk_build_id: string;
  packaged: boolean;
  build_state: 'idle' | 'building' | 'ready' | 'failed';
  source_behind_head: boolean;
  shared_build_in_progress: boolean;
}> = {}) {
  return {
    build_id: 'eng123',
    update_available: false,
    disk_build_id: 'disk999',
    packaged: false,
    build_state: 'idle' as const,
    source_behind_head: false,
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
    mockWasSwitchDismissed.mockReset();
    mockWasSwitchDismissed.mockReturnValue(false);
    mockNoteSwitchBuildId.mockReset();
    mockMarkSwitchDismissed.mockReset();
    mockRebuild.mockReset();
    mockRebuild.mockResolvedValue(undefined);
    toasts.value = [];
    engineVersionReady.value = false;
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
    expect(mockNoteSwitchBuildId).toHaveBeenCalledWith('disk999');
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
    mockWasSwitchDismissed.mockImplementation((id) => id === 'disk999');
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
    expect(mockMarkSwitchDismissed).toHaveBeenCalled(); // build remembered (durable)
  });

  it('re-surfaces badge + toast for a genuinely newer on-disk build after a prior dismiss', async () => {
    mockWasSwitchDismissed.mockImplementation((id) => id === 'disk-old');
    mockStatus.mockResolvedValue(status({ update_available: true, build_state: 'ready', disk_build_id: 'disk-new' }));
    await checkEngineVersion();
    expect(engineVersionReady.value).toBe(true);
    expect(hasSwitchToast()).toBe(true);
    expect(mockNoteSwitchBuildId).toHaveBeenCalledWith('disk-new');
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
    mockWasSwitchDismissed.mockImplementation((id) => id === 'disk999');
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
    mockWasSwitchDismissed.mockReset();
    mockWasSwitchDismissed.mockReturnValue(false);
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
