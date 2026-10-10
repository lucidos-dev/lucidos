/**
 * The predicate saying a URL must never be OPENED.
 *
 * It lives in `utils/` because both the ingress and the sink need it, and a
 * security predicate kept in two places drifts.
 *
 * The deep-link parser (`store/actions/notification-deeplink.ts`) screens a tap
 * before it becomes a navigate. The sinks screen the value they are about to
 * hand out: `openUrl` and `openUrlOutsideApp` in `store/actions/artifacts.ts`,
 * and `openExternalUrl` in `utils/openExternalUrl.ts`.
 *
 * NOT the only scheme classifier, and do not merge it with the other one.
 * `stripDangerousUrlSchemes` (`utils/renderMarkdown.ts`) screens an HREF inside
 * rendered markdown, and denies a different set: it drops `data:`, which is a
 * legitimate thing to OPEN but never a legitimate link target. Widening either
 * to match the other breaks the case the difference exists for.
 */

/** Schemes no caller may open.
 *
 *  A DENY list, not an allow list. `utils/openExternalUrl.ts` documents
 *  `mailto:`, `tel:`, `file:` and `data:` as legitimate targets that must reach
 *  their own handlers, so narrowing to http(s) would break them. */
const DANGEROUS_URL_SCHEME_RE = /^(javascript|vbscript):/i;

/** Whether `url` reaches the sink as one of the schemes above.
 *
 *  Test what the SINK sees, not what the caller passed. `openUrl` runs the
 *  value through `new URL(...).href` first, and that parser strips every
 *  leading C0 CONTROL as well as whitespace, then drops tab and newline
 *  anywhere. So `javascript:alert(1)` reaches `window.open` as a bare
 *  `javascript:` URL.
 *
 *  JavaScript's `\s` is the wrong set for that. It covers U+0020 and the usual
 *  line breaks, but NOT U+0000 to U+0008 or U+000E to U+001F. Matching the
 *  parser means stripping all of U+0000 to U+0020, which is why the class below
 *  is a range rather than `\s`. `\s` stays beside it for the few characters
 *  above U+0020 it adds, which only widens the guard.
 *
 *  The test is anchored, so stripping cannot manufacture a match from a URL
 *  that merely contains the word. */
export function hasDangerousScheme(url: string): boolean {
  return DANGEROUS_URL_SCHEME_RE.test(url.replace(/[\u0000-\u0020\s]/g, ''));
}
