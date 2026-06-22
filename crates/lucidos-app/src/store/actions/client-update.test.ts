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
}));

import { syncClientUpdateFromBuild } from './client-update';
import { updateAvailable, toasts, showToast, engineRestarting } from '../store';
import { getServedBuildId, wasSwUpdateDismissed, noteUpdateBuildId } from '../../hooks/sw-update';

const mockGetServedBuildId = vi.mocked(getServedBuildId);
const mockWasDismissed = vi.mocked(wasSwUpdateDismissed);
const mockNoteBuildId = vi.mocked(noteUpdateBuildId);

describe('syncClientUpdateFromBuild — badge', () => {
  beforeEach(() => {
    mockGetServedBuildId.mockReset();
    mockWasDismissed.mockReset();
    mockWasDismissed.mockReturnValue(false);
    mockNoteBuildId.mockReset();
    updateAvailable.value = false;
    toasts.value = [];
    engineRestarting.value = false;
  });

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
});

describe('syncClientUpdateFromBuild — toast', () => {
  // The bug: the badge was lit by this reliable build-id check, but the "New
  // version available" toast was driven only by the fragile SW updatefound ->
  // activated event — so the badge could show with no toast. The toast must come
  // from the SAME signal as the badge, so the two can never disagree.
  beforeEach(() => {
    mockGetServedBuildId.mockReset();
    mockWasDismissed.mockReset();
    mockWasDismissed.mockReturnValue(false);
    mockNoteBuildId.mockReset();
    updateAvailable.value = false;
    toasts.value = [];
    engineRestarting.value = false;
  });

  it('shows the "New version available" Refresh toast when the loaded build is stale', async () => {
    mockGetServedBuildId.mockResolvedValue('server999');
    await syncClientUpdateFromBuild();
    const toast = toasts.value.find((t) => t.key === 'update-available');
    expect(toast).toBeTruthy();
    expect(toast?.message).toBe('New version available');
    expect(toast?.action?.label).toBe('Refresh');
  });

  it('does NOT show the toast when the loaded build is current (fresh install)', async () => {
    mockGetServedBuildId.mockResolvedValue('client123');
    await syncClientUpdateFromBuild();
    expect(toasts.value.some((t) => t.key === 'update-available')).toBe(false);
  });

  it('records the served build id so a later dismiss can pin it', async () => {
    mockGetServedBuildId.mockResolvedValue('server999');
    await syncClientUpdateFromBuild();
    expect(mockNoteBuildId).toHaveBeenCalledWith('server999');
  });

  it('lights the badge but suppresses the toast for a build already dismissed', async () => {
    mockWasDismissed.mockReturnValue(true);
    mockGetServedBuildId.mockResolvedValue('server999');
    await syncClientUpdateFromBuild();
    expect(updateAvailable.value).toBe(true); // update IS available — badge stays lit
    expect(toasts.value.some((t) => t.key === 'update-available')).toBe(false);
  });

  it('does not stack on top of an existing refresh/restart toast', async () => {
    // The "Engine restarted" reconnect toast already offers a Refresh action.
    showToast('Engine restarted', 'success', { action: { label: 'Refresh', onClick: () => {} } });
    mockGetServedBuildId.mockResolvedValue('server999');
    await syncClientUpdateFromBuild();
    expect(toasts.value.some((t) => t.key === 'update-available')).toBe(false);
    expect(updateAvailable.value).toBe(true); // badge still reflects the available update
  });
});
