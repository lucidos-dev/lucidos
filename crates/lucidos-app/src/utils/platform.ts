export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export const DESKTOP_APP_UA_TOKEN = 'Lucidos-Desktop';

export function registrationUserAgent(rawUa: string, isDesktopApp: boolean): string {
  return isDesktopApp ? `${rawUa} ${DESKTOP_APP_UA_TOKEN}` : rawUa;
}

export function describeDeviceUserAgent(ua: string | null | undefined): string {
  if (!ua) return 'Unknown device';
  const browser = ua.match(/(?:Chrome|Firefox|Safari|Edge|Opera)\/[\d.]+/)?.[0]
    || ua.match(/(?:Chrome|Firefox|Safari|Edge|Opera)/)?.[0]
    || 'Unknown browser';
  const os = ua.includes('iPhone') ? 'iOS'
    : ua.includes('iPad') ? 'iPadOS'
    : ua.includes('Mac') ? 'macOS'
    : ua.includes('Android') ? 'Android'
    : ua.includes('Windows') ? 'Windows'
    : ua.includes('Linux') ? 'Linux'
    : 'Unknown OS';
  return `${browser} on ${os}`;
}

export function isMobileDeviceUserAgent(ua: string | null | undefined): boolean {
  if (!ua) return false;
  if (ua.includes(DESKTOP_APP_UA_TOKEN)) return false;
  return /iPhone|iPad|iPod|Android/.test(ua);
}

export function isTauriPreGatewayEntryFor(opts: {
  isTauri: boolean;
  protocol: string;
  hostname: string;
}): boolean {
  if (!opts.isTauri) return false;
  if (opts.protocol !== 'http:' && opts.protocol !== 'https:') return true;
  return opts.hostname === 'tauri.localhost';
}

export function isTauriPreGatewayEntry(): boolean {
  return isTauriPreGatewayEntryFor({
    isTauri: isTauri(),
    protocol: typeof location !== 'undefined' ? location.protocol : 'https:',
    hostname: typeof location !== 'undefined' ? location.hostname : '',
  });
}

export const isMac = /Mac|iPhone|iPad|iPod/.test(navigator.userAgent);

const APPLE_DEVICE_UA = /iPad|iPhone|iPod/;

const _isIOS = APPLE_DEVICE_UA.test(navigator.userAgent) ||
  (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);
export function isIOS(): boolean { return _isIOS; }

export function isWebKitUserAgent(ua: string): boolean {
  if (APPLE_DEVICE_UA.test(ua)) return true;
  if (!/AppleWebKit/.test(ua)) return false;
  return !/Chrom(e|ium)|Edg\/|OPR\//.test(ua);
}

const _isWebKit = _isIOS || isWebKitUserAgent(navigator.userAgent);
export function isWebKit(): boolean { return _isWebKit; }

const _isAndroid = /Android/.test(navigator.userAgent);
export function isAndroid(): boolean { return _isAndroid; }

export function thisDeviceIsMobile(): boolean {
  return isIOS() || isAndroid();
}

const _isStandalone = window.matchMedia('(display-mode: standalone)').matches ||
  (navigator as Navigator & { standalone?: boolean }).standalone === true;
export function isStandalone(): boolean { return _isStandalone; }

const _isIOSPwa = _isIOS && _isStandalone;
export function isIOSPwa(): boolean { return _isIOSPwa; }

export interface ClipboardAbilities {
  copy: boolean;
  paste: boolean;
  copyImage: boolean;
}

export function clipboardAbilitiesOf(
  clipboard: Partial<Clipboard> | undefined,
  clipboardItem?: unknown,
): ClipboardAbilities {
  return {
    copy: typeof clipboard?.writeText === 'function',
    paste: typeof clipboard?.readText === 'function',
    copyImage: typeof clipboard?.write === 'function' && typeof clipboardItem === 'function',
  };
}

export function clipboardAbilities(): ClipboardAbilities {
  if (typeof navigator === 'undefined') return clipboardAbilitiesOf(undefined);
  return clipboardAbilitiesOf(navigator.clipboard, globalThis.ClipboardItem);
}

export function cameraIsAvailableIn(env: {
  mediaDevices: { getUserMedia?: unknown } | undefined;
  secureContext: boolean;
}): boolean {
  return env.secureContext && typeof env.mediaDevices?.getUserMedia === 'function';
}

export function cameraIsAvailable(): boolean {
  if (typeof navigator === 'undefined' || typeof window === 'undefined') return false;
  return cameraIsAvailableIn({
    mediaDevices: navigator.mediaDevices,
    secureContext: window.isSecureContext === true,
  });
}

export function hasHoverPointer(): boolean {
  return window.matchMedia('(hover: hover)').matches;
}
