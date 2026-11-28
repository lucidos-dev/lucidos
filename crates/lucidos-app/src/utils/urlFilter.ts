const AD_TRACKER_DOMAINS = [
  'adtrafficquality.google',
  'googlesyndication.com',
  'doubleclick.net',
  'ampproject.org',
  'adsafeprotected.com',
  'moatads.com',
  'amazon-adsystem.com',
  'criteo.com',
  'taboola.com',
  'outbrain.com',
];

const AD_PATH_PATTERNS = [
  '/safeframe',
  '/ad-iframe',
];

export function isMainFrameUrl(url: string): boolean {
  if (!url) return false;

  if (!url.startsWith('http://') && !url.startsWith('https://')) return false;

  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return false;
  }

  const hostname = parsed.hostname;
  const pathname = parsed.pathname.toLowerCase();

  for (const domain of AD_TRACKER_DOMAINS) {
    if (hostname === domain || hostname.endsWith('.' + domain)) return false;
  }

  if (/^ads?[.-]/.test(hostname) || hostname.includes('.ads.')) return false;

  for (const pattern of AD_PATH_PATTERNS) {
    if (pathname.includes(pattern)) return false;
  }

  return true;
}
