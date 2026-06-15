import { describe, it, expect, beforeEach } from 'vitest';
import { threadMap, workspaceHasHistory } from './store';
import type { ThreadState } from './thread-events';

/** The computed only reads `meta.state`, so a minimal stub is enough. */
function thread(state: ThreadState['meta']['state']): ThreadState {
  return { meta: { state } } as unknown as ThreadState;
}

describe('workspaceHasHistory — new-workspace welcome gate', () => {
  beforeEach(() => {
    threadMap.value = new Map();
  });

  it('is false on a pristine workspace (no threads)', () => {
    expect(workspaceHasHistory.value).toBe(false);
  });

  it('is false when only composing drafts and discarded threads exist', () => {
    threadMap.value = new Map([
      ['a', thread('composing')],
      ['b', thread('discarded')],
    ]);
    expect(workspaceHasHistory.value).toBe(false);
  });

  it('is true once an active thread exists (archived threads also stay state=active)', () => {
    threadMap.value = new Map([['a', thread('active')]]);
    expect(workspaceHasHistory.value).toBe(true);
  });

  it('is true when a real thread sits alongside a composing draft', () => {
    threadMap.value = new Map([
      ['draft', thread('composing')],
      ['real', thread('active')],
    ]);
    expect(workspaceHasHistory.value).toBe(true);
  });
});
