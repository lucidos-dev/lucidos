/**
 * Post-process rendered HTML to linkify artifact paths, app names, and bare URLs.
 * Tracks <a> and <code> nesting to avoid nested anchors and linkifying code content.
 */

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

/** If the opening anchor tag points at a known artifact path (with or without
 *  a `data/` / `/data/` prefix), return a replacement opening tag that uses
 *  `class="artifact-link" data-path="…"` so the chat click handler routes the
 *  click through `openFilePreview` (content panel) instead of letting the
 *  browser navigate to the `/data/*` static mount as a top-level URL. Returns
 *  null when the href doesn't resolve to a known artifact — caller leaves the
 *  segment untouched. Visible text inside the anchor is preserved verbatim;
 *  other attributes (`title`, `target`, `rel`, …) are kept.
 *
 *  We force `href="#"` on the rewritten anchor instead of dropping href
 *  entirely. iOS Safari (and iOS PWA in standalone mode) treats `<a>` without
 *  an href as a non-interactive element — taps silently don't translate to
 *  `click` events, even with `cursor: pointer`, so the delegated chat click
 *  handler never fires and the user sees a dead link. `preventDefault` in
 *  the delegated handler suppresses the `#` scroll-to-top. */
function rewriteArtifactAnchor(tag: string, pathLookup: Map<string, string>): string | null {
  const m = tag.match(HREF_ATTR);
  if (!m) return null;
  const href = m[1] ?? m[2];
  if (!href) return null;
  let candidate = href;
  if (candidate.startsWith('/data/')) candidate = candidate.slice('/data/'.length);
  else if (candidate.startsWith('data/')) candidate = candidate.slice('data/'.length);
  const fullPath = pathLookup.get(candidate);
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
 *  tool accepts for the panel targets and that `handleNavigationRequest` routes
 *  via `switchMenuItem`. Kept in sync by hand: the set is short and stable. */
const NAV_TARGETS: ReadonlySet<string> = new Set([
  'notifications',
  'apps',
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
 *  `thread:<UUID>` scheme — bug report was `[Momentum app](app:momentum)`
 *  rendered as `<a href="app:momentum">` that dead-ended on macOS Chrome
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

export function linkifyPaths(
  html: string,
  paths: string[],
  apps: { name: string; id: string }[],
): string {
  const segments = html.split(/(<[^>]+>)/);

  // Pre-build pattern batches outside the loop — they're invariant across segments.

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

  let appTextToId: Map<string, string> | undefined;
  let appPatterns: RegExp[] = [];
  let appIds: Set<string> | undefined;
  if (apps.length > 0) {
    // Match both app names and app IDs (LLMs sometimes use the ID)
    appTextToId = new Map();
    for (const s of apps) {
      if (!appTextToId.has(s.name)) appTextToId.set(s.name, s.id);
      if (s.id !== s.name && !appTextToId.has(s.id)) appTextToId.set(s.id, s.id);
    }
    const appTexts = [...appTextToId.keys()].sort((a, b) => b.length - a.length);
    const appEscaped = appTexts.map((t) => t.replace(REGEX_ESCAPE, '\\$&'));
    appPatterns = buildBatchedPatterns(appEscaped, (alt) => `\\b(${alt})\\b`);
    appIds = new Set(apps.map((s) => s.id));
  }

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
        // App MUST run before artifact (the path list from lucidos.data.list()
        // contains every app's index.html, so a permissive artifact rewriter
        // would claim `apps/<id>/index.html` first and the click would land
        // on openFilePreview instead of the running app — the "Link to <app>
        // opens file preview" bug). App is strict (entry-points only), so
        // sub-files like apps/<id>/scripts/run.sh return null and fall through
        // to the artifact rewriter for the expected file-preview behavior.
        //
        // Nav matches only bare panel names (`notifications`, `apps`, …) with
        // optional `data/` prefix — no slash beyond the prefix. It can't
        // collide with the app rewriter (which requires `apps/<id>...`) or
        // the artifact rewriter (which requires a known path in pathLookup).
        // Order within nav vs. app vs. artifact is therefore safe in any
        // arrangement; we put nav between them for narrative clarity.
        let rewritten: string | null = null;
        if (appIds) rewritten = rewriteAppAnchor(segments[i], appIds);
        if (!rewritten) rewritten = rewriteNavAnchor(segments[i]);
        if (!rewritten && pathLookup) rewritten = rewriteArtifactAnchor(segments[i], pathLookup);
        if (rewritten) segments[i] = rewritten;
      }
      else if (tag === '</a>') insideAnchor = Math.max(0, insideAnchor - 1);
      else if (tag === '<code>' || tag.startsWith('<code ')) insideCode++;
      else if (tag === '</code>') insideCode = Math.max(0, insideCode - 1);
      continue;
    }

    // Text segment — artifact paths are linkified even inside <code> (LLMs wrap paths in backticks).
    // App names and URLs are skipped inside <code> to avoid mangling code content.

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

    if (insideAnchor === 0 && appPatterns.length > 0) {
      const matches = collectMatches(segments[i], appPatterns, (match) => {
        const id = appTextToId?.get(match);
        if (!id) return match;
        const escapedId = id.replace(/"/g, '&quot;');
        // href="#" — see rewriteArtifactAnchor: iOS Safari/PWA needs href for
        // tap→click translation; preventDefault in the delegated chat handler
        // suppresses the scroll-to-top.
        return `<a href="#" class="app-link" data-app-id="${escapedId}">${match}</a>`;
      });
      segments[i] = applyMatches(segments[i], matches);
    }

    if (insideAnchor === 0) {
      urlPattern.lastIndex = 0;
      segments[i] = segments[i].replace(urlPattern, (match) => {
        return `<a href="${match}" target="_blank" rel="noopener">${match}</a>`;
      });
    }
  }

  return segments.join('');
}
