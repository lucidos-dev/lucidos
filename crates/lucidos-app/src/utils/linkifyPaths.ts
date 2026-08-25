/**
 * Post-process rendered HTML to linkify artifact paths and bare URLs, and to
 * rewrite deliberate app / nav-panel / artifact anchors into click-routed
 * links. App names are NOT scanned in prose: an app becomes a link only when
 * the LLM writes an explicit markdown link to it. Tracks <a> and <code>
 * nesting to avoid nested anchors and linkifying code content.
 */

import { addLinkifyMs } from './renderPhaseTimers';

// Cap how many alternatives go into a single regex. WebKit's YARR throws
// "regular expression too large" on big alternations; V8 has no such limit.
// With ~50 chars per escaped path, 500 entries is ~25 KB of source.
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

/** Gather every candidate span, then keep the ones that survive precedence.
 *  `render` returns null to DECLINE a candidate, which the shape matcher below
 *  needs: its regex recognizes a path-shaped token, and only the resolver can
 *  say whether that token names a file. A declined candidate is dropped before
 *  precedence runs, so it cannot mask a real match beneath it. */
function collectMatches(
  text: string,
  patterns: RegExp[],
  render: (m: string) => string | null,
  matches: Match[] = [],
): Match[] {
  for (const pattern of patterns) {
    pattern.lastIndex = 0;
    let m: RegExpExecArray | null;
    while ((m = pattern.exec(text)) !== null) {
      const replacement = render(m[0]);
      if (replacement === null) continue;
      matches.push({ start: m.index, end: m.index + m[0].length, replacement });
    }
  }
  return matches;
}

/** Resolve overlaps across every candidate, whatever matcher produced it.
 *  Same start: longest wins, matching a single regex alternation over
 *  length-desc alternatives. Earlier non-overlapping match wins overall. */
