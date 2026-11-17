import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { RestoreAdmissionPhase } from '../utils/restoreAdmission';

const postClientLog = vi.fn();
vi.mock('../utils/clientLog', () => ({ postClientLog }));

const { installRestoreAdmission } = await import('../utils/restoreAdmission');
const {
  RESTORE_ADMISSION_STALL_MS, watchRestoreAdmission, noteRestoreAdmitted, stopWatchingRestoreAdmission,
} = await import('./restoreAdmissionWatchdog');

let phase: RestoreAdmissionPhase = 'awaiting-hello';
installRestoreAdmission({ wait: async () => {}, ready: () => phase === 'admitted', phase: () => phase });

describe('the restore admission watchdog', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    postClientLog.mockClear();
    phase = 'awaiting-hello';
  });
  afterEach(() => {
    noteRestoreAdmitted();
    postClientLog.mockClear();
    vi.useRealTimers();
  });

  it('costs a healthy boot no request, no log line and no restart', async () => {
    const restart = vi.fn();
    watchRestoreAdmission(restart);
    await vi.advanceTimersByTimeAsync(RESTORE_ADMISSION_STALL_MS / 2);
    phase = 'admitted';
    noteRestoreAdmitted();
    await vi.advanceTimersByTimeAsync(RESTORE_ADMISSION_STALL_MS * 4);
    expect(restart).not.toHaveBeenCalled();
    expect(postClientLog).not.toHaveBeenCalled();
  });

  it('names the phase a stall hung in, once', async () => {
    const restart = vi.fn();
    phase = 'journal';
    watchRestoreAdmission(restart);
    await vi.advanceTimersByTimeAsync(RESTORE_ADMISSION_STALL_MS);
    expect(postClientLog).toHaveBeenCalledTimes(1);
    expect(postClientLog).toHaveBeenCalledWith('restore-admission', 'stalled',
      { phase: 'journal', waited_ms: RESTORE_ADMISSION_STALL_MS });
    // A step past the hello stalls in local storage, which a restart cannot reach.
    expect(restart).not.toHaveBeenCalled();
    watchRestoreAdmission(restart);
    await vi.advanceTimersByTimeAsync(RESTORE_ADMISSION_STALL_MS);
    expect(postClientLog).toHaveBeenCalledTimes(1);
  });

  it('restarts a stream whose hello has not come, once per connection epoch', async () => {
    const restart = vi.fn();
    watchRestoreAdmission(restart);
    await vi.advanceTimersByTimeAsync(RESTORE_ADMISSION_STALL_MS * 3);
    expect(restart).toHaveBeenCalledTimes(1);
    // The restart opens a new epoch, which earns one more restart.
    watchRestoreAdmission(restart);
    await vi.advanceTimersByTimeAsync(RESTORE_ADMISSION_STALL_MS * 3);
    expect(restart).toHaveBeenCalledTimes(2);
  });

  it('reports the total wait once admission opens after a stall', async () => {
    watchRestoreAdmission(vi.fn());
    await vi.advanceTimersByTimeAsync(RESTORE_ADMISSION_STALL_MS);
    watchRestoreAdmission(vi.fn());
    await vi.advanceTimersByTimeAsync(2_000);
    phase = 'admitted';
    noteRestoreAdmitted();
    expect(postClientLog).toHaveBeenLastCalledWith('restore-admission', 'admitted_after_stall',
      { waited_ms: RESTORE_ADMISSION_STALL_MS + 2_000 });
    noteRestoreAdmitted();
    expect(postClientLog).toHaveBeenCalledTimes(2);
  });

  it('stops counting toward a restart once the stream is gone', async () => {
    const restart = vi.fn();
    watchRestoreAdmission(restart);
    stopWatchingRestoreAdmission();
    await vi.advanceTimersByTimeAsync(RESTORE_ADMISSION_STALL_MS * 2);
    expect(restart).not.toHaveBeenCalled();
    expect(postClientLog).not.toHaveBeenCalled();
  });

  it('never fires for a page with no restore gate', async () => {
    phase = 'admitted';
    const restart = vi.fn();
    watchRestoreAdmission(restart);
    await vi.advanceTimersByTimeAsync(RESTORE_ADMISSION_STALL_MS * 2);
    expect(restart).not.toHaveBeenCalled();
    expect(postClientLog).not.toHaveBeenCalled();
  });
});
