import { describe, it, expect } from 'vitest';

// Pure logic duplicated from thread-navigation.ts to avoid module-level
// store imports — mirrors navigation.test.ts pattern.
type ThreadNavEntry =
  | { type: 'thread'; id: string }
  | { type: 'draft'; id: string };

const MAX_THREAD_NAV_STACK = 50;

function entriesEqual(a: ThreadNavEntry, b: ThreadNavEntry): boolean {
  return a.type === b.type && a.id === b.id;
}

function pushThreadEntry(
  stack: ThreadNavEntry[],
  cursor: number,
  entry: ThreadNavEntry,
): { stack: ThreadNavEntry[]; cursor: number } | null {
  if (cursor < stack.length && entriesEqual(stack[cursor], entry)) return null;
  let newStack = [...stack.slice(0, cursor + 1), entry];
  let newCursor = newStack.length - 1;
  if (newStack.length > MAX_THREAD_NAV_STACK) {
    const overflow = newStack.length - MAX_THREAD_NAV_STACK;
    newStack = newStack.slice(overflow);
    newCursor -= overflow;
  }
  return { stack: newStack, cursor: newCursor };
}

const T = (id: string): ThreadNavEntry => ({ type: 'thread', id });
const D = (id: string): ThreadNavEntry => ({ type: 'draft', id });

describe('pushThreadEntry', () => {
  it('returns null for duplicate thread entry at cursor', () => {
    expect(pushThreadEntry([T('thread-1')], 0, T('thread-1'))).toBeNull();
  });

  it('returns null for duplicate draft entry at cursor', () => {
    expect(pushThreadEntry([D('draft-1')], 0, D('draft-1'))).toBeNull();
  });

  it('does not collapse different draft ids', () => {
    const result = pushThreadEntry([D('draft-1')], 0, D('draft-2'));
    expect(result).not.toBeNull();
    expect(result!.stack).toEqual([D('draft-1'), D('draft-2')]);
    expect(result!.cursor).toBe(1);
  });

  it('thread and draft with same id are different entries', () => {
    const result = pushThreadEntry([T('shared')], 0, D('shared'));
    expect(result).not.toBeNull();
    expect(result!.stack).toEqual([T('shared'), D('shared')]);
  });

  it('pushes new thread ID', () => {
    const result = pushThreadEntry([D('draft-x')], 0, T('thread-1'));
    expect(result).not.toBeNull();
    expect(result!.stack).toEqual([D('draft-x'), T('thread-1')]);
    expect(result!.cursor).toBe(1);
  });

  it('pushes draft after thread', () => {
    const result = pushThreadEntry([T('thread-1')], 0, D('draft-x'));
    expect(result).not.toBeNull();
    expect(result!.stack).toEqual([T('thread-1'), D('draft-x')]);
    expect(result!.cursor).toBe(1);
  });

  it('truncates forward history when pushing from middle', () => {
    const stack: ThreadNavEntry[] = [D('d0'), T('thread-1'), T('thread-2')];
    const result = pushThreadEntry(stack, 1, T('thread-3'));
    expect(result).not.toBeNull();
    expect(result!.stack).toEqual([D('d0'), T('thread-1'), T('thread-3')]);
    expect(result!.cursor).toBe(2);
  });

  it('caps stack at MAX_THREAD_NAV_STACK', () => {
    let stack: ThreadNavEntry[] = [D('seed')];
    let cursor = 0;
    for (let i = 0; i < MAX_THREAD_NAV_STACK; i++) {
      const r = pushThreadEntry(stack, cursor, T(`thread-${i}`));
      stack = r!.stack; cursor = r!.cursor;
    }
    expect(stack).toHaveLength(MAX_THREAD_NAV_STACK);
    expect(cursor).toBe(MAX_THREAD_NAV_STACK - 1);
  });
});

describe('thread back/forward simulation', () => {
  it('full navigation cycle', () => {
    let stack: ThreadNavEntry[] = [D('seed')];
    let cursor = 0;

    let r = pushThreadEntry(stack, cursor, T('thread-1'));
    stack = r!.stack; cursor = r!.cursor;
    r = pushThreadEntry(stack, cursor, T('thread-2'));
    stack = r!.stack; cursor = r!.cursor;
    expect(stack).toEqual([D('seed'), T('thread-1'), T('thread-2')]);
    expect(cursor).toBe(2);

    cursor--;
    expect(stack[cursor]).toEqual(T('thread-1'));
    cursor--;
    expect(stack[cursor]).toEqual(D('seed'));

    cursor++;
    expect(stack[cursor]).toEqual(T('thread-1'));

    r = pushThreadEntry(stack, cursor, T('thread-3'));
    stack = r!.stack; cursor = r!.cursor;
    expect(stack).toEqual([D('seed'), T('thread-1'), T('thread-3')]);
  });
});
