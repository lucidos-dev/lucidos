import { DESKTOP_APP_UA_TOKEN } from './platform';


export interface BrowserFacts {
  userAgent: string;
  maxTouchPoints: number;
}

function browserName(ua: string): string | null {
  if (/\bEdg(?:e|A|iOS)?\//.test(ua)) return 'Edge';
  if (/\bOPR\/|\bOpera[\s/]/.test(ua)) return 'Opera';
  if (/\bSamsungBrowser\//.test(ua)) return 'Samsung Internet';
  if (/\bFirefox\/|\bFxiOS\//.test(ua)) return 'Firefox';
  if (/\bCriOS\/|\bChrome\/|\bChromium\//.test(ua)) return 'Chrome';
  if (/\bSafari\//.test(ua)) return 'Safari';
  return null;
}

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

export function suggestDeviceLabel(facts: BrowserFacts): string | null {
  const browser = browserName(facts.userAgent);
  const platform = platformName(facts);
  if (browser && platform) return `${browser} on ${platform}`;
  return browser ?? platform;
}

export function suggestDeviceLabelHere(): string | null {
  if (typeof navigator === 'undefined') return null;
  return suggestDeviceLabel({
    userAgent: navigator.userAgent ?? '',
    maxTouchPoints: navigator.maxTouchPoints ?? 0,
  });
}

export function userAgentDeviceLabel(ua: string | null | undefined): string | null {
  if (!ua) return null;
  if (ua.includes(DESKTOP_APP_UA_TOKEN)) {
    const platform = platformName({ userAgent: ua, maxTouchPoints: 0 });
    return platform ? `Lucidos app on ${platform}` : 'Lucidos app';
  }
  return suggestDeviceLabel({ userAgent: ua, maxTouchPoints: 0 });
}
