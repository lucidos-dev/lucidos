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
