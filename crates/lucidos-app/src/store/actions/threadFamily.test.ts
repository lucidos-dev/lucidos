import { afterEach, describe, expect, it } from 'vitest';
import { threadMap } from '../store';
import type { ThreadState } from '../thread-events';
import { makeThread } from '../__tests__/thread-wiring-helpers';
import { collectThreadFamily } from './threadFamily';

function thread(id: string, meta: Partial<ThreadState['meta']> = {}): ThreadState {
  const base = makeThread();
  return { ...base, meta: { ...base.meta, id, section: 'inbox', ...meta } };
}

function seed(...threads: ThreadState[]): void {
  threadMap.value = new Map(threads.map((t) => [t.meta.id, t]));
}

afterEach(() => {
  threadMap.value = new Map();
});

describe('collectThreadFamily', () => {
  it('collects every transitive descendant, the scope archive and delete share', () => {
    seed(
      thread('root'),
      thread('child', { parentThreadId: 'root' }),
      thread('grandchild', { parentThreadId: 'child' }),
      thread('unrelated'),
    );

    expect([...collectThreadFamily('root')].sort()).toEqual(['child', 'grandchild', 'root']);
  });
});
