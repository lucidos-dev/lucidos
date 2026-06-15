// Build-time detection of a wedged `vite build --watch` serving stale CSS.
//
// A long-running `vite build --watch` (the `--built` dev mode that survives
// engine-only Apply restarts — see `.claude/rules/scripts.md`) can wedge its CSS
// pipeline: it keeps re-emitting fresh JS from changed source while serving a
// FROZEN, stale CSS bundle. The served index.html then pairs new JS (new class
// names from a rename) with old CSS (old class names), so every renamed/new
// class has no matching rule and the app renders unstyled — silently. This
// happened once after a large cc->CodingAgent class rename: the watch had been
// running 1.5 days and froze its CSS output.
//
// The wedge has a precise, build-observable signature: the CSS *source* changed
// since the previous build, but the emitted CSS bundle did NOT. These pure
// helpers express that signature so `cssStalenessGuard` in `vite.config.ts` can
// warn loudly in the build log instead of letting the desync fail silently.

export interface CssBuildSnapshot {
  /** Hash of the normalized CSS *source* (comments + whitespace stripped), read
   *  fresh from disk on each build — independent of Vite's (possibly wedged)
   *  module cache, so it always reflects the true current source. */
  cssSourceHash: string;
  /** Stable fingerprint of the *emitted* CSS assets (their content-hashed
   *  filenames). Changes iff the built CSS actually changed. */
  cssOutputFingerprint: string;
}

/** Strip CSS comments and collapse whitespace so a cosmetic-only source edit
 *  (reformatting, a comment tweak) doesn't read as a semantic change. Without
 *  this, a no-op rebuild whose minified output is byte-identical would false-fire
 *  the wedge warning. */
export function normalizeCss(src: string): string {
  return src
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/\s+/g, ' ')
    .trim();
}

/** The wedge signature: the (normalized) CSS source changed since the previous
 *  build, but the emitted CSS bundle did not — i.e. Vite re-emitted JS from
 *  fresh source while serving frozen, stale CSS. The first build (`prev === null`)
 *  is never a wedge — there's no prior generation to compare against. */
export function isCssWedge(prev: CssBuildSnapshot | null, curr: CssBuildSnapshot): boolean {
  return prev !== null
    && curr.cssSourceHash !== prev.cssSourceHash
    && curr.cssOutputFingerprint === prev.cssOutputFingerprint;
}
