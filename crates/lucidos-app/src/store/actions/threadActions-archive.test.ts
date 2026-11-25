import { describe, it, expect, vi } from 'vitest';
import { threadMap, focusedThreadId } from '../store';
import { makeThreadState } from './threads-test-helpers';
import { runCloseCascade } from './threadActions';
import { handleArchiveThread } from './threads';

vi.mock('./threads', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./threads')>()),
  handleArchiveThread: vi.fn(),
}));

describe('the close cascade waits for the archive it starts', () => {
  it('resolves only once the archive has finished', async () => {
    let finish!: () => void;
    vi.mocked(handleArchiveThread).mockReturnValueOnce(new Promise<void>((resolve) => { finish = resolve; }));
    threadMap.value = new Map([['t1', makeThreadState('t1', { meta: { section: 'inbox', status: 'idle' } })]]);
    focusedThreadId.value = 't1';

    let settled = false;
    const cascade = runCloseCascade().then(() => { settled = true; });
    for (let i = 0; i < 5; i++) await Promise.resolve();

    expect(handleArchiveThread).toHaveBeenCalledWith('t1');
    expect(settled).toBe(false);

    finish();
    await cascade;
    expect(settled).toBe(true);
  });
});
