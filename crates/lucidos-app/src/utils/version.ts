/** Compare CalVer strings (YYYY.MM.DD.patch). Returns true if a > b. */
export function isNewerVersion(a: string, b: string): boolean {
  const ap = a.split('.').map(Number);
  const bp = b.split('.').map(Number);
  for (let i = 0; i < Math.max(ap.length, bp.length); i++) {
    const av = ap[i] ?? 0;
    const bv = bp[i] ?? 0;
    if (av !== bv) return av > bv;
  }
  return false;
}
