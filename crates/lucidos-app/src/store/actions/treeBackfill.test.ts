import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { TreeBackfill } from '../../api/types';
import { handleGlobalEvent } from './thread-sync';
import {
  NO_BACKFILL_PROGRESS,
  backfillDetail,
  backfillPercent,
  forgetTreeBackfillOrder,
  loadTreeBackfill,
  treeBackfill,
  treeBackfillStarted,
  treeBackfillVersion,
} from './treeBackfill';
import { summaryTreesVersion } from '../store';
import { repairGapState } from '../eventGap';

const mockGet = vi.fn();
vi.mock('../../api/client', async (original) => ({
  ...(await original<typeof import('../../api/client')>()),
  getTreeBackfill: () => mockGet(),
}));

const ZERO = NO_BACKFILL_PROGRESS;

const RUNNING: TreeBackfill = {
  state: 'running',
  progress: { ...ZERO, done: 3, total: 10, done_milli: 3_000 },
};

describe('the Tree backfill store', () => {
  beforeEach(() => {
    treeBackfill.value = { status: 'not-loaded' };
    forgetTreeBackfillOrder();
    mockGet.mockReset();
  });

  it('is told of an event gap, which can swallow the frame that ended the backfill', () => {
    const before = treeBackfillVersion.value;
    repairGapState();
    expect(treeBackfillVersion.value).toBe(before + 1);
  });

  it('loads the snapshot', async () => {
    mockGet.mockResolvedValue(RUNNING);
    await loadTreeBackfill();
    expect(treeBackfill.value).toEqual({ status: 'loaded', data: RUNNING });
  });

  it('moves on each SSE frame, through to ready and back to off', () => {
    handleGlobalEvent('TreeBackfillProgressed', { progress: RUNNING.progress });
    expect(treeBackfill.value).toEqual({ status: 'loaded', data: RUNNING });

    handleGlobalEvent('TreeBackfillCompleted', { total: 10 });
    expect(treeBackfill.value).toEqual({
      status: 'loaded',
      data: { state: 'ready', filling: { ...RUNNING.progress, ready: true } },
    });

    handleGlobalEvent('TreeBackfillReset', {});
    expect(treeBackfill.value).toEqual({ status: 'loaded', data: { state: 'off', started: true } });
  });

  it('reads a progress frame after ready as ready and filling', () => {
    const filling = { ...ZERO, done: 8, total: 10, done_milli: 8_000, ready: true };
    handleGlobalEvent('TreeBackfillProgressed', { progress: filling });
    expect(treeBackfill.value).toEqual({ status: 'loaded', data: { state: 'ready', filling } });
  });

  it('keeps the count when the ready frame lands after a filling frame', () => {
    const filling = { ...ZERO, done: 8, total: 10, done_milli: 8_000, ready: true };
    handleGlobalEvent('TreeBackfillProgressed', { progress: filling });
    handleGlobalEvent('TreeBackfillCompleted', { total: 10 });
    expect(treeBackfill.value).toEqual({ status: 'loaded', data: { state: 'ready', filling } });
  });

  it('keeps a frame that lands while the snapshot read is open', async () => {
    let resolve: (v: TreeBackfill) => void = () => {};
    mockGet.mockReturnValue(new Promise<TreeBackfill>((r) => { resolve = r; }));
    const inFlight = loadTreeBackfill();
    handleGlobalEvent('TreeBackfillCompleted', { total: 10 });
    resolve(RUNNING);
    await inFlight;
    expect(treeBackfill.value).toEqual({ status: 'loaded', data: { state: 'ready' } });
  });

  it('keeps a newer snapshot over an older read that lands after it', async () => {
    let resolveOlder: (v: TreeBackfill) => void = () => {};
    mockGet.mockReturnValueOnce(new Promise<TreeBackfill>((r) => { resolveOlder = r; }));
    const older = loadTreeBackfill();
    mockGet.mockResolvedValueOnce({ state: 'ready' });
    await loadTreeBackfill();
    resolveOlder(RUNNING);
    await older;
    expect(treeBackfill.value).toEqual({ status: 'loaded', data: { state: 'ready' } });
  });

  it('drops a frame the snapshot already counted', async () => {
    const at = (seq: number, done: number) => ({ ...ZERO, done, total: 2, done_milli: done * 1_000, seq });
    mockGet.mockResolvedValue({ state: 'running', progress: at(2, 1) });
    await loadTreeBackfill();

    handleGlobalEvent('TreeBackfillProgressed', { progress: at(1, 0) });
    handleGlobalEvent('TreeBackfillProgressed', { progress: at(2, 1) });
    expect(treeBackfill.value).toEqual({ status: 'loaded', data: { state: 'running', progress: at(2, 1) } });

    handleGlobalEvent('TreeBackfillProgressed', { progress: at(3, 2) });
    expect(treeBackfill.value).toEqual({ status: 'loaded', data: { state: 'running', progress: at(3, 2) } });
  });

  it('never steps the bar back to an older frame than the count shown', async () => {
    const at = (seq: number, done: number) => ({ ...ZERO, done, total: 2, done_milli: done * 1_000, seq });
    mockGet.mockResolvedValue({ state: 'running', progress: at(2, 1) });
    await loadTreeBackfill();

    handleGlobalEvent('TreeBackfillProgressed', { progress: at(1, 0) });
    expect(treeBackfill.value).toEqual({ status: 'loaded', data: { state: 'running', progress: at(2, 1) } });
  });

  it('takes no order from a snapshot read that started before the reset', async () => {
    let resolve: (v: TreeBackfill) => void = () => {};
    mockGet.mockReturnValue(new Promise<TreeBackfill>((r) => { resolve = r; }));
    const inFlight = loadTreeBackfill();
    forgetTreeBackfillOrder();
    resolve({ state: 'running', progress: { ...RUNNING.progress, seq: 40 } });
    await inFlight;

    const fresh = { ...RUNNING.progress, done: 4, seq: 1 };
    handleGlobalEvent('TreeBackfillProgressed', { progress: fresh });
    expect(treeBackfill.value).toEqual({ status: 'loaded', data: { state: 'running', progress: fresh } });
  });

  it('takes a low order again once the order is forgotten', () => {
    handleGlobalEvent('TreeBackfillProgressed', { progress: { ...RUNNING.progress, seq: 40 } });
    forgetTreeBackfillOrder();
    handleGlobalEvent('TreeBackfillProgressed', { progress: { ...RUNNING.progress, done: 4, seq: 1 } });
    expect(treeBackfill.value).toEqual({
      status: 'loaded',
      data: { state: 'running', progress: { ...RUNNING.progress, done: 4, seq: 1 } },
    });
  });

  it('shows a failed first read, but keeps a live state over a failed refresh', async () => {
    mockGet.mockRejectedValue(new Error('offline'));
    await loadTreeBackfill();
    expect(treeBackfill.value.status).toBe('failed');

    handleGlobalEvent('TreeBackfillProgressed', { progress: RUNNING.progress });
    await loadTreeBackfill();
    expect(treeBackfill.value).toEqual({ status: 'loaded', data: RUNNING });
  });

  it('reads zero percent until the trees are counted', () => {
    expect(backfillPercent(ZERO)).toBe(0);
    expect(backfillPercent({ ...ZERO, done: 1, total: 3, done_milli: 1_000 })).toBe(33);
    expect(backfillPercent({ ...ZERO, done: 3, total: 3, done_milli: 3_000 })).toBe(100);
  });

  it('counts the built share of the trees under way', () => {
    const lastTree = { ...ZERO, done: 14, total: 15, done_milli: 14_617, nodes_done: 210, nodes_total: 340 };
    expect(backfillPercent(lastTree)).toBe(97);
    expect(backfillDetail(lastTree)).toBe('14 of 15 trees · 210 of 340 summaries in progress');
    expect(backfillDetail({ ...lastTree, nodes_done: 0, nodes_total: 0 })).toBe('14 of 15 trees');
  });

  it('reads started from an off state, and from any live one', () => {
    expect(treeBackfillStarted()).toBe(false);
    treeBackfill.value = { status: 'loaded', data: { state: 'off', started: false } };
    expect(treeBackfillStarted()).toBe(false);
    treeBackfill.value = { status: 'loaded', data: { state: 'off', started: true } };
    expect(treeBackfillStarted()).toBe(true);
    treeBackfill.value = { status: 'loaded', data: RUNNING };
    expect(treeBackfillStarted()).toBe(true);
  });

  it('moves the summary tree browser on completion, reset and delete, never on progress', () => {
    const before = summaryTreesVersion.value;
    handleGlobalEvent('TreeBackfillProgressed', { progress: RUNNING.progress });
    expect(summaryTreesVersion.value).toBe(before);
    handleGlobalEvent('TreeBackfillCompleted', { total: 10 });
    handleGlobalEvent('TreeBackfillReset', {});
    expect(summaryTreesVersion.value).toBe(before + 2);
    handleGlobalEvent('ThreadsDeleted', { thread_ids: [] });
    expect(summaryTreesVersion.value).toBe(before + 3);
  });
});
