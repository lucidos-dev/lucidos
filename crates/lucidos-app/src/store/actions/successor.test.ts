/**
 * Continue on the *Successor card*. A cold cache focuses the successor at once.
 * A warm one waits for the agent to spawn it, and the wait ends with the
 * agent's turn.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  continueInSuccessor: vi.fn(),
}));
vi.mock('./threads', () => ({ focusThreadOrBootstrap: vi.fn() }));

import { continueInSuccessor as postContinue } from '../../api/client';
import { focusedThreadId, threadMap } from '../store';
import { continueInSuccessor } from './successor';
import { focusThreadOrBootstrap } from './threads';
import { makeThreadState } from './threads-test-helpers';

function setThread(status: 'idle' | 'running', successor?: { thread_id: string; title: string }) {
  const thread = makeThreadState('old', { meta: { channel: 'claude_code', status, ...(successor && { successor }) } });
  threadMap.value = new Map([['old', thread]]);
}

const flush = () => new Promise<void>((resolve) => queueMicrotask(resolve));

describe('continueInSuccessor', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    focusedThreadId.value = 'old';
    setThread('idle');
  });

  it('focuses the successor at once on the brief path', async () => {
    vi.mocked(postContinue).mockResolvedValue({ path: 'brief', successor_thread_id: 'new' } as never);
    await continueInSuccessor('old');
    expect(focusThreadOrBootstrap).toHaveBeenCalledWith('new');
  });

  it('focuses the successor when the agent spawns it on the narrative path', async () => {
    vi.mocked(postContinue).mockResolvedValue({ path: 'narrative' } as never);
    await continueInSuccessor('old');
    expect(focusThreadOrBootstrap).not.toHaveBeenCalled();
    setThread('running');
    setThread('idle', { thread_id: 'new', title: 'Next' });
    await flush();
    expect(focusThreadOrBootstrap).toHaveBeenCalledWith('new');
  });

  it('stops waiting when the agent turn ends without a spawn', async () => {
    vi.mocked(postContinue).mockResolvedValue({ path: 'narrative' } as never);
    await continueInSuccessor('old');
    setThread('running');
    setThread('idle');
    await flush();
    setThread('idle', { thread_id: 'later', title: 'A later handoff' });
    await flush();
    expect(focusThreadOrBootstrap).not.toHaveBeenCalled();
  });

  it('leaves the focus alone when the user moved to another thread', async () => {
    vi.mocked(postContinue).mockResolvedValue({ path: 'narrative' } as never);
    await continueInSuccessor('old');
    focusedThreadId.value = 'elsewhere';
    setThread('idle', { thread_id: 'new', title: 'Next' });
    await flush();
    expect(focusThreadOrBootstrap).not.toHaveBeenCalled();
  });
});
