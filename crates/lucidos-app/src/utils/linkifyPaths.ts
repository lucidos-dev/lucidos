/**
 * Post-process rendered HTML to linkify artifact paths and bare URLs, and to
 * rewrite deliberate app / nav-panel / artifact anchors into click-routed links.
 * App names are NOT scanned in prose — an app becomes a link only when the LLM
 * writes an explicit markdown link to it. Tracks <a> and <code> nesting to avoid
 * nested anchors and linkifying code content.
 */

import { addLinkifyMs } from './renderPhaseTimers';

// Cap how many alternatives go into a single regex. WebKit's YARR throws
// "Invalid regular expression: regular expression too large" on big alternations;
// V8 has no such limit. With ~50 chars per escaped path, 500 entries → ~25 KB
// source — comfortably under every engine's threshold.
const REGEX_BATCH_SIZE = 500;

const REGEX_ESCAPE = /[.*+?^${}()|[\]\\]/g;

function buildBatchedPatterns(escaped: string[], wrap: (alt: string) => string): RegExp[] {
  const patterns: RegExp[] = [];
  for (let i = 0; i < escaped.length; i += REGEX_BATCH_SIZE) {
    const batch = escaped.slice(i, i + REGEX_BATCH_SIZE);
    patterns.push(new RegExp(wrap(batch.join('|')), 'g'));
  }
  return patterns;
}

type Match = { start: number; end: number; replacement: string };

function collectMatches(text: string, patterns: RegExp[], render: (m: string) => string): Match[] {
  const matches: Match[] = [];
  for (const pattern of patterns) {
    pattern.lastIndex = 0;
    let m: RegExpExecArray | null;
    while ((m = pattern.exec(text)) !== null) {
      matches.push({ start: m.index, end: m.index + m[0].length, replacement: render(m[0]) });
    }
  }
  // Same start → longest match wins (matches single-regex alternation behavior with
  // length-desc sorted alternatives). Earlier non-overlapping match wins overall.
  matches.sort((a, b) => a.start - b.start || (b.end - b.start) - (a.end - a.start));
  const filtered: Match[] = [];
  let cursor = 0;
  for (const m of matches) {
    if (m.start >= cursor) {
      filtered.push(m);
      cursor = m.end;
    }
  }
  return filtered;
}

function applyMatches(text: string, matches: Match[]): string {
  if (matches.length === 0) return text;
  let out = '';
  let pos = 0;
  for (const m of matches) {
    out += text.slice(pos, m.start) + m.replacement;
    pos = m.end;
  }
  out += text.slice(pos);
  return out;
}

const HREF_ATTR = /\shref\s*=\s*(?:"([^"]*)"|'([^']*)')/i;
const CLASS_ATTR = /\sclass\s*=\s*(?:"[^"]*"|'[^']*')/i;
const DATA_PATH_ATTR = /\sdata-path\s*=\s*(?:"[^"]*"|'[^']*')/i;

/** If the opening anchor tag points at a workspace data file, return a
 *  replacement opening tag that uses `class="artifact-link" data-path="…"` so
 *  the chat click handler routes the click through `openFilePreview` (content
 *  panel) instead of letting the browser navigate to the `/data/*` static mount
 *  as a top-level URL. Returns null when the href isn't a data file at all, and
 *  the caller leaves the segment untouched. Visible text inside the anchor is
 *  preserved verbatim; other attributes (`title`, `target`, `rel`, …) are kept.
 *
 *  Two ways to resolve, in this order:
 *
 *  1. **The known-paths lookup**, which also CANONICALIZES: it maps the bare
 *     form (`foo.md`) back to the full store path (`artifacts/foo.md`), which
 *     shape alone can't do. So it has to run first.
 *  2. **The shape** (`extractDataPathTarget`), for a path the cached artifact
 *     list doesn't know. That list is a projection refreshed by SSE, and a file
 *     the agent wrote seconds ago is routinely missing from it, so gating a
 *     DELIBERATE markdown link on it means concluding "not a file" from a stale
 *     cache. That is the failure `.claude/rules/frontend.md` names, and the one
 *     that reloaded the whole workspace on a fresh `lucidos data write`
 *     artifact. A path that really doesn't exist now dead-ends in the file
 *     preview's own 404, which is recoverable; a top-level navigation is not.
 *
 *  Shape-based resolution is deliberately limited to anchors. The text-segment
 *  linkifier below stays list-gated: it scans PROSE, where matching a path
 *  shape would linkify every incidental mention.
 *
 *  We force `href="#"` on the rewritten anchor instead of dropping href
 *  entirely. iOS Safari (and iOS PWA in standalone mode) treats `<a>` without
 *  an href as a non-interactive element — taps silently don't translate to
 *  `click` events, even with `cursor: pointer`, so the delegated chat click
 *  handler never fires and the user sees a dead link. `preventDefault` in
 *  the delegated handler suppresses the `#` scroll-to-top. */
