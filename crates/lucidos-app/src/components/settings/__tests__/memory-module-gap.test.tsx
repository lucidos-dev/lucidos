// @vitest-environment jsdom
/** Settings → Memory: the backfill bar after an event gap.
 *
 *  The bar is fed by `TreeBackfill*` frames. A gap can swallow the one that
 *  ended the backfill, so the open page reads the snapshot again. */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { MemoryModuleSection } from '../MemoryModuleSection';
import { preferences } from '../../../store/store';
import { repairGapState } from '../../../store/eventGap';
import { NO_BACKFILL_PROGRESS, treeBackfill } from '../../../store/actions/treeBackfill';
import type { BackfillProgress } from '../../../api/types';

const mockGet = vi.hoisted(() => vi.fn());

vi.mock('../../../api/client', async (original) => ({
  ...(await original<typeof import('../../../api/client')>()),
  getTreeBackfill: mockGet,
}));

const BUILDING: BackfillProgress = { ...NO_BACKFILL_PROGRESS, done: 4, total: 5, done_milli: 4000, seq: 7 };

function label(host: HTMLElement): string | undefined {
  return host.querySelector('.progress-label')?.textContent ?? undefined;
}

describe('the Memory page after an event gap', () => {
  beforeEach(() => {
    mockGet.mockReset();
    treeBackfill.value = { status: 'not-loaded' };
    preferences.value = { status: 'loaded', data: { memory_module: 'tree' } };
  });

  it('reads the backfill again, so a lost Ready frame does not leave the bar at 80%', async () => {
    mockGet.mockResolvedValueOnce({ state: 'running', progress: BUILDING });
    const host = document.createElement('div');
    document.body.appendChild(host);
    await act(async () => {
      render(<MemoryModuleSection />, host);
    });
    await vi.waitFor(() => expect(label(host)).toBe('Building memory, 80%'));

    mockGet.mockResolvedValueOnce({ state: 'ready' });
    await act(async () => {
      repairGapState();
    });

    await vi.waitFor(() => expect(label(host)).toBe('Ready'));
    expect(mockGet).toHaveBeenCalledTimes(2);
  });
});
