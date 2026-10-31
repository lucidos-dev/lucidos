/**
 * The composer's Camera item on a page that has no camera API.
 *
 * Over plain http on a LAN, `navigator.mediaDevices` is undefined. Reading
 * `getUserMedia` off it used to throw inside the capture overlay's effect,
 * before its `.catch` existed. The user got a black overlay and no toast.
 * `openRearCamera` must reject instead, so the overlay's catch reports it.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { NO_CAMERA_API, openRearCamera } from '../PromptInput';

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('openRearCamera', () => {
  it('rejects, rather than throwing, when the page has no mediaDevices', async () => {
    vi.stubGlobal('navigator', {});
    vi.stubGlobal('window', { isSecureContext: false });
    let result: Promise<MediaStream> | undefined;
    expect(() => { result = openRearCamera(); }).not.toThrow();
    await expect(result).rejects.toThrow(NO_CAMERA_API);
  });

  it('rejects on an insecure page even when mediaDevices is exposed', async () => {
    const getUserMedia = vi.fn(() => Promise.resolve({} as MediaStream));
    vi.stubGlobal('navigator', { mediaDevices: { getUserMedia } });
    vi.stubGlobal('window', { isSecureContext: false });
    await expect(openRearCamera()).rejects.toThrow(NO_CAMERA_API);
    expect(getUserMedia).not.toHaveBeenCalled();
  });

  it('asks for the rear camera on a secure page', async () => {
    const stream = {} as MediaStream;
    const getUserMedia = vi.fn(() => Promise.resolve(stream));
    vi.stubGlobal('navigator', { mediaDevices: { getUserMedia } });
    vi.stubGlobal('window', { isSecureContext: true });
    await expect(openRearCamera()).resolves.toBe(stream);
    expect(getUserMedia).toHaveBeenCalledWith({ video: { facingMode: 'environment' } });
  });
});
