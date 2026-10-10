/**
 * The name the pairing form offers for the device in front of you.
 *
 * The bug this replaced was a placeholder reading "My iPhone" on a Mac running
 * Chrome. So the cases here are the ones that were wrong: a desktop browser, a
 * Chromium fork, and iPadOS Safari claiming to be a Mac.
 */
import { describe, it, expect } from 'vitest';
import { suggestDeviceLabel } from './deviceLabel';

/** Real user agents, trimmed of nothing. A rewritten one proves nothing. */
const UA = {
  chromeMac:
    'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36',
  safariMac:
    'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15',
  safariIphone:
    'Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Mobile/15E148 Safari/604.1',
  chromeIphone:
    'Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/151.0.0.0 Mobile/15E148 Safari/604.1',
  chromeAndroid:
    'Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36',
  firefoxWindows:
    'Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:130.0) Gecko/20100101 Firefox/130.0',
  edgeWindows:
    'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36 Edg/151.0.0.0',
  operaLinux:
    'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36 OPR/117.0.0.0',
  chromebook:
    'Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36',
};

/** A pointing device, which is what a desktop reports. */
const MOUSE = 0;

describe('the suggested name says what the device is', () => {
  it('names a desktop browser and its machine', () => {
    expect(suggestDeviceLabel({ userAgent: UA.chromeMac, maxTouchPoints: MOUSE }))
      .toBe('Chrome on Mac');
    expect(suggestDeviceLabel({ userAgent: UA.safariMac, maxTouchPoints: MOUSE }))
      .toBe('Safari on Mac');
    expect(suggestDeviceLabel({ userAgent: UA.firefoxWindows, maxTouchPoints: MOUSE }))
      .toBe('Firefox on Windows');
  });

  it('names a phone', () => {
    expect(suggestDeviceLabel({ userAgent: UA.safariIphone, maxTouchPoints: 5 }))
      .toBe('Safari on iPhone');
    expect(suggestDeviceLabel({ userAgent: UA.chromeIphone, maxTouchPoints: 5 }))
      .toBe('Chrome on iPhone');
    expect(suggestDeviceLabel({ userAgent: UA.chromeAndroid, maxTouchPoints: 5 }))
      .toBe('Chrome on Android');
  });

  it('reads a Chromium fork as itself, never as Chrome', () => {
    // Both carry `Chrome/` in the user agent, so asking about Chrome first
    // would name every fork wrong.
    expect(suggestDeviceLabel({ userAgent: UA.edgeWindows, maxTouchPoints: MOUSE }))
      .toBe('Edge on Windows');
    expect(suggestDeviceLabel({ userAgent: UA.operaLinux, maxTouchPoints: MOUSE }))
      .toBe('Opera on Linux');
  });

  it('reads CrOS as a Chromebook rather than Linux', () => {
    expect(suggestDeviceLabel({ userAgent: UA.chromebook, maxTouchPoints: MOUSE }))
      .toBe('Chrome on Chromebook');
  });

  it('tells an iPad from the Mac it claims to be, by the touch count', () => {
    // iPadOS Safari sends a Macintosh user agent, byte for byte. Nothing in the
    // string separates the two.
    expect(suggestDeviceLabel({ userAgent: UA.safariMac, maxTouchPoints: 5 }))
      .toBe('Safari on iPad');
  });

  it('suggests nothing at all for a browser it cannot read', () => {
    // The name is permanent and it is what you read before revoking, so an
    // empty field beats a wrong guess. The gateway names the device instead.
    expect(suggestDeviceLabel({ userAgent: '', maxTouchPoints: MOUSE })).toBeNull();
    expect(suggestDeviceLabel({ userAgent: 'curl/8.7.1', maxTouchPoints: MOUSE })).toBeNull();
  });

  it('offers half an answer when that is all there is', () => {
    expect(suggestDeviceLabel({ userAgent: 'Mozilla/5.0 (Windows NT 10.0)', maxTouchPoints: MOUSE }))
      .toBe('Windows');
    expect(suggestDeviceLabel({ userAgent: 'Firefox/130.0', maxTouchPoints: MOUSE }))
      .toBe('Firefox');
  });
});
