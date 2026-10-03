import { describe, it, expect } from 'vitest';
import { archiveAllConfirmation, archiveAllResultMessage, idBatches, MAX_IDS_PER_REQUEST, undoAction } from './archive-all';
import { toastTap } from '../../components/shared/toastTap';
import type { ArchiveAllPreflight } from '../../api/threads';

function preflight(safe: number, kept: Record<string, number>): ArchiveAllPreflight {
  return {
    safe: Array.from({ length: safe }, (_, i) => ({ thread_id: `t${i}`, title: `Thread ${i}` })),
    kept,
    kept_count: Object.values(kept).reduce((a, b) => a + b, 0),
  };
}

describe('the Archive all confirm', () => {
  it('names what stays in Current, counted by reason', () => {
    const c = archiveAllConfirmation(preflight(42, { question: 12, pending_change: 4, draft: 2 }));
    expect(c?.title).toBe('Archive 42 threads?');
    expect(c?.message).toBe(
      '18 need you and stay in Current: 12 open questions, 4 unapplied changes, 2 unsent drafts. '
        + 'You can undo right after.',
    );
  });

  it('uses the singular for one of each', () => {
    const c = archiveAllConfirmation(preflight(1, { failed_run: 1 }));
    expect(c?.title).toBe('Archive 1 thread?');
    expect(c?.message).toBe('1 needs you and stays in Current: 1 failed run. You can undo right after.');
  });

  it('says everything goes when nothing needs the user', () => {
    expect(archiveAllConfirmation(preflight(3, {}))?.message).toBe(
      'Everything in Current goes to Archive. You can undo right after.',
    );
  });

  it('asks nothing when no thread is safe to archive', () => {
    expect(archiveAllConfirmation(preflight(0, { question: 5 }))).toBeNull();
  });
});

describe('the Archive all result', () => {
  it('reports the batch, and any thread that changed since the confirm', () => {
    expect(archiveAllResultMessage(5, 0)).toBe('Archived 5 threads.');
    expect(archiveAllResultMessage(1, 2)).toBe('Archived 1 thread. 2 changed since you confirmed and stayed.');
  });

  it('draws Undo as a button, so a tap on the toast does not undo', () => {
    expect(toastTap({ type: 'success', action: undoAction(['t1']) })).toBeNull();
  });
});

describe('the Archive all requests', () => {
  it('split a large Current into batches the engine accepts', () => {
    const ids = Array.from({ length: MAX_IDS_PER_REQUEST * 2 + 1 }, (_, i) => `t${i}`);
    const batches = idBatches(ids);
    expect(batches.map((b) => b.length)).toEqual([MAX_IDS_PER_REQUEST, MAX_IDS_PER_REQUEST, 1]);
    expect(batches.flat()).toEqual(ids);
    expect(idBatches([])).toEqual([]);
  });
});
