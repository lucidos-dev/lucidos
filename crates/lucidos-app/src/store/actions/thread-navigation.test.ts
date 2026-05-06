import { describe, it, expect } from 'vitest';
import { pushThreadEntry, removeEntriesById, replaceThreadEntry, MAX_THREAD_NAV_STACK, type ThreadNavEntry } from './thread-navigation';

const T = (id: string): ThreadNavEntry => ({ type: 'thread', id });

describe('pushThreadEntry', () => {
  it('seeds an empty stack from cursor=-1 — initial focus on a fresh tab', () => {
    // Regression: ensureInitialized leaves stack=[] and cursor=-1 when no thread is
    // focused at startup. The first focusThread() call lands here. Before the
    // guard, `cursor < stack.length` was `-1 < 0` (true), so entriesEqual read
    // stack[-1].type and threw, aborting focusThread before navigateToPane ran.
    // Symptom: tapping a thread on mobile focused it but never switched panes.
    const result = pushThreadEntry([], -1, T('thread-1'));
    expect(result).not.toBeNull();
    expect(result!.stack).toEqual([T('thread-1')]);
    expect(result!.cursor).toBe(0);
  });

  it('returns null for duplicate thread entry at cursor', () => {
    expect(pushThreadEntry([T('thread-1')], 0, T('thread-1'))).toBeNull();
  });

  it('pushes a new thread after an existing one', () => {
    const result = pushThreadEntry([T('thread-1')], 0, T('thread-2'));
    expect(result).not.toBeNull();
    expect(result!.stack).toEqual([T('thread-1'), T('thread-2')]);
    expect(result!.cursor).toBe(1);
  });

  it('truncates forward history when pushing from middle', () => {
    const stack: ThreadNavEntry[] = [T('t0'), T('t1'), T('t2')];
    const result = pushThreadEntry(stack, 1, T('t3'));
    expect(result).not.toBeNull();
    expect(result!.stack).toEqual([T('t0'), T('t1'), T('t3')]);
    expect(result!.cursor).toBe(2);
  });

  it('caps stack at MAX_THREAD_NAV_STACK', () => {
    let stack: ThreadNavEntry[] = [T('seed')];
    let cursor = 0;
    for (let i = 0; i < MAX_THREAD_NAV_STACK; i++) {
      const r = pushThreadEntry(stack, cursor, T(`thread-${i}`));
      stack = r!.stack; cursor = r!.cursor;
    }
    expect(stack).toHaveLength(MAX_THREAD_NAV_STACK);
    expect(cursor).toBe(MAX_THREAD_NAV_STACK - 1);
  });
});

describe('replaceThreadEntry', () => {
  it('replaces the cursor entry, dropping forward history', () => {
    const r = replaceThreadEntry([T('t0'), T('t1'), T('t2')], 1, T('t9'));
    expect(r.stack).toEqual([T('t0'), T('t9')]);
    expect(r.cursor).toBe(1);
  });

  it('replaces the only entry', () => {
    const r = replaceThreadEntry([T('t0')], 0, T('t1'));
    expect(r.stack).toEqual([T('t1')]);
    expect(r.cursor).toBe(0);
  });

  it('is a no-op (with forward truncate) when cursor entry already matches', () => {
    const r = replaceThreadEntry([T('t0'), T('t1'), T('t2')], 1, T('t1'));
    expect(r.stack).toEqual([T('t0'), T('t1')]);
    expect(r.cursor).toBe(1);
  });

  it('falls back to push when cursor is past end of stack', () => {
    const r = replaceThreadEntry([T('t0')], 5, T('t1'));
    expect(r.stack).toEqual([T('t0'), T('t1')]);
    expect(r.cursor).toBe(1);
  });

  it('seeds an empty stack from cursor=-1 via push fallback', () => {
    const r = replaceThreadEntry([], -1, T('thread-1'));
    expect(r.stack).toEqual([T('thread-1')]);
    expect(r.cursor).toBe(0);
  });
});

describe('removeEntriesById', () => {
  it('returns null when id is not in stack', () => {
    expect(removeEntriesById([T('a'), T('b')], 1, 'missing')).toBeNull();
  });

  it('removes the cursor entry and shifts cursor back to the previous entry', () => {
    // Discarding a draft that the user is currently sitting on: cursor should
    // land on the entry before the removed one so the next Back/Forward feels
    // contiguous from the user's last "real" position.
    const r = removeEntriesById([T('x'), T('draft')], 1, 'draft');
    expect(r).not.toBeNull();
    expect(r!.stack).toEqual([T('x')]);
    expect(r!.cursor).toBe(0);
  });

  it('removes an entry before the cursor and decrements cursor', () => {
    const r = removeEntriesById([T('draft'), T('x'), T('y')], 2, 'draft');
    expect(r).not.toBeNull();
    expect(r!.stack).toEqual([T('x'), T('y')]);
    expect(r!.cursor).toBe(1);
  });

  it('removes an entry after the cursor and leaves cursor alone', () => {
    const r = removeEntriesById([T('x'), T('y'), T('draft')], 0, 'draft');
    expect(r).not.toBeNull();
    expect(r!.stack).toEqual([T('x'), T('y')]);
    expect(r!.cursor).toBe(0);
  });

  it('removes every occurrence of the id', () => {
    const r = removeEntriesById([T('a'), T('dup'), T('b'), T('dup')], 2, 'dup');
    expect(r).not.toBeNull();
    expect(r!.stack).toEqual([T('a'), T('b')]);
    expect(r!.cursor).toBe(1);
  });

  it('empties the stack and resets cursor when the only entry is removed', () => {
    const r = removeEntriesById([T('only')], 0, 'only');
    expect(r).not.toBeNull();
    expect(r!.stack).toEqual([]);
    expect(r!.cursor).toBe(-1);
  });
});

describe('thread back/forward simulation', () => {
  it('full navigation cycle', () => {
    let stack: ThreadNavEntry[] = [T('seed')];
    let cursor = 0;

    let r = pushThreadEntry(stack, cursor, T('thread-1'));
    stack = r!.stack; cursor = r!.cursor;
    r = pushThreadEntry(stack, cursor, T('thread-2'));
    stack = r!.stack; cursor = r!.cursor;
    expect(stack).toEqual([T('seed'), T('thread-1'), T('thread-2')]);
    expect(cursor).toBe(2);

    cursor--;
    expect(stack[cursor]).toEqual(T('thread-1'));
    cursor--;
    expect(stack[cursor]).toEqual(T('seed'));

    cursor++;
    expect(stack[cursor]).toEqual(T('thread-1'));

    r = pushThreadEntry(stack, cursor, T('thread-3'));
    stack = r!.stack; cursor = r!.cursor;
    expect(stack).toEqual([T('seed'), T('thread-1'), T('thread-3')]);
  });
});
