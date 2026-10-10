/**
 * Regression guard for a "Get Tailscale" button that did nothing on a phone.
 *
 * The handler used to be `openExternal(url).catch(() => window.open(url))`,
 * reading as "try the desktop OS opener, fall back to a tab in the browser".
 * The fallback was dead code: `invoke` dereferences `window.__TAURI_INTERNALS__!`
 * SYNCHRONOUSLY (`utils/tauri.ts`) and `openExternal` is a plain function, so
 * off Tauri the call THROWS out of the handler before a promise exists and no
 * `.catch()` runs. Mobile Access is specifically the page a user opens from
 * their phone, which is exactly where the dead branch was the only branch.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

// Partial mocks throughout: the page pulls in the store graph, so replacing a
// module wholesale strands unrelated importers on missing exports.
const platformMocks = vi.hoisted(() => ({ isTauri: false, isIOS: false, isAndroid: false }));
vi.mock('../../../utils/platform', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/platform')>()),
  isTauri: () => platformMocks.isTauri,
  isIOS: () => platformMocks.isIOS,
  isAndroid: () => platformMocks.isAndroid,
}));

// The stub is driven per-test to THROW off Tauri, mirroring the real module. A
// mock that merely rejected would let the old dead `.catch()` fallback pass.
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
  // Half this page's readers are holding the phone they need to install on, so
  // the desktop download page is the wrong destination for them. All three URLs
  // were checked live (HTTP 200) when this was written.
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
    // The real bridge throws here; the handler must never reach it.
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
