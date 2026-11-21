interface ReleaseVersion {
  core: bigint[];
  pre: string[];
}

/** Strict semver, matching the Rust semver crate's u64 core components. */
function parseRelease(value: string): ReleaseVersion | null {
  const match = /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/.exec(value.trim());
  if (!match) return null;
  const core = match.slice(1, 4).map(BigInt);
  if (core.some(n => n > 18446744073709551615n)) return null;
  const pre = match[4]?.split('.') ?? [];
  if (pre.some(id => /^[0-9]+$/.test(id) && id.length > 1 && id.startsWith('0'))) return null;
  return { core, pre };
}

/** Whether a release feed names a valid strict semver version. */
export function isValidReleaseVersion(version: string): boolean {
  return parseRelease(version) !== null;
}

/** Release precedence only; build metadata never makes an update. Invalid input fails closed.
 * Rust's is_upgrade is the authority; the generated fixture tests every pair here. */
export function isNewerVersion(offered: string, current: string): boolean {
  const a = parseRelease(offered);
  const b = parseRelease(current);
  if (!a || !b) return false;
  for (let i = 0; i < a.core.length; i++) {
    if (a.core[i] !== b.core[i]) return a.core[i] > b.core[i];
  }
  if (!a.pre.length || !b.pre.length) return !a.pre.length && !!b.pre.length;
  for (let i = 0; i < Math.min(a.pre.length, b.pre.length); i++) {
    const av = a.pre[i];
    const bv = b.pre[i];
    if (av === bv) continue;
    const an = /^[0-9]+$/.test(av);
    const bn = /^[0-9]+$/.test(bv);
    if (an && bn) return BigInt(av) > BigInt(bv);
    if (an !== bn) return bn;
    return av > bv;
  }
  return a.pre.length > b.pre.length;
}

/** Dev build IDs are YYYY.MM.DD.patch, separate from published releases. */
export function isNewerBuildId(offered: string, current: string): boolean {
  const parse = (value: string): bigint[] | null =>
    /^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$/.test(value) ? value.split('.').map(BigInt) : null;
  const a = parse(offered);
  const b = parse(current);
  if (!a || !b) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return a[i] > b[i];
  }
  return false;
}