function resolveMatches(matches: Match[]): Match[] {
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
 *  replacement opening tag using `class="artifact-link" data-path="..."`, so
 *  the chat click handler routes through `openFilePreview` instead of letting
 *  the browser navigate to the `/data/*` static mount. Returns null when the
 *  href is not a data file. Visible text and other attributes are preserved.
 *
 *  Two ways to resolve, in this order. The known-paths lookup runs FIRST
 *  because it also canonicalizes, mapping the bare `foo.md` back to
 *  `artifacts/foo.md`, which shape alone cannot do. Then the shape
 *  (`extractDataPathTarget`), for a path the cached artifact list does not
 *  know. That list is a projection refreshed by SSE, so gating a DELIBERATE
 *  markdown link on it concludes "not a file" from a stale cache.
 *
 *  The text-segment linkifier resolves by shape too, via `DATA_PATH_IN_PROSE`,
 *  which adds one guard an anchor does not need: a final segment carrying an
 *  extension.
 *
 *  `href="#"` is forced rather than dropped: iOS Safari treats an `<a>` with
 *  no href as non-interactive, so taps never become `click` events. */
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
const DATA_TRIGGER_ID_ATTR = /\sdata-trigger-id\s*=\s*(?:"[^"]*"|'[^']*')/i;

/** UI panels reachable from a markdown link like `[Notifications](notifications)`.
 *  Mirrors the side-drawer menu items, the same names the `navigate_ui` LLM
 *  tool accepts and `handleNavigationRequest` routes. Most route via
 *  `switchMenuItem`; `app-store` is an alias landing on the Plugins panel.
 *  Kept in sync by hand: short and stable. */
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
 *  panel, meaning "open the inbox / apps list / triggers list", NOT "open a
 *  specific app or file inside it".
 *
 *  Accepted with an optional `data/` or `/data/` prefix, trailing slash, query
 *  string or fragment. Rejected: any sub-path (`apps/<id>`), external URLs and
 *  unknown panel names. Sub-paths fall through so the app rewriter and the
 *  artifact rewriter keep claiming their own shapes.
 *
 *  The LLM naturally writes `[Notifications](data/notifications)`, mirroring
 *  the `data/artifacts/<path>` shape it knows produces clickable links. Left
 *  alone the browser hits the engine's `/data/*` static mount and 404s on a
 *  folder that does not exist. */
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

/** Does this href carry a URL scheme (`https:`, `mailto:`, `app:`, `file:`)?
 *  The single source of truth for that question across every link router. A
 *  scheme is never claimed as a relative path in one place and left to the
 *  browser in another. Matches ANY scheme rather than a known set: the point
 *  is "not a relative path", and each caller decides what to do with the ones
 *  it owns. */
export function hasUrlScheme(href: string): boolean {
  return /^[a-z][a-z0-9+.-]*:/i.test(href);
}

/** The schemes a click may be handed to the browser for. `http` and `https` are
 *  ordinary links; `mailto`, `tel` and `sms` hand off to the OS and are
 *  universally claimed. */
const BROWSER_NAVIGABLE_SCHEMES: ReadonlySet<string> =
  new Set(['http', 'https', 'mailto', 'tel', 'sms']);

/** Can the browser actually DO something with this href?
 *
 *  Narrower than `hasUrlScheme`, and the difference is the point. A scheme no
 *  handler claims does nothing at all when clicked. A link that does nothing
 *  reads as a broken app rather than a broken link. ADR 0048 makes that
 *  argument about a `lucidos://` deep link, and it holds for an href the agent
 *  invents inside a message: `trigger:<id>` was exactly this, silent for as
 *  long as nothing claimed it.
 *
 *  So this is what the terminal guard tests, rather than "has a scheme". The
 *  app's OWN schemes never reach it: `app:`, `trigger:`, `repo:` and `file:`
 *  are claimed by their extractors first.
 *
 *  A legitimate third-party scheme (`vscode:`, `zoommtg:`) is swallowed by
 *  this, deliberately. Add it here if one turns out to be worth supporting. */
export function browserHandlesHref(href: string): boolean {
  if (!hasUrlScheme(href)) return false;
  return BROWSER_NAVIGABLE_SCHEMES.has(href.slice(0, href.indexOf(':')).toLowerCase());
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

/** A bare data path written in PROSE, recognized by shape. Group 1 is a leading
 *  boundary character the link must not swallow; group 2 is the path.
 *
 *  The agent is INSTRUCTED to write bare full paths, because the chat system
 *  prompt promises one becomes a link. ADR 0038 limited shape resolution to
 *  anchors, which broke that promise for a file the cached list had not caught
 *  up with. It is amended, and the plan it links carries the rationale.
 *
 *  Stricter than `extractDataPathTarget` in one way, since prose is not a
 *  deliberate anchor: the final segment must carry an extension. That keeps a
 *  folder mentioned in passing (`artifacts/marketing`) plain.
 *
 *  Three details carry weight, and `collectProseMatches` adds two more guards
 *  the regex cannot express. The boundary rejects a preceding word character or
 *  `/`, which stops `https://example.com/artifacts/x.md` being carved up
 *  mid-URL. The charset excludes `<>"'&` and whitespace, so an HTML entity
 *  terminates the match. The extension is alphanumeric, so
 *  `see artifacts/notes.md.` links the path and leaves the full stop.
 *
 *  No lookbehind: an older WebKit cannot transpile one. */
const DATA_PATH_IN_PROSE = new RegExp(
  '(^|[^\\w/])'
  + '(\\/?(?:data\\/)?'
  // Escaped like every other interpolated path in this file: a future prefix
  // carrying a regex metacharacter would otherwise change what this matches.
  + `(?:${DATA_PATH_PREFIXES.map((p) => p.slice(0, -1).replace(REGEX_ESCAPE, '\\$&')).join('|')})`
  + '(?:\\/[^\\s<>"\'&/]+)*'
  + '\\/[^\\s<>"\'&/]*[^\\s<>"\'&/.]\\.[A-Za-z0-9]+)',
  'g',
);

/** A character that would CONTINUE the filename past the extension the regex
 *  stopped at. Checked after the match rather than as a trailing `(?!…)`,
 *  because a lookahead sitting after a greedy `+` does not forbid the shape: it
 *  makes the engine give characters back until the lookahead passes. That
 *  turned `artifacts/archive.tar.zst-1` into a link to `archive.tar.zs`, which
 *  is worse than the truncation it was added to prevent.
 *
 *  `?` and `#` are deliberately absent, so `artifacts/report.html?v=2` still
 *  links its base path. */
const FILENAME_CONTINUES = /[-_~+%]/;

const SCHEME_CHAR = /[A-Za-z0-9+.-]/;
const SCHEME_FIRST_CHAR = /[A-Za-z]/;

/** Does a URL scheme end at `colonIndex`, as in `file:artifacts/x.pdf`?
 *
 *  The prose boundary accepts `:`, so without this the workspace half of a
 *  scheme URL becomes an artifact link. `hasUrlScheme` owns the question "is
 *  this a relative path", and it would reject the same string as an href. The
 *  prose matcher must not claim what an anchor declines.
 *
 *  Walks back over the scheme's own characters instead of slicing the text
 *  before the match. The slice was O(n) per match, which went quadratic on a
 *  segment whose every boundary is a colon. These runs are disjoint between
 *  matches, so the walk stays linear over the segment. Same grammar as
 *  `hasUrlScheme`: one letter, then letters, digits, `+`, `.` or `-`. */
function schemeEndsAt(text: string, colonIndex: number): boolean {
  let i = colonIndex - 1;
  while (i >= 0 && SCHEME_CHAR.test(text[i])) i--;
  return i + 1 < colonIndex && SCHEME_FIRST_CHAR.test(text[i + 1]);
}

/** A bare URL in a text segment. Shared by the URL linkifier and by
 *  `overlapsUrl`, so both agree on where a URL starts and ends. */
const URL_IN_TEXT = /https?:\/\/[^\s<>"')\]]+/g;

/** Spans of every bare URL in `text`. */
function urlSpans(text: string): Array<[number, number]> {
  const spans: Array<[number, number]> = [];
  URL_IN_TEXT.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = URL_IN_TEXT.exec(text)) !== null) spans.push([m.index, m.index + m[0].length]);
  return spans;
}

/** Does a path candidate sit inside a URL?
 *
 *  A workspace-shaped query or fragment value belongs to its URL, not to the
 *  workspace, and `=`, `?` and `#` are all legal prose boundaries. So
 *  `https://x/?next=artifacts/foo.md` offers a candidate the matcher would
 *  otherwise take.
 *
 *  In prose the markdown autolinker has already wrapped the URL, so the anchor
 *  guard covers it. Inside `<code>` it has not, and the URL pass is skipped
 *  there. Without this, the path pass splices an anchor into the middle of a
 *  shell command the reader is meant to copy. */
function overlapsUrl(spans: Array<[number, number]>, start: number, end: number): boolean {
  return spans.some(([from, to]) => start < to && end > from);
}

/** The anchor both text matchers emit. `href="#"` is required: see
 *  `rewriteArtifactAnchor`. `text` is the path as WRITTEN, already HTML-escaped
 *  by the markdown render; `path` is the canonical store path to open. */
function artifactLink(text: string, path: string): string {
  const escapedPath = path.replace(/"/g, '&quot;');
  return `<a href="#" class="artifact-link" data-path="${escapedPath}">${text}</a>`;
}

/** Collect the prose-shape candidates. Separate from `collectMatches` because
 *  the boundary group sits INSIDE the match and must be excluded from the span
 *  the link covers. */
function collectProseMatches(
  text: string,
  render: (m: string) => string | null,
  matches: Match[],
): void {
  DATA_PATH_IN_PROSE.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = DATA_PATH_IN_PROSE.exec(text)) !== null) {
    const start = m.index + m[1].length;
    const end = start + m[2].length;
    // Only a `:` boundary can carry a scheme, so nothing else pays for the walk.
    if (m[1] === ':' && schemeEndsAt(text, start - 1)) continue;
    if (FILENAME_CONTINUES.test(text.charAt(end))) continue;
    const replacement = render(m[2]);
    if (replacement === null) continue;
    matches.push({ start, end, replacement });
  }
}

