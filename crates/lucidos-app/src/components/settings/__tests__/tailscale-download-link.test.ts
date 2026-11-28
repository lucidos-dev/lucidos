import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const platformMocks = vi.hoisted(() => ({ isTauri: false, isIOS: false, isAndroid: false }));
vi.mock('../../../utils/platform', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/platform')>()),
  isTauri: () => platformMocks.isTauri,
  isIOS: () => platformMocks.isIOS,
  isAndroid: () => platformMocks.isAndroid,
}));

const openExternal = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/tauri', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/tauri')>()),
  openExternal,
}));

const openExternalUrl = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/openExternalUrl', () => ({ openExternalUrl }));

const showToast = vi.hoisted(() => vi.fn());
vi.mock('../../../store/store', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/store')>()),
  showToast,
}));

const { openTailscaleDownload, tailscaleDownloadUrl } = await import('../MobileAccessPage');

const DOWNLOAD_URL = 'https://tailscale.com/download';
const IOS_URL = 'https://apps.apple.com/app/tailscale/id1470499037';
const ANDROID_URL = 'https://play.google.com/store/apps/details?id=com.tailscale.ipn';

describe('tailscaleDownloadUrl', () => {
  it('sends each device to its own store', () => {
    expect(tailscaleDownloadUrl({ ios: true, android: false })).toBe(IOS_URL);
    expect(tailscaleDownloadUrl({ ios: false, android: true })).toBe(ANDROID_URL);
    expect(tailscaleDownloadUrl({ ios: false, android: false })).toBe(DOWNLOAD_URL);
  });

  it('prefers iOS if a UA somehow matches both', () => {
    expect(tailscaleDownloadUrl({ ios: true, android: true })).toBe(IOS_URL);
  });
});

describe('openTailscaleDownload', () => {
  beforeEach(() => {
    platformMocks.isTauri = false;
    platformMocks.isIOS = false;
    platformMocks.isAndroid = false;
    openExternal.mockReset();
    openExternalUrl.mockReset();
    showToast.mockReset();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('browser/PWA: routes through openExternalUrl without touching the Tauri bridge', () => {
    platformMocks.isTauri = false;
    openExternal.mockImplementation(() => {
      throw new TypeError("Cannot read properties of undefined (reading 'invoke')");
    });

    expect(() => openTailscaleDownload()).not.toThrow();

    expect(openExternalUrl).toHaveBeenCalledWith(DOWNLOAD_URL);
    expect(openExternal).not.toHaveBeenCalled();
  });

  it('iPhone: opens the App Store listing, not the desktop download page', () => {
    platformMocks.isIOS = true;
    openTailscaleDownload();
    expect(openExternalUrl).toHaveBeenCalledWith(IOS_URL);
  });

  it('Android: opens the Play Store listing', () => {
    platformMocks.isAndroid = true;
    openTailscaleDownload();
    expect(openExternalUrl).toHaveBeenCalledWith(ANDROID_URL);
  });

  it('desktop: uses the OS opener, not the embedded webview or a tab', () => {
    platformMocks.isTauri = true;
    openExternal.mockReturnValue(Promise.resolve());

    openTailscaleDownload();

    expect(openExternal).toHaveBeenCalledWith(DOWNLOAD_URL);
    expect(openExternalUrl).not.toHaveBeenCalled();
  });

  it('desktop: a rejected OS opener surfaces to the user instead of failing silently', async () => {
    platformMocks.isTauri = true;
    openExternal.mockReturnValue(Promise.reject(new Error('no handler')));

    openTailscaleDownload();
    await Promise.resolve();
    await Promise.resolve();

    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining(DOWNLOAD_URL),
      'error',
    );
  });
});
