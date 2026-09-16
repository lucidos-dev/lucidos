/** The delete confirmation says only what is true of THIS family.
 *
 *  Invariant I8 of `docs/plans/2026-09-16-implement-thread-deletion.md`. Each
 *  line below is gated on one preflight flag. A line shown when its flag is off
 *  is a claim the delete does not make: branch work on a chat thread, backups
 *  on a workspace that never took one, forgetting on a thread that taught
 *  nothing.
 */

import { describe, it, expect } from 'vitest';
import { deleteConfirmation, formatDeleteErrorToast } from './threads-delete';
import type { DeletePreflight } from '../../api/threads';
import { ApiError } from '../../api/client';

/** A single chat thread with nothing to warn about. Each case below turns on
 *  exactly the flag it is testing. */
function preflight(over: Partial<DeletePreflight> = {}): DeletePreflight {
  return {
    thread_count: 1,
    sub_thread_titles: [],
    memory_count: 0,
    has_unapplied_branch_work: false,
    has_applied_changes: false,
    backups_present: false,
    blocked_by: [],
    ...over,
  };
}

/** One row per conditional line: the flag that shows it, and a phrase that
 *  appears only in that line. */
const CONDITIONAL_LINES: {
  label: string;
  on: Partial<DeletePreflight>;
  phrase: string;
}[] = [
  {
    label: 'what Lucidos learned',
    on: { memory_count: 3 },
    phrase: 'forget what it learned here',
  },
  {
    label: 'unapplied branch work',
    on: { has_unapplied_branch_work: true },
    phrase: 'never applied goes too',
  },
  {
    label: 'applied code stays',
    on: { has_applied_changes: true },
    phrase: 'Code you already applied stays',
  },
  {
    label: 'backups still hold it',
    on: { backups_present: true },
    phrase: 'Backups taken before now',
  },
];

describe('deleteConfirmation', () => {
  describe('every conditional line is off until its flag is on', () => {
    for (const { label, on, phrase } of CONDITIONAL_LINES) {
      it(`${label} is hidden when its flag is off`, () => {
        expect(deleteConfirmation(preflight(), 'Heat pump').message).not.toContain(phrase);
      });

      it(`${label} is shown when its flag is on`, () => {
        expect(deleteConfirmation(preflight(on), 'Heat pump').message).toContain(phrase);
      });
    }
  });

  it('shows every line at once when every flag is on', () => {
    const all = deleteConfirmation(
      preflight({
        memory_count: 9,
        has_unapplied_branch_work: true,
        has_applied_changes: true,
        backups_present: true,
      }),
      'Heat pump',
    ).message;
    for (const { phrase } of CONDITIONAL_LINES) expect(all).toContain(phrase);
  });

  it('always names the thread and always says there is no undo', () => {
    const bare = deleteConfirmation(preflight(), 'Heat pump experiments');
    expect(bare.title).toBe('Delete this thread?');
    expect(bare.message).toContain('"Heat pump experiments"');
    expect(bare.message).toContain('This cannot be undone.');
  });

  it('falls back to a generic name for an untitled thread', () => {
    expect(deleteConfirmation(preflight(), '   ').message).toContain('"this thread"');
  });

  it('counts the cascade in the title and in the first line', () => {
    const family = deleteConfirmation(
      preflight({ thread_count: 4, sub_thread_titles: ['a', 'b', 'c'] }),
      'Heat pump',
    );
    expect(family.title).toBe('Delete 4 threads?');
    expect(family.message).toContain('and its 3 sub-threads');
  });

  it('says sub-thread in the singular for a family of two', () => {
    const pair = deleteConfirmation(
      preflight({ thread_count: 2, sub_thread_titles: ['only child'] }),
      'Heat pump',
    );
    expect(pair.title).toBe('Delete 2 threads?');
    expect(pair.message).toContain('and its sub-thread,');
    expect(pair.details?.groups[0].header).toBe('Sub-thread');
  });

  it('mentions no cascade for a thread that is alone', () => {
    const alone = deleteConfirmation(preflight(), 'Heat pump');
    expect(alone.message).not.toContain('sub-thread');
    expect(alone.details).toBeUndefined();
  });

  it('expands the count into the list of titles', () => {
    const family = deleteConfirmation(
      preflight({ thread_count: 3, sub_thread_titles: ['Pump curves', '   '] }),
      'Heat pump',
    );
    expect(family.details?.groups[0].header).toBe('2 sub-threads');
    expect(family.details?.groups[0].items).toEqual(['Pump curves', 'Untitled thread']);
  });

  it('separates each line into its own paragraph', () => {
    // `<DialogMessage>` renders one <p> per blank-line block, and collapses a
    // single newline to a space. Six conditions on one line would read as a
    // wall of text.
    const all = deleteConfirmation(
      preflight({ memory_count: 1, has_applied_changes: true }),
      'Heat pump',
    ).message;
    expect(all.split('\n\n')).toHaveLength(4);
  });
});

