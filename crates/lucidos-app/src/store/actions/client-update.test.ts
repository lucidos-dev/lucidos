import { describe, it, expect, beforeEach, vi } from 'vitest';

// CLIENT_BUILD_ID is the build that produced the running code — pin it so each
// test varies only the served id.
vi.mock('virtual:build-id', () => ({ CLIENT_BUILD_ID: 'client123' }));
vi.mock('../../hooks/sw-update', () => ({
  getServedBuildId: vi.fn(),
  refreshClient: vi.fn(),
  markSwUpdateDismissed: vi.fn(),
  wasSwUpdateDismissed: vi.fn(() => false),
  noteUpdateBuildId: vi.fn(),
  // store.ts (imported transitively) pulls this from the same module.
  markEngineVersionDismissed: vi.fn(),
}));

import { syncClientUpdateFromBuild } from './client-update';
import { updateAvailable, toasts, showToast, engineRestarting, dismissToast, preferences } from '../store';
import { getServedBuildId, wasSwUpdateDismissed, noteUpdateBuildId, markSwUpdateDismissed } from '../../hooks/sw-update';

const mockGetServedBuildId = vi.mocked(getServedBuildId);
const mockWasDismissed = vi.mocked(wasSwUpdateDismissed);
const mockNoteBuildId = vi.mocked(noteUpdateBuildId);
const mockMarkDismissed = vi.mocked(markSwUpdateDismissed);

const UPDATE_KEY = 'update-available';
const hasUpdateToast = () => toasts.value.some((t) => t.key === UPDATE_KEY);

function reset() {
  mockGetServedBuildId.mockReset();
  mockWasDismissed.mockReset();
  mockWasDismissed.mockReturnValue(false);
  mockNoteBuildId.mockReset();
  mockMarkDismissed.mockReset();
  updateAvailable.value = false;
  toasts.value = [];
  engineRestarting.value = false;
  // The refresh dismissal is a global preference, so syncClientUpdateFromBuild
  // skips until preferences load. Seed loaded so the surface-behavior tests run;
  // the gated-while-loading case has its own test below.
  preferences.value = { status: 'loaded', data: {} };
}

describe('syncClientUpdateFromBuild — badge', () => {
  beforeEach(reset);

  it('clears the badge when the served build matches the running build', async () => {
    updateAvailable.value = true;
    mockGetServedBuildId.mockResolvedValue('client123');
    await syncClientUpdateFromBuild();
    expect(updateAvailable.value).toBe(false); // self-correcting: was true, now cleared
  });

  it('lights the badge when the served build is newer than the running build', async () => {
    mockGetServedBuildId.mockResolvedValue('server999');
    await syncClientUpdateFromBuild();
    expect(updateAvailable.value).toBe(true);
  });

  it('leaves a lit badge untouched when the served build id is indeterminate', async () => {
    updateAvailable.value = true;
    mockGetServedBuildId.mockResolvedValue(null); // offline / transient
    await syncClientUpdateFromBuild();
    expect(updateAvailable.value).toBe(true); // must not mis-clear on a failed check
  });

  it('leaves an unlit badge untouched when the served build id is indeterminate', async () => {
    mockGetServedBuildId.mockResolvedValue(null);
    await syncClientUpdateFromBuild();
    expect(updateAvailable.value).toBe(false);
  });

  it('skips entirely until preferences load (durable global dismissal not yet known)', async () => {
    // Before preferences load, the global refresh-dismissal is unknown — surfacing
    // would flash an already-dismissed toast on cold start. Skip without even
    // fetching the served build; useStartup re-runs this after loadPreferences.
    preferences.value = { status: 'loading' };
    updateAvailable.value = true;
    mockGetServedBuildId.mockResolvedValue('server999');
    await syncClientUpdateFromBuild();
    expect(mockGetServedBuildId).not.toHaveBeenCalled();
    expect(hasUpdateToast()).toBe(false);
    expect(updateAvailable.value).toBe(true); // left as-is, not mis-cleared
  });
});

describe('syncClientUpdateFromBuild — badge ⟺ toast (arrival coupled; dismiss defers)', () => {
  beforeEach(reset);

  it('sets the badge AND the Refresh toast together when the loaded build is stale', async () => {
    mockGetServedBuildId.mockResolvedValue('server999');
    await syncClientUpdateFromBuild();
    expect(updateAvailable.value).toBe(true);
    const toast = toasts.value.find((t) => t.key === UPDATE_KEY);
    expect(toast?.message).toBe('New version available — refresh to sync');
    expect(toast?.action?.label).toBe('Refresh');
    // "Later" is the explicit defer affordance (dismisses; badge stays lit).
    expect(toast?.secondaryAction?.label).toBe('Later');
  });

  it('leaves BOTH absent when the loaded build is current (fresh install)', async () => {
    mockGetServedBuildId.mockResolvedValue('client123');
    await syncClientUpdateFromBuild();
    expect(updateAvailable.value).toBe(false);
    expect(hasUpdateToast()).toBe(false);
  });

  it('records the served build id so a later dismiss can pin it', async () => {
    mockGetServedBuildId.mockResolvedValue('server999');
    await syncClientUpdateFromBuild();
    expect(mockNoteBuildId).toHaveBeenCalledWith('server999');
  });

  it('keeps the badge lit but suppresses the toast for a build already dismissed', async () => {
    // Dismiss defers: the toast is gone for THIS build, but the badge stays lit
    // (still stale) as the persistent refresh affordance — the user can still
    // refresh from the reload badge.
    mockWasDismissed.mockReturnValue(true);
    mockGetServedBuildId.mockResolvedValue('server999');
    await syncClientUpdateFromBuild();
    expect(updateAvailable.value).toBe(true); // badge persists
    expect(hasUpdateToast()).toBe(false); // toast deferred
  });

  it('removes the toast (and clears the badge) when the client is no longer stale', async () => {
    mockGetServedBuildId.mockResolvedValue('server999');
    await syncClientUpdateFromBuild();
    expect(hasUpdateToast()).toBe(true);
    expect(updateAvailable.value).toBe(true);
    // The running build now matches (e.g. a refresh landed) — both clear together.
    mockGetServedBuildId.mockResolvedValue('client123');
    await syncClientUpdateFromBuild();
    expect(hasUpdateToast()).toBe(false);
    expect(updateAvailable.value).toBe(false);
  });

  it('dismissing defers: removes the toast, keeps the badge lit, remembers the build', async () => {
    mockGetServedBuildId.mockResolvedValue('server999');
    await syncClientUpdateFromBuild();
    expect(hasUpdateToast()).toBe(true);
    expect(updateAvailable.value).toBe(true);
    // User clicks X or "Later" → dismissToast('update-available').
    dismissToast(UPDATE_KEY);
    expect(hasUpdateToast()).toBe(false); // toast deferred away
    expect(updateAvailable.value).toBe(true); // badge persists (update from badge)
    expect(mockMarkDismissed).toHaveBeenCalled(); // build remembered (durable)
  });

  it('surfaces alongside the action-less "Engine restarted" confirmation', async () => {
    // A restart that also rebuilt the client must still tell the user to refresh;
    // there is no longer a hasRefreshToast guard to hold it back.
    showToast('Engine restarted', 'success', { autoDismissMs: 5_000 });
    mockGetServedBuildId.mockResolvedValue('server999');
    await syncClientUpdateFromBuild();
    expect(hasUpdateToast()).toBe(true);
    expect(updateAvailable.value).toBe(true);
  });
});