/** Recognize an href that points at a file under the workspace's `data/` tree,
 *  by SHAPE rather than by membership of the cached artifact list. Returns the
 *  normalized store path (`artifacts/report.html`), or null.
 *
 *  Accepts an optional `data/` / `/data/` prefix or a bare leading `/`, and
 *  strips any query string or fragment. Rejected:
 *    - any URL scheme (`https:`, `app:`, `file:`): real links, or another
 *      extractor's job
 *    - a bare sub-tree name with nothing under it (`artifacts`, `apps`), which
 *      is a directory rather than a file. `apps` / `triggers` are also nav
 *      panel names, so this guard keeps the two from fighting whatever order
 *      the extractors run in.
 *    - anything outside the known sub-trees
 *
 *  Both the render-time rewriter and the click handler use this, so neither
 *  depends on cache warmth. A file written moments ago is not in the cached
 *  list, and a plain relative href navigates the SPA fallback, reloading the
 *  whole workspace. */
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
 *  should open directly: a `file://` URL or an absolute POSIX path. The
 *  release flow hands the user a link to a staged `.dmg` living OUTSIDE the
 *  workspace. Clicking it must mount the image via the OS, not route through
 *  the in-app preview or the engine's `/data/*` static mount.
 *
 *  Returns the path or URL to hand to the OS opener. Null for a workspace
 *  route (`/data/...`, `/artifacts/...` and the bare directory form of each),
 *  a relative href, or an external web URL.
 *
 *  This runs LAST in the click handler, after the app / nav / data-path
 *  extractors, so the absolute workspace routes they claim never reach here.
 *  The guards below are the belt to that braces: they keep an absolute data
 *  route out of the OS opener on their own, so reordering the handler cannot
 *  turn `/artifacts/report.pdf` into a disk path. Derived from
 *  `DATA_PATH_PREFIXES`, so a new sub-tree is covered the moment it is added
 *  there. */
