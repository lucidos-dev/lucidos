// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const decoder = vi.hoisted(() => ({ loaded: vi.fn() }));
vi.mock('jsqr', () => {
  decoder.loaded();
  return { default: vi.fn() };
});
import PairingScanner from './PairingScanner';

const host = document.createElement('div');
afterEach(() => {
  act(() => render(null, host));
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

it('does not load the decoder or request the camera merely by importing the scanner', () => {
  expect(decoder.loaded).not.toHaveBeenCalled();
});

it('stops a camera stream that arrives after unmount', async () => {
  let resolveStream!: (stream: MediaStream) => void;
  const getUserMedia = vi.fn(() => new Promise<MediaStream>((resolve) => { resolveStream = resolve; }));
  vi.stubGlobal('navigator', { mediaDevices: { getUserMedia } });
  await act(async () => {
    render(<PairingScanner onCode={vi.fn()} />, host);
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
  await vi.waitFor(() => expect(getUserMedia).toHaveBeenCalledOnce());
  act(() => render(null, host));
  const stop = vi.fn();
  await act(async () => {
    resolveStream({ getTracks: () => [{ stop }] } as unknown as MediaStream);
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
  expect(stop).toHaveBeenCalledOnce();
});

it('shows a failure when camera access rejects', async () => {
  vi.stubGlobal('navigator', { mediaDevices: { getUserMedia: vi.fn().mockRejectedValue(new Error('denied')) } });
  act(() => render(<PairingScanner onCode={vi.fn()} />, host));
  await vi.waitFor(() => expect(host.textContent).toContain('could not start the camera scanner'));
});