function rewriteArtifactAnchor(
  tag: string,
  pathLookup: Map<string, string> | undefined,
): string | null {
  const m = tag.match(HREF_ATTR);
  if (!m) return null;
  const href = m[1] ?? m[2];
  if (!href) return null;
  let candidate = href;
  if (candidate.startsWith('/data/')) candidate = candidate.slice('/data/'.length);
  else if (candidate.startsWith('data/')) candidate = candidate.slice('data/'.length);
  const fullPath = pathLookup?.get(candidate) ?? extractDataPathTarget(href);
  if (!fullPath) return null;
  const escapedPath = fullPath.replace(/"/g, '&quot;');
  const stripped = tag
    .replace(HREF_ATTR, '')
    .replace(CLASS_ATTR, '')
    .replace(DATA_PATH_ATTR, '');
  return stripped.replace(/^<a/i, `<a href="#" class="artifact-link" data-path="${escapedPath}"`);
}

const DATA_APP_ID_ATTR = /\sdata-app-id\s*=\s*(?:"[^"]*"|'[^']*')/i;
const DATA_NAV_TARGET_ATTR = /\sdata-nav-target\s*=\s*(?:"[^"]*"|'[^']*')/i;

/** UI panels reachable from a markdown link like `[Notifications](notifications)`.
 *  Mirrors the side-drawer menu items — the same names the `navigate_ui` LLM
 *  tool accepts for the panel targets and that `handleNavigationRequest` routes.
 *  Most route via `switchMenuItem`; `app-store` is a retained alias that lands
 *  on the Plugins panel (with its full marketplace catalog showing). Kept in
 *  sync by hand: short and stable. */
const NAV_TARGETS: ReadonlySet<string> = new Set([
  'notifications',
  'apps',
  'app-store',
  'triggers',
  'changes',
  'files',
  'settings',
]);

/** Extract a UI-panel target from an href that points at a Lucidos navigation
 *  panel — meaning "open the inbox / apps list / triggers list / …", NOT
 *  "open a specific app or file inside it".
 *
 *  Accepted shapes (with optional `data/` or `/data/` prefix, trailing slash,
 *  query string, fragment):
 *    - `notifications`, `apps`, `triggers`, `changes`, `files`, `settings`
 *    - `data/notifications`, `/notifications`, `notifications/`, `notifications?x=1`
 *
 *  Rejected: any sub-path (`apps/<id>`, `notifications/foo`), external URLs,
 *  unknown panel names. Sub-paths fall through so the app rewriter and
 *  artifact rewriter keep claiming their own shapes.
 *
 *  Why this rewriter exists: the LLM naturally writes
 *  `[Notifications](data/notifications)` in chat replies, mirroring the
 *  `data/artifacts/<path>` and `data/apps/<id>/index.html` shapes it already
 *  knows produce clickable links. Left alone, the browser hits the engine's
 *  `/data/*` static mount and 404s on a folder that does not exist. The
 *  rewriter routes the click through `handleNavigationRequest` instead, which
 *  matches what the `navigate_ui` LLM tool does for the same target. */
export function extractNavTargetFromHref(href: string): string | null {
  let candidate = href;
  if (candidate.startsWith('/data/')) candidate = candidate.slice('/data/'.length);
  else if (candidate.startsWith('data/')) candidate = candidate.slice('data/'.length);
  if (candidate.startsWith('/')) candidate = candidate.slice(1);
  // Strip query string / fragment so `notifications?refresh=1` matches.
  const queryStart = candidate.search(/[?#]/);
  if (queryStart !== -1) candidate = candidate.slice(0, queryStart);
  if (candidate.endsWith('/')) candidate = candidate.slice(0, -1);
  if (!candidate) return null;
  // Sub-path → not a panel target. `apps/<id>` belongs to the app rewriter,
  // `notifications/foo` is meaningless (no such sub-path exists).
  if (candidate.includes('/')) return null;
  return NAV_TARGETS.has(candidate) ? candidate : null;
}

/** Does this href carry a URL scheme (`https:`, `mailto:`, `tel:`, `app:`,
 *  `file:`, `data:`, …)? The single source of truth for that question across
 *  every link router: the chat click handler's terminal guard, the preview
 *  iframe bridge, and the href extractors here all key off it, so a scheme is
 *  never claimed as a relative path in one place and left to the browser in
 *  another. Deliberately matches ANY scheme rather than a known set: the point
 *  is "not a relative path", and each caller decides what to do with the ones
 *  it owns. */
export function hasUrlScheme(href: string): boolean {
  return /^[a-z][a-z0-9+.-]*:/i.test(href);
}

/** The `data/` sub-trees a workspace file can live in. Single source of truth
 *  for both the href recognizer below and `normalizeDataPath` in
 *  `store/actions/artifacts.ts`, which prefixes anything unprefixed with
 *  `artifacts/`. `system-knowhow/` is engine-shipped and read-only, but it is
 *  served by the same `/data/*` mount and previews the same way, so a link to
 *  it must route in-app like any other. */
export const DATA_PATH_PREFIXES: readonly string[] = [
  'artifacts/',
  'knowhow/',
  'apps/',
  'triggers/',
  'system-knowhow/',
];

/** Recognize an href that points at a file under the workspace's `data/` tree,
 *  by SHAPE rather than by membership of the cached artifact list. Returns the
 *  normalized store path (`artifacts/report.html`), or null.
 *
 *  Accepted (with an optional `data/` / `/data/` prefix or a bare leading `/`,
 *  and with any query string / fragment stripped):
 *    - `artifacts/pr-review/pr-1582/index.html`, `data/knowhow/x/notes.md`
 *    - `/artifacts/report.pdf`, `apps/todo/styles.css`
 *
 *  Rejected:
 *    - any URL scheme (`https:`, `app:`, `file:`, `mailto:`): real links, or
 *      another extractor's job
 *    - a bare sub-tree name with nothing under it (`artifacts`, `artifacts/`,
 *      `apps`), which is a directory rather than a file. `apps` / `triggers`
 *      are also nav panel names, and the nav extractor runs first; this guard
 *      means the two can't fight even if that order ever changes.
 *    - anything outside the known sub-trees (`notifications`, `README`)
 *
 *  Why it exists: `lucidos data write` prints exactly this link shape for the
 *  agent to paste, and the artifact rewriter used to resolve it against the
 *  cached path list only. A file written moments ago is not in that cache, so
 *  the anchor stayed a plain relative href, the browser navigated to
 *  `/<slug>/artifacts/…`, and the SPA fallback served the app shell: the whole
 *  workspace reloaded. Both the render-time rewriter and the click handler use
 *  this so neither depends on cache warmth. */
export function extractDataPathTarget(href: string): string | null {
  // Any URL scheme disqualifies a data path, same test as extractBareAppRef.
  if (hasUrlScheme(href)) return null;
  let candidate = href;
  if (candidate.startsWith('/data/')) candidate = candidate.slice('/data/'.length);
  else if (candidate.startsWith('data/')) candidate = candidate.slice('data/'.length);
  else if (candidate.startsWith('/')) candidate = candidate.slice(1);
  const queryStart = candidate.search(/[?#]/);
  if (queryStart !== -1) candidate = candidate.slice(0, queryStart);
  const prefix = DATA_PATH_PREFIXES.find((p) => candidate.startsWith(p));
  if (!prefix) return null;
  // Nothing under the sub-tree, or a trailing slash: a directory, not a file.
  if (candidate.length === prefix.length || candidate.endsWith('/')) return null;
  return candidate;
}

/** Recognize an href that points at a REAL local filesystem location the OS
 *  should open directly — a `file://` URL or an absolute POSIX path
 *  (`/Users/…/foo.dmg`, `/Applications/…`). The release flow hands the user a
 *  link to a staged `.dmg` that lives OUTSIDE the workspace (under
 *  `~/projects/lucidos/.lucidos/release-worktrees/<version>/…`); clicking it
 *  must mount the image — or, for a folder, reveal it — via the OS, NOT route
 *  through the in-app file preview or the engine's `/data/*` static mount.
 *
 *  Returns the path/URL to hand to the OS opener, or null when the href is a
 *  workspace route or an external web URL:
 *    - `file://…`                 → that URL (always a local file/dir)
 *    - `/Users/…`, `/Applications/…`, any other absolute path → that path
 *    - `/data/…`, `/data`         → null (engine static mount, artifact/nav own it)
 *    - `/artifacts/…`, `/apps/…`, `/knowhow/…`, and the bare directory form of
 *      each → null (a workspace data route, never a disk path)
 *    - `notifications`, `data/x`, `apps/x` (relative) → null (not absolute)
 *    - `https://…`, `http://…`    → null (keep browser / panel-webview behavior)
 *
 *  This runs LAST in the click handler, after the app / nav / data-path
 *  extractors, so the absolute workspace routes they claim never reach here.
 *  The guards below are the belt to that braces: they keep an absolute data
 *  route out of the OS opener on their own, so reordering the handler can't
 *  turn `/artifacts/report.pdf` into a disk path. Derived from
 *  `DATA_PATH_PREFIXES` rather than spelled out, so a new `data/` sub-tree is
 *  covered here the moment it is added there. */
export function extractLocalFileTarget(href: string): string | null {
  // file:// URL — unambiguously a local file or directory.
  if (/^file:\/\//i.test(href)) return href;
  // Absolute POSIX path. Exclude the workspace's own absolute routes so a
  // `/data/…` or `/artifacts/…` link is never handed to the OS as a disk path.
  if (href.startsWith('/')) {
    return isWorkspaceAbsoluteRoute(href) ? null : href;
  }
  return null;
}

/** True when an absolute href addresses the workspace's own `data/` tree
 *  (the `/data/*` static mount, or one of its sub-trees reached without the
 *  `data/` segment) rather than a filesystem location. Matches both the
 *  sub-path form (`/artifacts/x.md`) and the bare directory (`/artifacts`). */
function isWorkspaceAbsoluteRoute(href: string): boolean {
  if (href === '/data' || href.startsWith('/data/')) return true;
  return DATA_PATH_PREFIXES.some((p) => {
    const bare = `/${p.slice(0, -1)}`;
    return href === bare || href.startsWith(`/${p}`);
  });
}

/** Mirror of `rewriteArtifactAnchor` / `rewriteAppAnchor` for navigation
 *  panels. Returns a replacement opening tag with
 *  `class="nav-link" data-nav-target="<target>"` so the chat click handler
 *  routes through `handleNavigationRequest({ target })`. Returns null when
 *  the href is not a panel name — caller falls through to the next rewriter
 *  (app, then artifact). */
function rewriteNavAnchor(tag: string): string | null {
  const m = tag.match(HREF_ATTR);
  if (!m) return null;
  const href = m[1] ?? m[2];
  if (!href) return null;
  const target = extractNavTargetFromHref(href);
  if (!target) return null;
  const escapedTarget = target.replace(/"/g, '&quot;');
  const stripped = tag
    .replace(HREF_ATTR, '')
    .replace(CLASS_ATTR, '')
    .replace(DATA_NAV_TARGET_ATTR, '');
  return stripped.replace(/^<a/i, `<a href="#" class="nav-link" data-nav-target="${escapedTarget}"`);
}

/** Extract a Lucidos app id from an href that points at an app's ENTRY
 *  POINT — meaning "open this app", not "preview a file inside it".
 *  Accepted shapes (with optional `data/` or `/data/` prefix):
 *    - `apps/<id>`            (folder, no sub-path)
 *    - `apps/<id>/`           (folder, trailing slash)
 *    - `apps/<id>/index.html` (canonical entry file)
 *    - `app:<id>`             (custom-scheme shorthand, with optional `/`)
 *
 *  Rejected: any other sub-path (`apps/<id>/scripts/run.sh`,
 *  `apps/<id>/styles.css`, `app:<id>/anything`, etc.) — those are real
 *  files (or have no defined meaning under the `app:` scheme) and should
 *  fall through. Also rejected: external URLs
 *  (`https://example.com/apps/...`), fragments, mailto, lookalike schemes
 *  (`apple:`, `application:`), etc.
 *
 *  Why entry-points-only: `lucidos.data.list()` returns EVERY file under
 *  `data/`, so the artifact path list always contains
 *  `apps/<id>/index.html` for every app. A permissive extractor would have
 *  the app rewriter pre-empt the artifact rewriter for sub-files the user
 *  actually wants to preview. The entry-point gate keeps the app rewriter
 *  narrow enough to coexist with the artifact rewriter.
 *
 *  Why the `app:<id>` scheme: LLMs invent it by analogy to the documented
 *  `thread:<UUID>` scheme — bug report was `[Habit Tracker app](app:habit-tracker)`
 *  rendered as `<a href="app:habit-tracker">` that dead-ended on macOS Chrome
 *  because no handler claimed the unknown URL scheme. Accepting it here
 *  routes the click through `openApp` the same way the long form does.
 *
 *  Query strings and fragments on entry-point hrefs are stripped before
 *  matching — `apps/<id>/index.html?v=2` is still an entry point.
 *
 *  Exported because two layers need it: `rewriteAppAnchor` (anchor
 *  rewriter, runs at render time) and the chat click handler
 *  (defense-in-depth — fires for unrewritten anchors). */
export function extractAppIdFromHref(href: string): string | null {
  // `app:<id>` custom-scheme branch. Match before the path-based normalization
  // below so the leading-slash strip can't mangle it.
  if (href.startsWith('app:')) {
    const rest = href.slice('app:'.length);
    if (rest.length === 0) return null;
    // Strip query / fragment first so `app:todo?refresh=1` resolves to `todo`.
    const queryStart = rest.search(/[?#]/);
    const trimmed = queryStart === -1 ? rest : rest.slice(0, queryStart);
    if (trimmed.length === 0) return null;
    const slash = trimmed.indexOf('/');
    if (slash === -1) return trimmed;                   // `app:<id>`
    const appId = trimmed.slice(0, slash);
    const afterSlash = trimmed.slice(slash + 1);
    if (afterSlash.length === 0) return appId || null;  // `app:<id>/`
    return null;                                        // `app:<id>/sub` → no meaning
  }
  let candidate = href;
  if (candidate.startsWith('/data/')) candidate = candidate.slice('/data/'.length);
  else if (candidate.startsWith('data/')) candidate = candidate.slice('data/'.length);
  if (candidate.startsWith('/')) candidate = candidate.slice(1);
  if (!candidate.startsWith('apps/')) return null;
  const rest = candidate.slice('apps/'.length);
  if (rest.length === 0) return null;
  // Strip query string / fragment so they don't bleed into id comparison.
  const queryStart = rest.search(/[?#]/);
  const trimmed = queryStart === -1 ? rest : rest.slice(0, queryStart);
  if (trimmed.length === 0) return null;
  const slash = trimmed.indexOf('/');
  if (slash === -1) return trimmed;                     // `apps/<id>`
  const appId = trimmed.slice(0, slash);
  const afterSlash = trimmed.slice(slash + 1);
  if (afterSlash.length === 0) return appId || null;    // `apps/<id>/`
  if (afterSlash === 'index.html') return appId || null;// `apps/<id>/index.html`
  return null;                                          // sub-file → artifact
}

/** Extract a BARE app reference from an href — a single path segment with no
 *  URL scheme and no sub-path, e.g. `habit-tracker` or `Habit Tracker`. The
 *  caller resolves the returned token against the known app ids/names; this
 *  function only normalizes the shape and rejects everything that clearly
 *  isn't a bare reference.
 *
 *  Why it exists: the LLM writes `[Habit Tracker](habit-tracker)` — the app id
 *  (or name) as a bare relative href — by analogy to `[Notifications](notifications)`.
 *  That href matches NONE of the strict shapes: no `apps/` prefix, no `app:`
 *  scheme (so `extractAppIdFromHref` declines), and it isn't a nav panel. Left
 *  alone the browser resolves the relative href against the base href to a
 *  non-existent route; the engine's SPA fallback then serves the shell and the
 *  whole workspace reloads (the "Opening workspace" splash — very visible on an
 *  iOS PWA). Routing the click through `openApp` instead is the fix.
 *
 *  Returns null (not a bare candidate) when the href:
 *    - carries a URL scheme (`http:`, `app:`, `mailto:`, `file:`, …) — `app:<id>`
 *      is `extractAppIdFromHref`'s job; the rest are real links.
 *    - has a slash beyond an optional leading/trailing one (`apps/<id>`,
 *      `foo/bar`) — a sub-path, owned by the app / artifact rewriters.
 *    - is empty.
 *
 *  Accepts an optional `data/`-less single token with a query/fragment
 *  (`habit-tracker?v=2`, `habit-tracker#top`) — stripped before returning.
 *
 *  The returned token is percent-DECODED: markdown renders an app-name
 *  destination that contains spaces/special chars encoded — `[x](<Habit Tracker>)`
 *  and `[x](Habit%20Tracker)` both render as `href="Habit%20Tracker"` — so the
 *  raw href must be decoded back to `Habit Tracker` to match the app name at
 *  lookup. (App IDs are slugs, so decoding is a no-op for the id case.) */
export function extractBareAppRef(href: string): string | null {
  // Any URL scheme disqualifies a bare ref. `app:<id>` is handled upstream by
  // extractAppIdFromHref; `http(s):`, `mailto:`, `tel:`, `file:` are real links.
  if (hasUrlScheme(href)) return null;
  let candidate = href;
  if (candidate.startsWith('/')) candidate = candidate.slice(1);
  const queryStart = candidate.search(/[?#]/);
  if (queryStart !== -1) candidate = candidate.slice(0, queryStart);
  if (candidate.endsWith('/')) candidate = candidate.slice(0, -1);
  if (!candidate) return null;
  if (candidate.includes('/')) return null; // sub-path → not a bare ref
  // Decode so a percent-encoded app name (`Habit%20Tracker`) matches the raw
  // name. A malformed escape (a bare `%` not starting a valid sequence) throws
  // — keep the raw token in that case rather than dropping the ref.
  try { return decodeURIComponent(candidate); } catch { return candidate; }
}

/** Mirror of `rewriteArtifactAnchor` for apps. LLMs naturally write
 *  `[Name](apps/<id>/index.html)` to link to an app — pulldown_cmark renders
 *  that as a plain `<a href="apps/<id>/index.html">` which, left alone, lets
 *  the browser navigate to the file under the `/data/*` static mount (file
 *  preview), NOT the running app. Reject hrefs whose `<id>` isn't a known
 *  app so unrelated `apps/...` URLs (e.g. external sites or stale
 *  references) stay alone. */
function rewriteAppAnchor(tag: string, appIds: Set<string>): string | null {
  const m = tag.match(HREF_ATTR);
  if (!m) return null;
  const href = m[1] ?? m[2];
  if (!href) return null;
  const appId = extractAppIdFromHref(href);
  if (!appId || !appIds.has(appId)) return null;
  const escapedId = appId.replace(/"/g, '&quot;');
  const stripped = tag
    .replace(HREF_ATTR, '')
    .replace(CLASS_ATTR, '')
    .replace(DATA_APP_ID_ATTR, '');
  return stripped.replace(/^<a/i, `<a href="#" class="app-link" data-app-id="${escapedId}"`);
}

/** Bare "open the app" hrefs the LLM emits when it knows the app name should be
 *  a link but supplies no id — `[Site Publisher](app)`, `(app/)`, `(app:)`.
 *  These match neither the `app:<id>` / `apps/<id>` shapes (no id) nor a nav
 *  panel (`app` singular isn't one — `apps` plural is), so without recovery they
 *  render as a raw relative `<a href="app">` that the browser resolves against
 *  the gateway base (`/<slug>/`) to `/<slug>/app`, a dead end.
 *
 *  TEMPORARY MEASURE — model-tolerance (removable; see docs/temporary-measures.md
 *  § "Bare `app` href recovery", governed by .claude/rules/temporary-measures.md).
 *  Drop once the agent reliably emits `app:<id>` links carrying an id. */
const BARE_APP_HREF = /^app:?\/?$/i;

/** Plain-text content between an opening `<a>` tag at `openTagIndex` and its
 *  matching `</a>` in the alternating tag/text `segments`. Nested tags are
 *  skipped so `<a href="app"><strong>Site Publisher</strong></a>` still yields
 *  "Site Publisher". */
function anchorText(segments: string[], openTagIndex: number): string {
  let text = '';
  for (let j = openTagIndex + 1; j < segments.length; j++) {
    if (j % 2 === 1) {
      if (segments[j].toLowerCase() === '</a>') break;
      continue; // nested tag — skip, keep gathering text
    }
    text += segments[j];
  }
  return text;
}

/** Last-resort app-anchor rewriter for bare `app` hrefs (see `BARE_APP_HREF`):
 *  resolve the app from the anchor's visible TEXT instead of its href, since the
 *  href carries no id. Runs only after the strict href-based rewriters decline,
 *  so a real `apps/<id>` / nav / artifact link is never hijacked. Returns null
 *  when the href isn't a bare-app shape or the text names no known app. */
function rewriteBareAppAnchorByText(
  tag: string,
  linkText: string,
  appTextToId: Map<string, string>,
): string | null {
  const m = tag.match(HREF_ATTR);
  if (!m) return null;
  const href = m[1] ?? m[2];
  if (!href || !BARE_APP_HREF.test(href.trim())) return null;
  const id = appTextToId.get(linkText.trim());
  if (!id) return null;
  const escapedId = id.replace(/"/g, '&quot;');
  const stripped = tag
    .replace(HREF_ATTR, '')
    .replace(CLASS_ATTR, '')
    .replace(DATA_APP_ID_ATTR, '');
  return stripped.replace(/^<a/i, `<a href="#" class="app-link" data-app-id="${escapedId}"`);
}

/** Rewrite a bare app-id/name href (see `extractBareAppRef`) to an app-link.
 *  Resolves the token against BOTH app ids and names — `appTextToId` maps each
 *  form to its id, so `habit-tracker` and `Habit Tracker` both resolve. Runs AFTER
 *  the strict `apps/<id>`, nav, and artifact rewriters so a reserved nav panel
 *  (`apps`, `notifications`, …) or a real artifact path always wins, and this
 *  only claims an href none of them wanted. Returns null when the href isn't a
 *  bare ref or names no known app. */
function rewriteAppAnchorByBareRef(tag: string, appTextToId: Map<string, string>): string | null {
  const m = tag.match(HREF_ATTR);
  if (!m) return null;
  const href = m[1] ?? m[2];
  if (!href) return null;
  const token = extractBareAppRef(href);
  if (!token) return null;
  const id = appTextToId.get(token);
  if (!id) return null;
  const escapedId = id.replace(/"/g, '&quot;');
  const stripped = tag
    .replace(HREF_ATTR, '')
    .replace(CLASS_ATTR, '')
    .replace(DATA_APP_ID_ATTR, '');
  return stripped.replace(/^<a/i, `<a href="#" class="app-link" data-app-id="${escapedId}"`);
}

/** The invariant regex batches + lookups linkify needs for a given (paths, apps)
 *  set. Building these is the bulk of a linkify call, but they don't depend on the
 *  html — so they're built once per (paths, apps) and reused across every block /
 *  exchange in a render (see `getCompiled`). */
interface CompiledLinkify {
  pathPatterns: RegExp[];
  pathLookup: Map<string, string> | undefined;
  appTextToId: Map<string, string> | undefined;
  appIds: Set<string> | undefined;
}

function buildCompiled(
  paths: string[],
  apps: { name: string; id: string }[],
): CompiledLinkify {
  let pathPatterns: RegExp[] = [];
  // Single map keyed by every form a path may appear in (full path AND bare —
  // i.e. with the `artifacts/` prefix stripped, since LLMs sometimes write
  // bare paths). Values are always the full store path, so both the
  // text-segment linkifier (bare match → full data-path) and the anchor
  // rewriter (href → full data-path) hand the click handler a canonical path.
  let pathLookup: Map<string, string> | undefined;
  if (paths.length > 0) {
    pathLookup = new Map();
    for (const p of paths) {
      pathLookup.set(p, p);
      if (p.startsWith('artifacts/')) {
        pathLookup.set(p.slice('artifacts/'.length), p);
      }
    }
    const allMatchable = [...pathLookup.keys()];
    allMatchable.sort((a, b) => b.length - a.length);
    const escaped = allMatchable.map((p) => p.replace(REGEX_ESCAPE, '\\$&'));
    pathPatterns = buildBatchedPatterns(escaped, (alt) => `(${alt})`);
  }

  // Map each app name AND id to its id so the anchor rewriters can resolve a
  // deliberate `[X](app:x)` / `[X](habit-tracker)` link the LLM wrote. Bare-text
  // app-name scanning was removed — an app named in prose is NOT auto-linked (it
  // was unreliable, a blind `\b(name)\b` match that linkified every mention of a
  // generically-named app). Apps become links only via an explicit markdown link.
  let appTextToId: Map<string, string> | undefined;
  let appIds: Set<string> | undefined;
  if (apps.length > 0) {
    appTextToId = new Map();
    for (const s of apps) {
      if (!appTextToId.has(s.name)) appTextToId.set(s.name, s.id);
      if (s.id !== s.name && !appTextToId.has(s.id)) appTextToId.set(s.id, s.id);
    }
    appIds = new Set(apps.map((s) => s.id));
  }

  return { pathPatterns, pathLookup, appTextToId, appIds };
}

// Build the invariant patterns ONCE per (paths, apps) set instead of once per
// call. Every exchange in a render passes the SAME `artifactPaths`/`apps` array
// references (loadedOr returns the signal's `.data` or a stable NO_* constant),
// so a 1-entry by-reference memo hits for every call after the first — and across
// renders until the artifact/app list changes. `generation` bumps on each rebuild
// and folds into the output-cache key below so a list change invalidates output.
let cachedPaths: string[] | null = null;
let cachedApps: { name: string; id: string }[] | null = null;
let cachedCompiled: CompiledLinkify | null = null;
let generation = 0;

function getCompiled(paths: string[], apps: { name: string; id: string }[]): CompiledLinkify {
  if (paths === cachedPaths && apps === cachedApps && cachedCompiled) return cachedCompiled;
  cachedCompiled = buildCompiled(paths, apps);
  cachedPaths = paths;
  cachedApps = apps;
  generation++;
  return cachedCompiled;
}

// Output LRU cache — mirrors renderMarkdown's. linkify output is a pure function
// of (html, paths, apps); keyed on `${generation}\0${html}` so a (paths, apps)
// change (new generation) invalidates. A hit is O(1) — this is what makes a
// re-render (which busts the per-exchange useMemo in ChatExchange and re-linkifies
// every block) cheap instead of the hundreds of ms the profile measured.
const LINKIFY_CACHE_MAX = 400;
const linkifyCache = new Map<string, string>();

/** Test-only: clear the output cache + compiled memo so module-level state can't
 *  leak between tests. Not part of the runtime surface. */
export function _resetLinkifyCacheForTesting(): void {
  linkifyCache.clear();
  cachedPaths = null;
  cachedApps = null;
  cachedCompiled = null;
  generation = 0;
}

/** Post-process rendered HTML to linkify artifact paths and bare URLs, and rewrite
 *  deliberate app / nav / artifact anchors (app names are not scanned in prose).
 *  Pure in (html, paths, apps); LRU-cached so a re-render that re-invokes it with
 *  unchanged content is O(1). `opts.cache: false` opts out (used for the live
 *  streaming buffer, whose html changes every token — caching it would only thrash
 *  the LRU). try/finally records elapsed linkify time for the perf phase split
 *  (utils/renderPhaseTimers.ts) and re-throws unchanged. */
export function linkifyPaths(
  html: string,
  paths: string[],
  apps: { name: string; id: string }[],
  opts?: { cache?: boolean },
): string {
  const start = performance.now();
  try {
    const useCache = opts?.cache !== false;
    // Build/reuse the patterns first — this may bump `generation`, which the
    // cache key below must reflect.
    const compiled = getCompiled(paths, apps);
    const key = useCache ? `${generation}\0${html}` : '';
    if (useCache) {
      const hit = linkifyCache.get(key);
      if (hit !== undefined) {
        // LRU touch: move to most-recently-used.
        linkifyCache.delete(key);
        linkifyCache.set(key, hit);
        return hit;
      }
    }
    const out = applyCompiled(html, compiled);
    if (useCache) {
      linkifyCache.set(key, out);
      if (linkifyCache.size > LINKIFY_CACHE_MAX) {
        // Evict the least-recently-used (first key in insertion order).
        const oldest = linkifyCache.keys().next().value;
        if (oldest !== undefined) linkifyCache.delete(oldest);
      }
    }
    return out;
  } finally {
    addLinkifyMs(performance.now() - start);
  }
}

function applyCompiled(html: string, compiled: CompiledLinkify): string {
  const { pathPatterns, pathLookup, appTextToId, appIds } = compiled;
  const segments = html.split(/(<[^>]+>)/);

  const urlPattern = /https?:\/\/[^\s<>"')\]]+/g;

  // Track tag nesting to skip content inside <a> (prevents nested anchors)
  // and <code> (code content should not be linkified).
  let insideAnchor = 0;
  let insideCode = 0;

  for (let i = 0; i < segments.length; i++) {
    if (i % 2 === 1) {
      // Tag segment — update nesting counters
      const tag = segments[i].toLowerCase();
      if (tag.startsWith('<a ') || tag === '<a>') {
        insideAnchor++;
        // Rewriter order: app → nav → artifact.
        //
        // App MUST run before artifact, and this is now load-bearing rather
        // than merely prudent. It used to hold because the path list from
        // lucidos.data.list() contains every app's index.html, so a permissive
        // artifact rewriter would claim `apps/<id>/index.html` and the click
        // would land on openFilePreview instead of the running app (the
        // "Link to <app> opens file preview" bug). The artifact rewriter now
        // resolves by SHAPE, so it claims `apps/<id>/index.html` whether or not
        // the list is loaded: only running the strict app rewriter first keeps
        // an app entry point opening the app. App stays strict (entry points
        // only), so sub-files like apps/<id>/scripts/run.sh fall through to the
        // artifact rewriter for the expected file-preview behavior.
        //
        // Nav matches only bare panel names (`notifications`, `apps`, …) with
        // an optional `data/` prefix and no slash beyond it. It still can't
        // collide with either neighbour: the app rewriter requires
        // `apps/<id>...`, and the artifact rewriter requires a non-empty
        // remainder after a `data/` sub-tree prefix, so a bare `apps` or
        // `triggers` is a directory to it and declined. Nav's position is
        // therefore still free; we put it between them for narrative clarity.
        let rewritten: string | null = null;
        if (appIds) rewritten = rewriteAppAnchor(segments[i], appIds);
        if (!rewritten) rewritten = rewriteNavAnchor(segments[i]);
        // Unconditional, NOT gated on `pathLookup`: the rewriter now also
        // resolves a data path by shape, which is exactly what a workspace with
        // an empty / not-yet-loaded artifact list needs.
        if (!rewritten) rewritten = rewriteArtifactAnchor(segments[i], pathLookup);
        // A bare app-id/name href — `[Habit Tracker](habit-tracker)` — that the
        // strict rewriters and nav declined. Resolve it from the HREF against
        // the known app ids/names. Without this the browser navigates to the
        // relative href and the SPA fallback reloads the whole workspace.
        if (!rewritten && appTextToId) {
          rewritten = rewriteAppAnchorByBareRef(segments[i], appTextToId);
        }
        // Last resort: a bare `app` href (no id) that the strict rewriters
        // declined and that isn't the `apps` nav panel — resolve from the
        // anchor's visible text. Covers `[Site Publisher](app)`, which would
        // otherwise dead-end at `/<slug>/app`.
        if (!rewritten && appTextToId) {
          rewritten = rewriteBareAppAnchorByText(segments[i], anchorText(segments, i), appTextToId);
        }
        if (rewritten) segments[i] = rewritten;
      }
      else if (tag === '</a>') insideAnchor = Math.max(0, insideAnchor - 1);
      else if (tag === '<code>' || tag.startsWith('<code ')) insideCode++;
      else if (tag === '</code>') insideCode = Math.max(0, insideCode - 1);
      continue;
    }

    // Text segment — artifact paths are linkified even inside <code> (LLMs wrap paths in backticks).
    // URLs are skipped inside <code> to avoid mangling code content. App names are
    // NOT scanned in text at all — apps only linkify via an explicit anchor above.

    if (insideAnchor === 0 && pathPatterns.length > 0) {
      const matches = collectMatches(segments[i], pathPatterns, (match) => {
        const fullPath = pathLookup!.get(match)!;
        const escapedPath = fullPath.replace(/"/g, '&quot;');
        // href="#" — see rewriteArtifactAnchor: iOS Safari/PWA needs href for
        // tap→click translation; preventDefault in the delegated chat handler
        // suppresses the scroll-to-top.
        return `<a href="#" class="artifact-link" data-path="${escapedPath}">${match}</a>`;
      });
      segments[i] = applyMatches(segments[i], matches);
    }

    if (insideCode > 0) continue;

    if (insideAnchor === 0) {
      urlPattern.lastIndex = 0;
      segments[i] = segments[i].replace(urlPattern, (match) => {
        return `<a href="${match}" target="_blank" rel="noopener">${match}</a>`;
      });
    }
  }

  return segments.join('');
}
