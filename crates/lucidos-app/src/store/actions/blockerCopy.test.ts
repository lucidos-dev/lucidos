// A refused archive or delete is told in the thread menu's words (ADR 0378),
// whether the engine's 409 or the delete preflight said it.

import { describe, expect, it } from 'vitest';
import { ApiError } from '../../api/client';
import { BLOCKER_REASON, blockedRefusal, blockerFromMembers, strongerOwnBlocker } from './blockerCopy';
import { OWN_BLOCKER_PRIORITY, type Blocker } from '../../generated/thread-lifecycle';

const refusal = (body: Record<string, unknown>) => new ApiError(409, 'conflict', body);

describe('BLOCKER_REASON', () => {
  it('words every blocker the engine can name, as a sentence', () => {
    // A slug with no words would toast an empty card.
    const held: Blocker[] = [
      'home',
      ...OWN_BLOCKER_PRIORITY,
      ...OWN_BLOCKER_PRIORITY.map((o) => `descendant_${o}` as const),
    ];
    for (const blocker of held) {
      if (blocker === 'none') continue;
      expect(BLOCKER_REASON[blocker], blocker).toMatch(/^[A-Z].*\.$/);
    }
  });
});

describe('blockedRefusal', () => {
  it("reads the engine's blocker slug off a 409", () => {
    const err = refusal({ reason: 'parent_not_archivable', blocker: 'question' });
    expect(blockedRefusal(err, 't')).toEqual({ blocker: 'question', subThreadId: null });
  });

  it('shows the first blocking sub-thread, skipping the target itself', () => {
    const err = refusal({
      reason: 'descendants_blocking',
      blocker: 'descendant_running',
      blocking: [{ thread_id: 't' }, { thread_id: 'child' }],
    });
    expect(blockedRefusal(err, 't')).toEqual({ blocker: 'descendant_running', subThreadId: 'child' });
  });

  it('ignores anything that is not an explained cascade refusal', () => {
    expect(blockedRefusal(refusal({ reason: 'apply_in_progress' }), 't')).toBeNull();
    expect(blockedRefusal(refusal({ blocker: 'something_new' }), 't')).toBeNull();
    expect(blockedRefusal(new ApiError(500, 'oops', { blocker: 'running' }), 't')).toBeNull();
    expect(blockedRefusal(new Error('the tunnel dropped'), 't')).toBeNull();
  });
});

describe('blockerFromMembers', () => {
  it('reads the target as its own blocker', () => {
    const members = [{ thread_id: 't', reason: 'waiting_for_user_answer' }];
    expect(blockerFromMembers(members, 't')).toEqual({ blocker: 'question', subThreadId: null });
  });

  it('reads a live agent session as a turn still running', () => {
    const members = [{ thread_id: 't', reason: 'agent_session_live' }];
    expect(blockerFromMembers(members, 't')?.blocker).toBe('running');
  });

  it('names the strongest sub-thread reason and that sub-thread', () => {
    const members = [
      { thread_id: 'a', reason: 'pending_change' },
      { thread_id: 'b', reason: 'running' },
    ];
    expect(blockerFromMembers(members, 't')).toEqual({ blocker: 'descendant_running', subThreadId: 'b' });
  });

  it("puts the target's own reason before a sub-thread's", () => {
    const members = [
      { thread_id: 'b', reason: 'running' },
      { thread_id: 't', reason: 'pending_change' },
    ];
    expect(blockerFromMembers(members, 't')?.blocker).toBe('pending_change');
  });

  it('counts a reason it does not know as running, so it still blocks', () => {
    expect(blockerFromMembers([{ thread_id: 'a', reason: 'something_new' }], 't')?.blocker).toBe(
      'descendant_running',
    );
  });

  it('names the home thread outright', () => {
    expect(blockerFromMembers([{ thread_id: 't', reason: 'home_thread' }], 't')?.blocker).toBe('home');
  });

  it('answers null for a family nothing blocks', () => {
    expect(blockerFromMembers([], 't')).toBeNull();
  });
});

describe('strongerOwnBlocker', () => {
  it("follows the engine's priority order", () => {
    expect(strongerOwnBlocker('pending_change', 'running')).toBe('running');
    expect(strongerOwnBlocker(null, 'question')).toBe('question');
    expect(strongerOwnBlocker('question', null)).toBe('question');
  });
});
