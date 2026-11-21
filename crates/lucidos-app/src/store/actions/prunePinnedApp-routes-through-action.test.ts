// Contract: AppDeleted SSE must route the pinnedApps prune through the
// canonical writer in actions/pinnedApps.ts, not duplicate the signal +
// localStorage path.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { pinnedApps } from '../store';

vi.mock('./apps', () => ({ loadApps: vi.fn() }));
vi.mock('./triggers', () => ({ loadTriggers: vi.fn() }));
vi.mock('./artifacts', () => ({ loadArtifacts: vi.fn() }));

// Mock the canonical writer surface in pinnedApps.ts so we can detect that
// entityReferences delegates to it instead of duplicating the persistence path.
vi.mock('./pinnedApps', async () => {
  const actual = await vi.importActual<typeof import('./pinnedApps')>('./pinnedApps');
  return {
    ...actual,
    removePinnedAppLocal: vi.fn((appId: string) => {
      // Stub behaviour mirrors what the real writer must do — strip the entry
      // from both signal and localStorage — so the existing outcome assertions
      // (signal cleared, key updated) still hold while we observe the call.
      const current = pinnedApps.value;
      const data = current.status === 'loaded' ? current.data : [];
      const filtered = data.filter(e => e.app_id !== appId);
      pinnedApps.value = { status: 'loaded', data: filtered };
      localStorage.setItem('pinned_apps', JSON.stringify(filtered));
    }),
  };
});

import { processSSEForReferences } from './entityReferences';
import { removePinnedAppLocal } from './pinnedApps';

describe('AppDeleted SSE routes prune through pinnedApps action', () => {
  beforeEach(() => {
    localStorage.clear();
    pinnedApps.value = { status: 'loaded', data: [{ app_id: 'habit-tracker' }, { app_id: 'ledger' }] };
    vi.clearAllMocks();
  });

  it('calls removePinnedAppLocal once for the deleted app', () => {
    processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
    expect(removePinnedAppLocal).toHaveBeenCalledTimes(1);
    expect(removePinnedAppLocal).toHaveBeenCalledWith('habit-tracker');
  });

  it('still routes through removePinnedAppLocal when the deleted app was not pinned', () => {
    // The action owns the "is it actually pinned?" check — entityReferences
    // must not duplicate it.
    pinnedApps.value = { status: 'loaded', data: [{ app_id: 'other-app' }] };
    processSSEForReferences('AppDeleted', { app_id: 'habit-tracker' });
    expect(removePinnedAppLocal).toHaveBeenCalledTimes(1);
    expect(removePinnedAppLocal).toHaveBeenCalledWith('habit-tracker');
  });
});
