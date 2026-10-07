import { describe, it, expect } from 'vitest';
import { archiveAllConfirmation, archiveAllResultMessage, idBatches, nothingToArchiveMessage, undoAction } from './archive-all';
import { BLOCKER_REASON } from './blockerCopy';
import { ARCHIVE_ALL_MAX_IDS } from '@lucidos/engine-constants';
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
      '18 need you and stay in Current: 12 waiting for your answer, '
        + '4 with a change to apply or discard, 2 with unsent drafts. You can undo right after.',
    );
  });

  it('uses the singular for one of each', () => {
    const c = archiveAllConfirmation(preflight(1, { failed_run: 1 }));
    expect(c?.title).toBe('Archive 1 thread?');
    expect(c?.message).toBe('1 needs you and stays in Current: 1 whose last run failed. You can undo right after.');
  });

  it('says everything goes when nothing needs the user', () => {
    expect(archiveAllConfirmation(preflight(3, {}))?.message).toBe(
      'Everything in Current goes to Archive. You can undo right after.',
    );
  });

  it('asks nothing when no thread is safe to archive', () => {
    expect(archiveAllConfirmation(preflight(0, { question: 5 }))).toBeNull();
  });

  it("words a kept reason the menu also shows in the menu's own words", () => {
    // ADR 0378: the confirm and the thread menu say the same thing.
    const c = archiveAllConfirmation(preflight(1, { question: 1, pending_change: 1 }))!;
    expect(BLOCKER_REASON.descendant_question).toContain('waiting for your answer');
    expect(c.message).toContain('1 waiting for your answer');
    expect(BLOCKER_REASON.descendant_pending_change).toContain('a change to apply or discard');
    expect(c.message).toContain('1 with a change to apply or discard');
  });
});

describe('the Archive all toast when nothing can go', () => {
  it('says how many need the user, and why', () => {
    expect(nothingToArchiveMessage(preflight(0, { question: 2, busy: 1 }))).toBe(
      'Nothing to archive. 3 threads in Current need you: 2 waiting for your answer, 1 still working.',
    );
    expect(nothingToArchiveMessage(preflight(0, { draft: 1 }))).toBe(
      'Nothing to archive. 1 thread in Current needs you: 1 with an unsent draft.',
    );
  });

  it('says plainly when Current is empty', () => {
    expect(nothingToArchiveMessage(preflight(0, {}))).toBe('Nothing to archive.');
  });
});

describe('the Archive all result', () => {
  it('reports the batch, and any thread that changed since the confirm', () => {
    expect(archiveAllResultMessage(5, {})).toBe('Archived 5 threads.');
    expect(archiveAllResultMessage(1, { question: 1, pinned: 1 })).toBe(
      'Archived 1 thread. 2 changed since you confirmed and stayed: 1 waiting for your answer, 1 now pinned.',
    );
  });

  it('draws Undo as a button, so a tap on the toast does not undo', () => {
    expect(toastTap({ type: 'success', action: undoAction(['t1']) })).toBeNull();
  });
});

describe('the Archive all requests', () => {
  it('split a large Current into batches the engine accepts', () => {
    const ids = Array.from({ length: ARCHIVE_ALL_MAX_IDS * 2 + 1 }, (_, i) => `t${i}`);
    const batches = idBatches(ids);
    expect(batches.map((b) => b.length)).toEqual([ARCHIVE_ALL_MAX_IDS, ARCHIVE_ALL_MAX_IDS, 1]);
    expect(batches.flat()).toEqual(ids);
    expect(idBatches([])).toEqual([]);
  });
});
