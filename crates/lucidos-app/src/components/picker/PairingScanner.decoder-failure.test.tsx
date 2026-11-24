// @vitest-environment jsdom
import { expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import PairingScanner from './PairingScanner';

const recovery = vi.hoisted(() => ({ reload: vi.fn(() => true), log: vi.fn() }));
vi.mock('../../hooks/sw-update', () => ({ reloadForStaleChunk: recovery.reload }));
vi.mock('../../utils/clientLog', () => ({ postClientLog: recovery.log }));
vi.mock('jsqr', () => { throw new Error('decoder unavailable'); });

it('shows decoder load failure without opening the camera', async () => {
  const getUserMedia = vi.fn();
  vi.stubGlobal('navigator', { mediaDevices: { getUserMedia } });
  const host = document.createElement('div');
  try {
    act(() => render(<PairingScanner onCode={vi.fn()} />, host));
    await vi.waitFor(() => expect(host.textContent).toContain('could not start the camera scanner'));
    expect(getUserMedia).not.toHaveBeenCalled();
    expect(recovery.reload).toHaveBeenCalledOnce();
    expect(recovery.log).toHaveBeenCalledWith('chunk', 'load_failed', expect.objectContaining({
      message: expect.any(String),
    }));
  } finally {
    act(() => render(null, host));
    vi.unstubAllGlobals();
  }
});
