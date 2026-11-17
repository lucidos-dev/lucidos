// @vitest-environment jsdom
/**
 * The composer words a known agent pause, and a failed read of it raises an
 * error toast rather than passing for "not paused".
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../api/client', async (original) => ({
  ...(await original<typeof import('../api/client')>()),
  readAgentPause: vi.fn(),
}));
vi.mock('../store/store', async (original) => ({
  ...(await original<typeof import('../store/store')>()),
  showToast: vi.fn(),
}));

import { readAgentPause } from '../api/client';
import { showToast } from '../store/store';
import { useAgentPaused } from './useAgentPaused';

let host: HTMLDivElement;
let paused: boolean | undefined;

function Probe({ threadId }: { threadId: string | null }) {
  paused = useAgentPaused(threadId);
  return null;
}

async function draw(threadId: string | null): Promise<void> {
  await act(async () => { render(<Probe threadId={threadId} />, host); });
  await act(async () => { await Promise.resolve(); });
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  paused = undefined;
  vi.mocked(readAgentPause).mockReset();
  vi.mocked(showToast).mockClear();
});

afterEach(() => {
  render(null, host);
  host.remove();
});

describe('useAgentPaused', () => {
  it('answers the pause the engine reports', async () => {
    vi.mocked(readAgentPause).mockResolvedValue(true);
    await draw('thread-1');
    expect(paused).toBe(true);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('reads nothing without a thread', async () => {
    await draw(null);
    expect(readAgentPause).not.toHaveBeenCalled();
    expect(paused).toBe(false);
  });

  it('raises an error toast when the read fails', async () => {
    vi.mocked(readAgentPause).mockRejectedValue(new Error('engine down'));
    await draw('thread-1');
    expect(paused).toBe(false);
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('Could not read whether the agent is paused'), 'error');
  });
});
