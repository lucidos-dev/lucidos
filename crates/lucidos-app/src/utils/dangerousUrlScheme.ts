
const DANGEROUS_URL_SCHEME_RE = /^(javascript|vbscript):/i;

export function hasDangerousScheme(url: string): boolean {
  return DANGEROUS_URL_SCHEME_RE.test(url.replace(/[\u0000-\u0020\s]/g, ''));
}
