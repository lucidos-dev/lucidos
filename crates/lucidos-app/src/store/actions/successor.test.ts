/**
 * Continue on the *Successor card* starts a *successor session* in the same
 * thread. A message typed in a predecessor of a legacy successor thread moves
 * to the newest thread of its chain.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  continueInSuccessor: vi.fn(),
}));
vi.mock('./threads', () => ({ focusThreadOrBootstrap: vi.fn() }));
vi.mock('./compose', async () => {
  const { patchDraft } = await import('../composeDrafts');
  return { updateCompose: vi.fn((id: string, patch: object) => patchDraft(id, patch)) };
});

import { continueInSuccessor as postContinue } from '../../api/client';
import { focusedThreadId, threadMap } from '../store';
import { clearDraft, getDraft, patchDraft } from '../composeDrafts';
import { continueInSuccessor, latestSuccessor, moveDraftToSuccessor } from './successor';
import { focusThreadOrBootstrap } from './threads';
import { makeThreadState } from './threads-test-helpers';

function setThread(status: 'idle' | 'running', successor?: { thread_id: string; title: string }) {
  const thread = makeThreadState('old', { meta: { channel: 'claude_code', status, ...(successor && { successor }) } });
  threadMap.value = new Map([['old', thread]]);
}

describe('continueInSuccessor', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    focusedThreadId.value = 'old';
    setThread('idle');
    clearDraft('old');
  });

  it('starts the successor session in place on either path', async () => {
    for (const outcome of [{ path: 'brief', starts: 'now' }, { path: 'narrative' }] as const) {
      vi.mocked(postContinue).mockResolvedValue(outcome);
      await continueInSuccessor('old');
      expect(focusThreadOrBootstrap).not.toHaveBeenCalled();
      expect(focusedThreadId.value).toBe('old');
    }
  });

  it('sends the typed message and clears it once the engine took it', async () => {
    vi.mocked(postContinue).mockResolvedValue({ path: 'narrative' });
    patchDraft('old', { text: ' Add a toggle ' });
    await continueInSuccessor('old');
    expect(postContinue).toHaveBeenCalledWith('old', 'Add a toggle', undefined);
    expect(getDraft('old').text).toBe('');
  });

  it('keeps the typed message when the engine refuses', async () => {
    vi.mocked(postContinue).mockRejectedValue(new Error('refused'));
    patchDraft('old', { text: 'Add a toggle' });
    await expect(continueInSuccessor('old')).rejects.toThrow('refused');
    expect(getDraft('old').text).toBe('Add a toggle');
  });
});

describe('moveDraftToSuccessor', () => {
  const link = (thread_id: string) => ({ thread_id, title: thread_id });

  beforeEach(() => {
    vi.clearAllMocks();
    for (const id of ['old', 'mid', 'new']) clearDraft(id);
    threadMap.value = new Map([
      ['old', makeThreadState('old', { meta: { channel: 'claude_code', successor: link('mid') } })],
      ['mid', makeThreadState('mid', { meta: { channel: 'claude_code', successor: link('new') } })],
      ['new', makeThreadState('new', { meta: { channel: 'claude_code' } })],
    ]);
  });

  it('follows the chain to its newest thread', () => {
    expect(latestSuccessor('old')).toBe('new');
    expect(latestSuccessor('new')).toBeNull();
  });

  it('stops at a successor that is not loaded, and on a loop', () => {
    threadMap.value = new Map([
      ['old', makeThreadState('old', { meta: { successor: link('gone') } })],
      ['a', makeThreadState('a', { meta: { successor: link('b') } })],
      ['b', makeThreadState('b', { meta: { successor: link('a') } })],
    ]);
    expect(latestSuccessor('old')).toBe('gone');
    expect(latestSuccessor('a')).toBe('b');
  });

  it('moves the message to the newest thread, and focuses it', () => {
    patchDraft('old', { text: 'fix the header', image_hashes: ['img1'] });
    moveDraftToSuccessor('old');
    expect(getDraft('new').text).toBe('fix the header');
    expect(getDraft('new').image_hashes).toEqual(['img1']);
    expect(getDraft('old').text).toBe('');
    expect(focusThreadOrBootstrap).toHaveBeenCalledWith('new');
  });

  it('keeps the message here when the newest thread holds a draft', () => {
    patchDraft('old', { text: 'fix the header' });
    patchDraft('new', { text: 'half a thought' });
    moveDraftToSuccessor('old');
    expect(getDraft('new').text).toBe('half a thought');
    expect(getDraft('old').text).toBe('fix the header');
    expect(focusThreadOrBootstrap).toHaveBeenCalledWith('new');
  });

  it('keeps the message here when the newest thread is not loaded', () => {
    threadMap.value = new Map([
      ['old', makeThreadState('old', { meta: { channel: 'claude_code', successor: link('gone') } })],
    ]);
    patchDraft('old', { text: 'fix the header' });
    moveDraftToSuccessor('old');
    expect(getDraft('old').text).toBe('fix the header');
    expect(getDraft('gone').text).toBe('');
    expect(focusThreadOrBootstrap).toHaveBeenCalledWith('gone');
  });
});
