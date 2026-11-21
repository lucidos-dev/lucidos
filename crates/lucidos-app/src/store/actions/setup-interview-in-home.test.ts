/**
 * The setup interview runs in Home, where everything starts (ADR 0411), and
 * creates no thread of its own.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('./chat', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./chat')>()),
  sendMessage: vi.fn(async () => 'sent' as const),
}));
vi.mock('./threads', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./threads')>()),
  focusThread: vi.fn(),
}));

import { SETUP_INTERVIEW_PROMPT, startSetupInterview } from './compose';
import { sendMessage } from './chat';
import { focusThread } from './threads';
import { threadMap } from '../store';
import { makeThreadState } from './threads-test-helpers';

afterEach(() => {
  threadMap.value = new Map();
  vi.mocked(sendMessage).mockClear();
  vi.mocked(focusThread).mockClear();
});

describe('startSetupInterview', () => {
  it('opens Home and sends the interview there', async () => {
    threadMap.value = new Map([
      ['home', makeThreadState('home', { meta: { title: 'Home', home: true } })],
    ]);

    expect(await startSetupInterview()).toBe(true);

    expect(focusThread).toHaveBeenCalledWith('home');
    expect(sendMessage).toHaveBeenCalledTimes(1);
    expect(sendMessage).toHaveBeenCalledWith(SETUP_INTERVIEW_PROMPT, undefined, { threadId: 'home' });
    expect([...threadMap.value.keys()]).toEqual(['home']);
  });

  it('reports a send the engine refused', async () => {
    threadMap.value = new Map([
      ['home', makeThreadState('home', { meta: { title: 'Home', home: true } })],
    ]);
    vi.mocked(sendMessage).mockResolvedValueOnce('shown-as-failed');

    expect(await startSetupInterview()).toBe(false);
  });
});