describe('formatDeleteErrorToast', () => {
  it('names the owner-only refusal rather than showing a bare 403', () => {
    const refusal = new ApiError(403, 'forbidden', { reason: 'not_the_owners_device' });
    expect(formatDeleteErrorToast(refusal)).toBe('Only a signed-in device can delete a thread.');
  });

  it('names the one blocker and what it is doing', () => {
    const one = new ApiError(409, 'conflict', {
      reason: 'descendants_blocking',
      blocking: [{ thread_id: 'a', title: 'Pump curves', reason: 'running' }],
    });
    expect(formatDeleteErrorToast(one, 'target')).toBe(
      `Can't delete yet, "Pump curves" is still running`,
    );
  });

  it('says THIS thread when the only blocker is the target itself', () => {
    // The engine's blocking list covers the whole family. Reporting the target
    // as a sub-thread told a childless thread one of its sub-threads was busy.
    const itself = new ApiError(409, 'conflict', {
      reason: 'descendants_blocking',
      blocking: [{ thread_id: 'target', title: 'Mine', reason: 'agent_session_live' }],
    });
    expect(formatDeleteErrorToast(itself, 'target')).toBe(
      "Can't delete, this thread still has a coding agent running",
    );
  });

  it('falls back to counting when more than one member blocks', () => {
    const many = new ApiError(409, 'conflict', {
      reason: 'descendants_blocking',
      blocking: [
        { thread_id: 'a', reason: 'running' },
        { thread_id: 'b', reason: 'agent_session_live' },
      ],
    });
    expect(formatDeleteErrorToast(many, 'target')).toContain(
      '2 threads in this family are still busy',
    );
  });

  it('says busy for a reason it does not recognise', () => {
    const unknown = new ApiError(409, 'conflict', {
      reason: 'descendants_blocking',
      blocking: [{ thread_id: 'a', reason: 'something_new' }],
    });
    expect(formatDeleteErrorToast(unknown, 'target')).toContain('a sub-thread is still busy');
  });

  it('tells a parked thread apart from a running one', () => {
    const parked = new ApiError(409, 'conflict', {
      reason: 'parent_not_deletable',
      parent_status: 'waiting_for_user_answer',
    });
    expect(formatDeleteErrorToast(parked)).toContain('waiting for your answer');

    const running = new ApiError(409, 'conflict', {
      reason: 'parent_not_deletable',
      parent_status: 'running',
    });
    expect(formatDeleteErrorToast(running)).toContain('still running');
  });

  it('points a pending change at Apply or Discard', () => {
    const pending = new ApiError(409, 'conflict', { reason: 'parent_has_pending_changes' });
    expect(formatDeleteErrorToast(pending)).toContain('apply or discard the pending change');
  });

  it('falls back to the error detail for anything unstructured', () => {
    expect(formatDeleteErrorToast(new Error('the tunnel dropped'))).toContain('the tunnel dropped');
  });
});
