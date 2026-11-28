import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('virtual:build-id', () => ({ CLIENT_BUILD_ID: 'client123' }));
vi.mock('../../hooks/sw-update', () => ({
  getServedBuildId: vi.fn(),
  refreshClient: vi.fn(),
  markSwUpdateDismissed: vi.fn(),
  wasSwUpdateDismissed: vi.fn(() => false),
  noteUpdateBuildId: vi.fn(),
  markEngineVersionDismissed: vi.fn(),
}));

import { syncClientUpdateFromBuild } from './client-update';
import { updateAvailable, toasts, showToast, engineRestarting, dismissToast, preferences } from '../store';
import { getServedBuildId, refreshClient, wasSwUpdateDismissed, noteUpdateBuildId, markSwUpdateDismissed } from '../../hooks/sw-update';

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
  preferences.value = { status: 'loaded', data: {} };
}

describe('syncClientUpdateFromBuild — badge', () => {
  beforeEach(reset);

  it('clears the badge when the served build matches the running build', async () => {
    updateAvailable.value = true;
    mockGetServedBuildId.mockResolvedValue('client123');
    expect(await syncClientUpdateFromBuild()).toBe(false);
    expect(updateAvailable.value).toBe(false);
  });

  it('lights the badge when the served build is newer than the running build', async () => {
    mockGetServedBuildId.mockResolvedValue('server999');
    expect(await syncClientUpdateFromBuild()).toBe(true);
    expect(updateAvailable.value).toBe(true);
  });

  it('leaves a lit badge untouched when the served build id is indeterminate', async () => {
    updateAvailable.value = true;
    mockGetServedBuildId.mockResolvedValue(null);
    await syncClientUpdateFromBuild();
    expect(updateAvailable.value).toBe(true);
  });

  it('leaves an unlit badge untouched when the served build id is indeterminate', async () => {
    mockGetServedBuildId.mockResolvedValue(null);
    await syncClientUpdateFromBuild();
    expect(updateAvailable.value).toBe(false);
  });

  it('skips entirely until preferences load (durable global dismissal not yet known)', async () => {
    preferences.value = { status: 'loading' };
    updateAvailable.value = true;
    mockGetServedBuildId.mockResolvedValue('server999');
    await syncClientUpdateFromBuild();
    expect(mockGetServedBuildId).not.toHaveBeenCalled();
    expect(hasUpdateToast()).toBe(false);
    expect(updateAvailable.value).toBe(true);
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
    mockWasDismissed.mockReturnValue(true);
    mockGetServedBuildId.mockResolvedValue('server999');
    await syncClientUpdateFromBuild();
    expect(updateAvailable.value).toBe(true);
    expect(hasUpdateToast()).toBe(false);
  });

  it('removes the toast (and clears the badge) when the client is no longer stale', async () => {
    mockGetServedBuildId.mockResolvedValue('server999');
    await syncClientUpdateFromBuild();
    expect(hasUpdateToast()).toBe(true);
    expect(updateAvailable.value).toBe(true);
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
    dismissToast(UPDATE_KEY);
    expect(hasUpdateToast()).toBe(false);
    expect(updateAvailable.value).toBe(true);
    expect(mockMarkDismissed).toHaveBeenCalled();
  });

  it('surfaces alongside the action-less "Engine restarted" confirmation', async () => {
    showToast('Engine restarted', 'success', { autoDismissMs: 5_000 });
    mockGetServedBuildId.mockResolvedValue('server999');
    await syncClientUpdateFromBuild();
    expect(hasUpdateToast()).toBe(true);
    expect(updateAvailable.value).toBe(true);
  });
});

describe('syncClientUpdateFromBuild: Refresh action', () => {
  beforeEach(() => {
    reset();
    vi.mocked(refreshClient).mockClear();
  });

  it('routes Refresh through refreshClient so the service worker is swapped', async () => {
    mockGetServedBuildId.mockResolvedValue('server999');
    await syncClientUpdateFromBuild();
    toasts.value.find((t) => t.key === UPDATE_KEY)?.action?.onClick?.();
    expect(refreshClient).toHaveBeenCalledTimes(1);
  });

  it('collapses the toast on Refresh without recording a deferral for the build', async () => {
    mockGetServedBuildId.mockResolvedValue('server999');
    await syncClientUpdateFromBuild();
    expect(hasUpdateToast()).toBe(true);
    toasts.value.find((t) => t.key === UPDATE_KEY)?.action?.onClick?.();
    expect(hasUpdateToast()).toBe(false);
    expect(mockMarkDismissed).not.toHaveBeenCalled();
  });
});
