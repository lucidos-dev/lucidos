import { describe, it, expect, beforeEach, vi } from 'vitest';

// CLIENT_BUILD_ID is the build that produced the running code — pin it so each
// test varies only the served id.
vi.mock('virtual:build-id', () => ({ CLIENT_BUILD_ID: 'client123' }));
vi.mock('../../hooks/sw-update', () => ({ getServedBuildId: vi.fn() }));

import { syncClientUpdateFromBuild } from './client-update';
import { updateAvailable } from '../store';
import { getServedBuildId } from '../../hooks/sw-update';

const mockGetServedBuildId = vi.mocked(getServedBuildId);

describe('syncClientUpdateFromBuild', () => {
  beforeEach(() => {
    mockGetServedBuildId.mockReset();
    updateAvailable.value = false;
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
