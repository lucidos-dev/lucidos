/**
 * A name to offer for the device being paired, read off the browser.
 *
 * The pairing form fills its name field with this rather than dangling a
 * placeholder. A placeholder is a guess the user has to retype to accept, and
 * the guess it made was wrong on most devices: a laptop was invited to call
 * itself "My iPhone". A filled field is the same suggestion with the typing
 * already done, and it stays editable.
 *
 * The name matters beyond the moment: **Paired devices** lists it, it is what
 * you read before revoking, and nothing renames a device afterwards. So an
 * unrecognised browser suggests nothing at all and leaves the field empty,
 * where the gateway's own naming applies (`lucidos pair --label`, else "Paired
 * device"). A wrong name is worse than no name on a list you revoke from.
 */

/** What a browser tells us about itself. Taken as data so the naming is a pure
 *  function, testable without a DOM. */
export interface BrowserFacts {
  userAgent: string;
  /** iPadOS Safari claims to be a Mac. Only the touch count gives it away. */
  maxTouchPoints: number;
}

/** The engine, in the order a user would name it. Every Chromium fork carries
 *  `Chrome` in its user agent, so the forks are asked about first. */
function browserName(ua: string): string | null {
  if (/\bEdg(?:e|A|iOS)?\//.test(ua)) return 'Edge';
  if (/\bOPR\/|\bOpera[\s/]/.test(ua)) return 'Opera';
  if (/\bSamsungBrowser\//.test(ua)) return 'Samsung Internet';
  if (/\bFirefox\/|\bFxiOS\//.test(ua)) return 'Firefox';
  if (/\bCriOS\/|\bChrome\/|\bChromium\//.test(ua)) return 'Chrome';
  // Last, because every browser above also says Safari.
  if (/\bSafari\//.test(ua)) return 'Safari';
  return null;
}

/** The machine, named the way its owner would say it out loud. */
function platformName({ userAgent: ua, maxTouchPoints }: BrowserFacts): string | null {
  if (/\biPhone\b/.test(ua)) return 'iPhone';
  if (/\biPad\b/.test(ua)) return 'iPad';
  if (/\bAndroid\b/.test(ua)) return 'Android';
  if (/\bCrOS\b/.test(ua)) return 'Chromebook';
  if (/\bMacintosh\b|\bMac OS X\b/.test(ua)) return maxTouchPoints > 1 ? 'iPad' : 'Mac';
  if (/\bWindows\b/.test(ua)) return 'Windows';
  if (/\bLinux\b/.test(ua)) return 'Linux';
  return null;
}

/**
 * What to call this device, or `null` when the browser said nothing usable.
 *
 * Half an answer is still worth offering: a browser with no platform, or a
 * platform with no browser, names the device on its own.
 */
export function suggestDeviceLabel(facts: BrowserFacts): string | null {
  const browser = browserName(facts.userAgent);
  const platform = platformName(facts);
  if (browser && platform) return `${browser} on ${platform}`;
  return browser ?? platform;
}

/** The same suggestion for the browser running this code. Empty off a browser,
 *  which is where the form's field starts anyway. */
export function suggestDeviceLabelHere(): string | null {
  if (typeof navigator === 'undefined') return null;
  return suggestDeviceLabel({
    userAgent: navigator.userAgent ?? '',
    maxTouchPoints: navigator.maxTouchPoints ?? 0,
  });
}