export function extractLocalFileTarget(href: string): string | null {
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
 *  routes through `handleNavigationRequest({ target })`. Returns null when the
 *  href is not a panel name, and the caller falls through to the next
 *  rewriter. */
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

/** Extract a Lucidos app id from an href that points at an app's ENTRY POINT,
 *  meaning "open this app", not "preview a file inside it".
 *  Accepted (with an optional `data/` or `/data/` prefix):
 *    - `apps/<id>`, `apps/<id>/`, `apps/<id>/index.html`
 *    - `app:<id>`, the custom-scheme shorthand, with an optional `/`
 *  A query string or fragment is stripped before matching. Rejected: any other
 *  sub-path, external URLs, mailto and lookalike schemes (`apple:`).
 *
 *  Entry points only, because `lucidos.data.list()` returns EVERY file under
 *  `data/`, so the artifact path list always contains `apps/<id>/index.html`.
 *  A permissive extractor would pre-empt the artifact rewriter for sub-files
 *  the user wants to preview.
 *
 *  `app:<id>` is accepted because LLMs invent it by analogy to the documented
 *  `thread:<UUID>` scheme, and no OS handler claims the unknown scheme.
 *
 *  Exported because `rewriteAppAnchor` and the chat click handler both need
 *  it. */
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

/** Extract a trigger id from a `trigger:<id>` href, meaning "show me THIS
 *  trigger" rather than "open the Triggers panel". A trailing `/` is accepted
 *  and a query or fragment stripped, mirroring the `app:<id>` branch above.
 *
 *  The scheme exists because a trigger is the one first-class thing the agent
 *  names in a reply that had no link form: a file has its path, an app has
 *  `app:<id>`, a thread has `thread:<ws>/<uuid>`, and a trigger had only the
 *  panel it lives in. So the agent linked the panel, and when told to link the
 *  trigger it invented exactly this href. Nothing claimed it, and an unclaimed
 *  scheme is handed to a browser that has no handler for it, so the click did
 *  nothing at all. See docs/plans/2026-08-24-a-trigger-is-a-link.md.
 *
 *  `triggers` (the panel) cannot collide: it carries no scheme, so it stays
 *  with `extractNavTargetFromHref`. Nor can `triggers/<slug>`, which is a
 *  workspace path the artifact rewriter owns.
 *
 *  Exported because the anchor rewriter, the chat click handler and the
 *  preview-iframe router all need it. */
export function extractTriggerIdFromHref(href: string): string | null {
  if (!href.startsWith('trigger:')) return null;
  const rest = href.slice('trigger:'.length);
  const queryStart = rest.search(/[?#]/);
  const trimmed = queryStart === -1 ? rest : rest.slice(0, queryStart);
  const slash = trimmed.indexOf('/');
  if (slash === -1) return trimmed || null;              // `trigger:<id>`
  const id = trimmed.slice(0, slash);
  // `trigger:<id>/` is the same destination; `trigger:<id>/sub` has no meaning.
  return trimmed.slice(slash + 1).length === 0 ? (id || null) : null;
}

/** Extract a BARE app reference from an href: a single path segment with no
 *  URL scheme and no sub-path, e.g. `habit-tracker` or `Habit Tracker`. The
 *  caller resolves the token against the known app ids and names.
 *
 *  The LLM writes `[Habit Tracker](habit-tracker)` by analogy to
 *  `[Notifications](notifications)`. That href matches none of the strict
 *  shapes. Left alone the browser resolves it against the base href to a
 *  non-existent route, and the SPA fallback reloads the whole workspace.
 *
 *  Returns null when the href carries a URL scheme, has a slash beyond an
 *  optional leading or trailing one, or is empty. A query or fragment is
 *  stripped.
 *
 *  The returned token is percent-DECODED. Markdown encodes a destination
 *  carrying spaces, so `Habit%20Tracker` has to decode back before it can
 *  match the app name. */
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
  // name. A malformed escape throws: keep the raw token rather than dropping
  // the ref.
  try { return decodeURIComponent(candidate); } catch { return candidate; }
}

/** Mirror of `rewriteArtifactAnchor` for apps. LLMs naturally write
 *  `[Name](apps/<id>/index.html)`, which renders as a plain relative anchor.
 *  Left alone the browser navigates to the file under the `/data/*` static
 *  mount, showing a file preview rather than the running app. An href whose
 *  `<id>` names no known app is rejected, so an unrelated `apps/...` URL is
 *  left alone. */
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

/** Mirror of `rewriteAppAnchor` for triggers. Unlike the app rewriter it does
 *  NOT check the id against a loaded list. `navigateToTrigger` re-fetches the
 *  registry on a cache miss before deciding a trigger is gone. Gating the
 *  rewrite on a cached projection is how a trigger created moments ago renders
 *  as a dead link. Same reasoning as `rewriteArtifactAnchor` resolving by
 *  shape. */
function rewriteTriggerAnchor(tag: string): string | null {
  const m = tag.match(HREF_ATTR);
  if (!m) return null;
  const href = m[1] ?? m[2];
  if (!href) return null;
  const triggerId = extractTriggerIdFromHref(href);
  if (!triggerId) return null;
  const escapedId = triggerId.replace(/"/g, '&quot;');
  const stripped = tag
    .replace(HREF_ATTR, '')
    .replace(CLASS_ATTR, '')
    .replace(DATA_TRIGGER_ID_ATTR, '');
  return stripped.replace(
    /^<a/i,
    `<a href="#" class="trigger-link" data-trigger-id="${escapedId}"`,
  );
}

/** Bare "open the app" hrefs the LLM emits when it knows the app name should be
 *  a link but supplies no id: `[Site Publisher](app)`, `(app/)`, `(app:)`.
 *  These match neither the `app:<id>` / `apps/<id>` shapes nor a nav panel.
 *  Without recovery the browser resolves the relative href against the gateway
 *  base to a dead end.
 *
 *  TEMPORARY MEASURE, model-tolerance. Removable: see
 *  docs/temporary-measures.md § "Bare `app` href recovery", governed by
 *  .claude/rules/temporary-measures.md. Drop it once the agent reliably emits
 *  `app:<id>` links carrying an id. */
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
      continue; // nested tag: skip it, keep gathering text
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
 *  `appTextToId` maps both forms to the id, so `habit-tracker` and
 *  `Habit Tracker` both resolve. Runs AFTER the strict `apps/<id>`, nav and
 *  artifact rewriters, so a reserved nav panel or a real artifact path always
 *  wins. This only claims an href none of them wanted, and returns null when
 *  the href is not a bare ref, or names no known app. */
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

/** The regex batches and lookups linkify needs for a given (paths, apps) set.
 *  Building these is the bulk of a linkify call, and does not depend on the
 *  html. So they are built once per (paths, apps) and reused across every
 *  block in a render (see `getCompiled`). */
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
  // Keyed by every form a path may appear in: the full path, and the bare form
  // with the `artifacts/` prefix stripped, since LLMs sometimes write those.
  // Values are always the full store path, so both the text-segment linkifier
  // and the anchor rewriter hand the click handler a canonical path.
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
  // deliberate `[X](app:x)` / `[X](habit-tracker)` link the LLM wrote. An app
  // named in prose is NOT auto-linked: a blind `\b(name)\b` match linkifies
  // every mention of a generically-named app.
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

// Build the patterns ONCE per (paths, apps) set instead of once per call. Every
// exchange in a render passes the SAME array references. So a 1-entry
// by-reference memo hits for every call after the first, and across renders
// until the artifact or app list changes. `generation` bumps on each rebuild
// and folds into the output-cache key below, so a list change invalidates it.
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

// Output LRU cache, mirroring renderMarkdown's. linkify output is a pure
// function of (html, paths, apps), keyed on `${generation}\0${html}` so a
// (paths, apps) change invalidates it. A re-render busts the per-exchange memo
// in ChatExchange and re-linkifies every block, which the O(1) hit keeps cheap.
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

/** Post-process rendered HTML to linkify artifact paths and bare URLs, and to
 *  rewrite deliberate app / nav / artifact anchors. Pure in (html, paths,
 *  apps), and LRU-cached so a re-render with unchanged content is O(1).
 *  `opts.cache: false` opts out, for the live streaming buffer whose html
 *  changes every token. try/finally records elapsed linkify time for the perf
 *  phase split (utils/renderPhaseTimers.ts). */
export function linkifyPaths(
  html: string,
  paths: string[],
  apps: { name: string; id: string }[],
  opts?: { cache?: boolean },
): string {
  const start = performance.now();
  try {
    const useCache = opts?.cache !== false;
    // Build or reuse the patterns first: this may bump `generation`, which the
    // cache key below has to reflect.
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

  // Track tag nesting to skip content inside <a> (prevents nested anchors)
  // and <code> (code content should not be linkified).
  let insideAnchor = 0;
  let insideCode = 0;

  for (let i = 0; i < segments.length; i++) {
    if (i % 2 === 1) {
      const tag = segments[i].toLowerCase();
      if (tag.startsWith('<a ') || tag === '<a>') {
        insideAnchor++;
        // Rewriter order: app, then trigger, then nav, then artifact.
        //
        // App MUST run before artifact. The artifact rewriter resolves by
        // SHAPE, so it claims `apps/<id>/index.html` whether or not the path
        // list is loaded. Only the strict app rewriter running first keeps an
        // app entry point opening the app rather than a file preview. App
        // stays strict, so a sub-file like `apps/<id>/scripts/run.sh` falls
        // through to the artifact rewriter.
        //
        // Trigger sits next to app because it is the other scheme-based entity
        // link. It claims `trigger:` and nothing else, so it can collide with
        // no neighbour: the `triggers` panel carries no scheme and a
        // `triggers/<slug>` path is the artifact rewriter's.
        //
        // Nav matches only bare panel names with an optional `data/` prefix
        // and no slash beyond it, so it cannot collide with either neighbour.
        // Its position between them is for narrative clarity.
        let rewritten: string | null = null;
        if (appIds) rewritten = rewriteAppAnchor(segments[i], appIds);
        if (!rewritten) rewritten = rewriteTriggerAnchor(segments[i]);
        if (!rewritten) rewritten = rewriteNavAnchor(segments[i]);
        // Unconditional, NOT gated on `pathLookup`: the rewriter also resolves
        // a data path by shape, which is what a workspace with an empty or
        // not-yet-loaded artifact list needs.
        if (!rewritten) rewritten = rewriteArtifactAnchor(segments[i], pathLookup);
        // A bare app-id/name href the strict rewriters and nav declined,
        // resolved from the HREF against the known app ids and names. Without
        // this the browser navigates to the relative href and the SPA fallback
        // reloads the whole workspace.
        if (!rewritten && appTextToId) {
          rewritten = rewriteAppAnchorByBareRef(segments[i], appTextToId);
        }
        // Last resort: a bare `app` href with no id, resolved from the
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

    // Artifact paths are linkified even inside <code>, since LLMs wrap paths
    // in backticks. URLs are skipped inside <code>, to avoid mangling code
    // content. App names are not scanned in text at all.

    if (insideAnchor === 0) {
      // Two matchers, one precedence pass. The cached list runs first because it
      // also CANONICALIZES, mapping a bare `foo.md` back to `artifacts/foo.md`,
      // which shape alone cannot do. The shape matcher then claims a full path
      // the list does not know. One shared `resolveMatches` is what lets a
      // longer shape span beat a shorter cached one at the same start, e.g.
      // `artifacts/notes.md.bak` over a cached `artifacts/notes.md`.
      const candidates: Match[] = [];
      if (pathPatterns.length > 0) {
        collectMatches(segments[i], pathPatterns, (m) => artifactLink(m, pathLookup!.get(m)!), candidates);
      }
      collectProseMatches(segments[i], (m) => {
        const fullPath = pathLookup?.get(m) ?? extractDataPathTarget(m);
        return fullPath ? artifactLink(m, fullPath) : null;
      }, candidates);
      const spans = urlSpans(segments[i]);
      const outsideUrls = spans.length === 0
        ? candidates
        : candidates.filter((c) => !overlapsUrl(spans, c.start, c.end));
      segments[i] = applyMatches(segments[i], resolveMatches(outsideUrls));
    }

    if (insideCode > 0) continue;

    if (insideAnchor === 0) {
      URL_IN_TEXT.lastIndex = 0;
      segments[i] = segments[i].replace(URL_IN_TEXT, (match) => {
        return `<a href="${match}" target="_blank" rel="noopener">${match}</a>`;
      });
    }
  }

  return segments.join('');
}
