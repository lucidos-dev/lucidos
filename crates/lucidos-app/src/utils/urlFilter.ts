/** Domains commonly used by ad/tracker iframes — not real page URLs. */
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

/** Path patterns that indicate ad/tracker iframe content regardless of domain. */
const AD_PATH_PATTERNS = [
  '/safeframe',   // IAB SafeFrame ad containers (used by many ad networks)
  '/ad-iframe',   // Generic ad iframe paths
];

/**
 * Returns true if the URL looks like a legitimate top-level page URL,
 * false if it looks like an iframe/ad/tracker/blank URL that should be ignored.
 */
export function isMainFrameUrl(url: string): boolean {
  if (!url) return false;

  // Reject non-http(s) schemes — these are iframe/internal content
  if (!url.startsWith('http://') && !url.startsWith('https://')) return false;

  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return false;
  }

  const hostname = parsed.hostname;
  const pathname = parsed.pathname.toLowerCase();

  // Reject known ad/tracker domains
  for (const domain of AD_TRACKER_DOMAINS) {
    if (hostname === domain || hostname.endsWith('.' + domain)) return false;
  }

  // Reject hostnames that look like ad servers (e.g., adsdkprod.azureedge.net)
  if (/^ads?[.-]/.test(hostname) || hostname.includes('.ads.')) return false;

  // Reject URLs with ad-related path patterns
  for (const pattern of AD_PATH_PATTERNS) {
    if (pathname.includes(pattern)) return false;
  }

  return true;
}
