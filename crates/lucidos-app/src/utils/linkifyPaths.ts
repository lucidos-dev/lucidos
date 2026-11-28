
import { makeInertBody } from './escapeHtml';
import { escapeHtmlAttr } from './escapeHtmlAttr';
import { addLinkifyMs } from './renderPhaseTimers';
import { parseRepoPath, type RepoLocator } from '../store/repoPath';
import { DATA_PATH_PREFIXES, workspaceDataRoot } from './dataPathPrefixes';

const REGEX_ESCAPE = /[.*+?^${}()|[\]\\]/g;

type Match = { start: number; end: number; replacement: string };

function compareKeyToSpan(key: string, text: string, at: number, len: number): number {
  const n = Math.min(key.length, len);
  for (let j = 0; j < n; j++) {
    const d = key.charCodeAt(j) - text.charCodeAt(at + j);
    if (d !== 0) return d;
  }
  return key.length - len;
}

function sharedPrefixLength(key: string, text: string, at: number, len: number): number {
  const n = Math.min(key.length, len);
  let j = 0;
  while (j < n && key.charCodeAt(j) === text.charCodeAt(at + j)) j++;
  return j;
}

function longestKeyAt(keys: readonly string[], text: string, at: number): string | null {
  let len = text.length - at;
  let end = keys.length;
  while (len > 0) {
    let lo = 0;
    let hi = end;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if (compareKeyToSpan(keys[mid], text, at, len) <= 0) lo = mid + 1;
      else hi = mid;
    }
    if (lo === 0) return null;
    const key = keys[lo - 1];
    const shared = sharedPrefixLength(key, text, at, len);
    if (shared === key.length) return key;
    len = shared;
    end = lo - 1;
  }
  return null;
}

function collectKnownPathMatches(
  text: string,
  sortedKeys: readonly string[],
  render: (key: string) => string,
  matches: Match[],
): void {
  let i = 0;
  while (i < text.length) {
    const key = longestKeyAt(sortedKeys, text, i);
    if (key === null) {
      i++;
      continue;
    }
    matches.push({ start: i, end: i + key.length, replacement: render(key) });
    i += key.length;
  }
}

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
const DATA_LINE_ATTRS = /\sdata-line(?:-end)?\s*=\s*(?:"[^"]*"|'[^']*')/gi;

function rewriteArtifactAnchor(
  tag: string,
  pathLookup: Map<string, string> | undefined,
): string | null {
  const m = tag.match(HREF_ATTR);
  if (!m) return null;
  const href = m[1] ?? m[2];
  if (!href) return null;
  let candidate = href.replace(LINE_FRAGMENT_RE, '');
  if (candidate.startsWith('/data/')) candidate = candidate.slice('/data/'.length);
  else if (candidate.startsWith('data/')) candidate = candidate.slice('data/'.length);
  const fullPath = pathLookup?.get(candidate) ?? extractDataPathTarget(href);
  if (!fullPath) return null;
  const escapedPath = fullPath.replace(/"/g, '&quot;');
  const lines = hrefLineRef(href);
  const lineAttrs = lines === null
    ? ''
    : ` data-line="${lines.line}"${lines.lineEnd === undefined ? '' : ` data-line-end="${lines.lineEnd}"`}`;
  const stripped = tag
    .replace(HREF_ATTR, '')
    .replace(CLASS_ATTR, '')
    .replace(DATA_PATH_ATTR, '')
    .replace(DATA_LINE_ATTRS, '');
  return stripped.replace(/^<a/i, `<a href="#" class="artifact-link" data-path="${escapedPath}"${lineAttrs}`);
}

const DATA_APP_ID_ATTR = /\sdata-app-id\s*=\s*(?:"[^"]*"|'[^']*')/i;
const DATA_APP_FRAGMENT_ATTR = /\sdata-app-fragment\s*=\s*(?:"[^"]*"|'[^']*')/i;
const DATA_NAV_TARGET_ATTR = /\sdata-nav-target\s*=\s*(?:"[^"]*"|'[^']*')/i;
const DATA_SETTINGS_VIEW_ATTR = /\sdata-settings-view\s*=\s*(?:"[^"]*"|'[^']*')/i;
const DATA_SETTINGS_FRAGMENT_ATTR = /\sdata-settings-fragment\s*=\s*(?:"[^"]*"|'[^']*')/i;
const DATA_TRIGGER_ID_ATTR = /\sdata-trigger-id\s*=\s*(?:"[^"]*"|'[^']*')/i;

export const NAV_TARGETS: ReadonlySet<string> = new Set([
  'dashboard',
  'notifications',
  'apps',
  'plugins',
  'app-store',
  'triggers',
  'thread-queue',
  'changes',
  'files',
  'settings',
]);

export function extractNavTargetFromHref(href: string): string | null {
  let candidate = href;
  if (candidate.startsWith('/data/')) candidate = candidate.slice('/data/'.length);
  else if (candidate.startsWith('data/')) candidate = candidate.slice('data/'.length);
  if (candidate.startsWith('/')) candidate = candidate.slice(1);
  const queryStart = candidate.search(/[?#]/);
  if (queryStart !== -1) candidate = candidate.slice(0, queryStart);
  if (candidate.endsWith('/')) candidate = candidate.slice(0, -1);
  if (!candidate) return null;
  if (candidate.includes('/')) return null;
  return NAV_TARGETS.has(candidate) ? candidate : null;
}

export interface SettingsHrefTarget {
  view: string;
  fragment: string | null;
}

export function extractSettingsTargetFromHref(href: string): SettingsHrefTarget | null {
  const m = /^settings:([a-z0-9-]+)\/?(?:#([a-z0-9:-]+))?$/i.exec(href);
  return m ? { view: m[1], fragment: m[2] ?? null } : null;
}

export function hasUrlScheme(href: string): boolean {
  return /^[a-z][a-z0-9+.-]*:/i.test(href);
}

const BROWSER_NAVIGABLE_SCHEMES: ReadonlySet<string> =
  new Set(['http', 'https', 'mailto', 'tel', 'sms']);

export function browserHandlesHref(href: string): boolean {
  if (!hasUrlScheme(href)) return false;
  return BROWSER_NAVIGABLE_SCHEMES.has(href.slice(0, href.indexOf(':')).toLowerCase());
}

export const PROSE_DATA_PATH_PREFIXES: readonly string[] = [
  'artifacts/',
  'knowhow/',
  'apps/',
  'triggers/',
  'system-knowhow/',
];

const DATA_PATH_IN_PROSE = new RegExp(
  '(^|[^\\w/])'
  + '(\\/?(?:data\\/)?'
  + `(?:${PROSE_DATA_PATH_PREFIXES.map((p) => p.slice(0, -1).replace(REGEX_ESCAPE, '\\$&')).join('|')})`
  + '(?:\\/[^\\s<>"\'&/]+)*'
  + '\\/[^\\s<>"\'&/]*[^\\s<>"\'&/.]\\.[A-Za-z0-9]+)',
  'g',
);

const FILENAME_CONTINUES = /[-_~+%]/;

const SCHEME_CHAR = /[A-Za-z0-9+.-]/;
const SCHEME_FIRST_CHAR = /[A-Za-z]/;

function schemeEndsAt(text: string, colonIndex: number): boolean {
  let i = colonIndex - 1;
  while (i >= 0 && SCHEME_CHAR.test(text[i])) i--;
  return i + 1 < colonIndex && SCHEME_FIRST_CHAR.test(text[i + 1]);
}

const URL_IN_TEXT = /(?:https?|file):\/\/[^\s<>"')\]]*[^\s<>"')\].,;:!?]/g;

function urlSpans(text: string): Array<[number, number]> {
  const spans: Array<[number, number]> = [];
  URL_IN_TEXT.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = URL_IN_TEXT.exec(text)) !== null) spans.push([m.index, m.index + m[0].length]);
  return spans;
}

function overlapsUrl(spans: Array<[number, number]>, start: number, end: number): boolean {
  return spans.some(([from, to]) => start < to && end > from);
}

function artifactLink(text: string, path: string): string {
  const escapedPath = path.replace(/"/g, '&quot;');
  return `<a href="#" class="artifact-link" data-path="${escapedPath}">${text}</a>`;
}

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
    if (m[1] === ':' && schemeEndsAt(text, start - 1)) continue;
    if (FILENAME_CONTINUES.test(text.charAt(end))) continue;
    const replacement = render(m[2]);
    if (replacement === null) continue;
    matches.push({ start, end, replacement });
  }
}

export function extractDataPathTarget(href: string): string | null {
  if (hasUrlScheme(href)) return null;
  let candidate = href;
  if (candidate.startsWith('/data/')) candidate = candidate.slice('/data/'.length);
  else if (candidate.startsWith('data/')) candidate = candidate.slice('data/'.length);
  else if (candidate.startsWith('/')) candidate = candidate.slice(1);
  const queryStart = candidate.search(/[?#]/);
  if (queryStart !== -1) candidate = candidate.slice(0, queryStart);
  const prefix = DATA_PATH_PREFIXES.find((p) => candidate.startsWith(p));
  if (!prefix) return null;
  if (candidate.length === prefix.length || candidate.endsWith('/')) return null;
  return candidate;
}

export function extractWorkspaceDiskDataPath(href: string, workspacePath: string): string | null {
  if (!workspacePath) return null;
  let decoded = href;
  try { decoded = decodeURI(href); } catch {  }
  if (/^file:\/\//i.test(decoded)) decoded = decoded.slice('file://'.length);
  const dataRoot = workspaceDataRoot(workspacePath);
  if (!decoded.startsWith(dataRoot)) return null;
  const relative = decoded.slice(dataRoot.length);
  if (relative.split('/').some((segment) => segment === '.' || segment === '..')) return null;
  return extractDataPathTarget(relative);
}

export function extractLocalFileTarget(href: string): string | null {
  if (/^file:\/\//i.test(href)) return href;
  if (href.startsWith('/')) {
    return isWorkspaceAbsoluteRoute(href) ? null : href;
  }
  return null;
}

function isWorkspaceAbsoluteRoute(href: string): boolean {
  if (href === '/data' || href.startsWith('/data/')) return true;
  return DATA_PATH_PREFIXES.some((p) => {
    const bare = `/${p.slice(0, -1)}`;
    return href === bare || href.startsWith(`/${p}`);
  });
}

function rewriteNavAnchor(tag: string): string | null {
  const m = tag.match(HREF_ATTR);
  if (!m) return null;
  const href = m[1] ?? m[2];
  if (!href) return null;
  const settings = extractSettingsTargetFromHref(href);
  const target = settings ? 'settings' : extractNavTargetFromHref(href);
  if (!target) return null;
  const viewAttr = settings ? ` data-settings-view="${escapeAttr(settings.view)}"` : '';
  const fragmentAttr = settings?.fragment ? ` data-settings-fragment="${escapeAttr(settings.fragment)}"` : '';
  const stripped = tag
    .replace(HREF_ATTR, '')
    .replace(CLASS_ATTR, '')
    .replace(DATA_NAV_TARGET_ATTR, '')
    .replace(DATA_SETTINGS_VIEW_ATTR, '')
    .replace(DATA_SETTINGS_FRAGMENT_ATTR, '');
  return stripped.replace(
    /^<a/i,
    `<a href="#" class="nav-link" data-nav-target="${escapeAttr(target)}"${viewAttr}${fragmentAttr}`,
  );
}

export interface AppHrefTarget {
  appId: string;
  fragment: string | null;
}

export function extractAppTargetFromHref(href: string): AppHrefTarget | null {
  const hash = href.indexOf('#');
  const fragment = hash === -1 ? '' : href.slice(hash + 1);
  const appId = extractAppEntryPoint(hash === -1 ? href : href.slice(0, hash));
  if (appId === null) return null;
  return { appId, fragment: fragment.length === 0 ? null : fragment };
}

function extractAppEntryPoint(href: string): string | null {
  if (href.startsWith('app:')) {
    const rest = href.slice('app:'.length);
    if (rest.length === 0) return null;
    const queryStart = rest.indexOf('?');
    const trimmed = queryStart === -1 ? rest : rest.slice(0, queryStart);
    if (trimmed.length === 0) return null;
    const slash = trimmed.indexOf('/');
    if (slash === -1) return trimmed;
    const appId = trimmed.slice(0, slash);
    const afterSlash = trimmed.slice(slash + 1);
    if (afterSlash.length === 0) return appId || null;
    return null;
  }
  let candidate = href;
  if (candidate.startsWith('/data/')) candidate = candidate.slice('/data/'.length);
  else if (candidate.startsWith('data/')) candidate = candidate.slice('data/'.length);
  if (candidate.startsWith('/')) candidate = candidate.slice(1);
  if (!candidate.startsWith('apps/')) return null;
  const rest = candidate.slice('apps/'.length);
  if (rest.length === 0) return null;
  const queryStart = rest.indexOf('?');
  const trimmed = queryStart === -1 ? rest : rest.slice(0, queryStart);
  if (trimmed.length === 0) return null;
  const slash = trimmed.indexOf('/');
  if (slash === -1) return trimmed;
  const appId = trimmed.slice(0, slash);
  const afterSlash = trimmed.slice(slash + 1);
  if (afterSlash.length === 0) return appId || null;
  if (afterSlash === 'index.html') return appId || null;
  return null;
}

export function extractTriggerIdFromHref(href: string): string | null {
  if (!href.startsWith('trigger:')) return null;
  const rest = href.slice('trigger:'.length);
  const queryStart = rest.search(/[?#]/);
  const trimmed = queryStart === -1 ? rest : rest.slice(0, queryStart);
  const slash = trimmed.indexOf('/');
  if (slash === -1) return trimmed || null;
  const id = trimmed.slice(0, slash);
  return trimmed.slice(slash + 1).length === 0 ? (id || null) : null;
}

export type RepoFileHrefTarget = { locator: RepoLocator } & Partial<LineRef>;

const LINE_FRAGMENT_RE = /#L(\d+)(?:-L?(\d+))?$/;

const LABEL_LINES_RE = /:(\d+)(?:\s*[-\u2013]\s*(\d+))?$/;

export interface LineRef {
  line: number;
  lineEnd?: number;
}

function lineRefFrom(match: RegExpExecArray | null): LineRef | null {
  if (!match) return null;
  return match[2] === undefined
    ? { line: Number(match[1]) }
    : { line: Number(match[1]), lineEnd: Number(match[2]) };
}

export function hrefLineRef(href: string): LineRef | null {
  return lineRefFrom(LINE_FRAGMENT_RE.exec(href));
}

export function labelLineRef(label: string): LineRef | null {
  return lineRefFrom(LABEL_LINES_RE.exec(label.trim()));
}

export function extractRepoFileTargetFromHref(href: string): RepoFileHrefTarget | null {
  if (!href.startsWith('repo:')) return null;
  let decoded = href;
  try { decoded = decodeURIComponent(href); } catch {  }
  const lineMatch = LINE_FRAGMENT_RE.exec(decoded);
  const locator = parseRepoPath(lineMatch ? decoded.slice(0, lineMatch.index) : decoded);
  if (!locator) return null;
  const lines = lineRefFrom(lineMatch);
  if (!lines) return { locator };
  return { locator, line: lines.line, lineEnd: lines.lineEnd };
}

export function extractBareAppRef(href: string): string | null {
  if (hasUrlScheme(href)) return null;
  let candidate = href;
  if (candidate.startsWith('/')) candidate = candidate.slice(1);
  const queryStart = candidate.search(/[?#]/);
  if (queryStart !== -1) candidate = candidate.slice(0, queryStart);
  if (candidate.endsWith('/')) candidate = candidate.slice(0, -1);
  if (!candidate) return null;
  if (candidate.includes('/')) return null;
  try { return decodeURIComponent(candidate); } catch { return candidate; }
}

function rewriteAppAnchor(tag: string, appIds: Set<string>): string | null {
  const m = tag.match(HREF_ATTR);
  if (!m) return null;
  const href = m[1] ?? m[2];
  if (!href) return null;
  const target = extractAppTargetFromHref(href);
  if (!target || !appIds.has(target.appId)) return null;
  const fragmentAttr = target.fragment === null
    ? ''
    : ` data-app-fragment="${escapeAttr(target.fragment)}"`;
  const stripped = stripAppLinkAttrs(tag);
  return stripped.replace(
    /^<a/i,
    `<a href="#" class="app-link" data-app-id="${escapeAttr(target.appId)}"${fragmentAttr}`,
  );
}

function stripAppLinkAttrs(tag: string): string {
  return tag
    .replace(HREF_ATTR, '')
    .replace(CLASS_ATTR, '')
    .replace(DATA_APP_ID_ATTR, '')
    .replace(DATA_APP_FRAGMENT_ATTR, '');
}

function escapeAttr(value: string): string {
  return value.replace(/"/g, '&quot;');
}

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

function anchorText(segments: string[], openTagIndex: number): string {
  let text = '';
  for (let j = openTagIndex + 1; j < segments.length; j++) {
    if (j % 2 === 1) {
      if (segments[j].toLowerCase() === '</a>') break;
      continue;
    }
    text += segments[j];
  }
  return text;
}

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
  const stripped = stripAppLinkAttrs(tag);
  return stripped.replace(/^<a/i, `<a href="#" class="app-link" data-app-id="${escapeAttr(id)}"`);
}

function rewriteAppAnchorByBareRef(tag: string, appTextToId: Map<string, string>): string | null {
  const m = tag.match(HREF_ATTR);
  if (!m) return null;
  const href = m[1] ?? m[2];
  if (!href) return null;
  const token = extractBareAppRef(href);
  if (!token) return null;
  const id = appTextToId.get(token);
  if (!id) return null;
  const stripped = stripAppLinkAttrs(tag);
  return stripped.replace(/^<a/i, `<a href="#" class="app-link" data-app-id="${escapeAttr(id)}"`);
}

interface CompiledLinkify {
  sortedPathKeys: string[];
  pathLookup: Map<string, string> | undefined;
  appTextToId: Map<string, string> | undefined;
  appIds: Set<string> | undefined;
}

function buildCompiled(
  paths: string[],
  apps: { name: string; id: string }[],
): CompiledLinkify {
  let sortedPathKeys: string[] = [];
  let pathLookup: Map<string, string> | undefined;
  if (paths.length > 0) {
    pathLookup = new Map();
    for (const p of paths) {
      pathLookup.set(p, p);
      if (p.startsWith('artifacts/')) {
        pathLookup.set(p.slice('artifacts/'.length), p);
      }
    }
    sortedPathKeys = [...pathLookup.keys()].filter((key) => key.length > 0).sort();
  }

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

  return { sortedPathKeys, pathLookup, appTextToId, appIds };
}

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

const LINKIFY_CACHE_MAX = 400;
const linkifyCache = new Map<string, string>();

export function _resetLinkifyCacheForTesting(): void {
  linkifyCache.clear();
  cachedPaths = null;
  cachedApps = null;
  cachedCompiled = null;
  generation = 0;
}

export function linkifyPaths(
  html: string,
  paths: string[],
  apps: { name: string; id: string }[],
  opts?: { cache?: boolean },
): string {
  const start = performance.now();
  try {
    const useCache = opts?.cache !== false;
    const compiled = getCompiled(paths, apps);
    const key = useCache ? `${generation}\0${html}` : '';
    if (useCache) {
      const hit = linkifyCache.get(key);
      if (hit !== undefined) {
        linkifyCache.delete(key);
        linkifyCache.set(key, hit);
        return hit;
      }
    }
    const out = applyCompiled(html, compiled);
    if (useCache) {
      linkifyCache.set(key, out);
      if (linkifyCache.size > LINKIFY_CACHE_MAX) {
        const oldest = linkifyCache.keys().next().value;
        if (oldest !== undefined) linkifyCache.delete(oldest);
      }
    }
    return out;
  } finally {
    addLinkifyMs(performance.now() - start);
  }
}

const TAG_SPLIT = /(<[^>]+>)/;

const STASH_MARK = '\uE000';
const STASH_REF = new RegExp(`${STASH_MARK}(\\d+)${STASH_MARK}`, 'g');

function splitTags(html: string): string[] | null {
  const segments = html.split(TAG_SPLIT);
  if (!segments.some(isCutTag)) return segments;
  const escaped = withAttributeBracketsEscaped(html);
  if (escaped === null) return null;
  const resplit = escaped.split(TAG_SPLIT);
  return resplit.some(isCutTag) ? null : resplit;
}

function isCutTag(segment: string, index: number): boolean {
  return index % 2 === 1 && endsInsideValue(segment);
}

function endsInsideValue(tag: string): boolean {
  let open: string | null = null;
  let previous = '';
  for (const c of tag) {
    if (open) {
      if (c === open) open = null;
    } else if ((c === '"' || c === "'") && previous === '=') {
      open = c;
    }
    if (!open && !/\s/.test(c)) previous = c;
  }
  return open !== null;
}

let attributeBody: HTMLElement | null | undefined;

function withAttributeBracketsEscaped(html: string): string | null {
  if (html.includes(STASH_MARK)) return null;
  if (attributeBody === undefined) attributeBody = makeInertBody();
  const body = attributeBody;
  if (!body) return null;
  body.innerHTML = html;
  try {
    const stash: string[] = [];
    for (const el of Array.from(body.querySelectorAll('*'))) {
      for (const attr of Array.from(el.attributes)) {
        if (!/[<>]/.test(attr.value)) continue;
        stash.push(attr.value);
        attr.value = `${STASH_MARK}${stash.length - 1}${STASH_MARK}`;
      }
    }
    return body.innerHTML.replace(STASH_REF, (_, i: string) => escapeHtmlAttr(stash[Number(i)]));
  } finally {
    body.textContent = '';
  }
}

function applyCompiled(html: string, compiled: CompiledLinkify): string {
  const { sortedPathKeys, pathLookup, appTextToId, appIds } = compiled;
  const segments = splitTags(html);
  if (!segments) return html;

  let insideAnchor = 0;
  let insideCode = 0;

  for (let i = 0; i < segments.length; i++) {
    if (i % 2 === 1) {
      const tag = segments[i].toLowerCase();
      if (tag.startsWith('<a ') || tag === '<a>') {
        insideAnchor++;
        let rewritten: string | null = null;
        if (appIds) rewritten = rewriteAppAnchor(segments[i], appIds);
        if (!rewritten) rewritten = rewriteTriggerAnchor(segments[i]);
        if (!rewritten) rewritten = rewriteNavAnchor(segments[i]);
        if (!rewritten) rewritten = rewriteArtifactAnchor(segments[i], pathLookup);
        if (!rewritten && appTextToId) {
          rewritten = rewriteAppAnchorByBareRef(segments[i], appTextToId);
        }
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


    if (insideAnchor === 0) {
      const candidates: Match[] = [];
      if (sortedPathKeys.length > 0) {
        collectKnownPathMatches(segments[i], sortedPathKeys, (m) => artifactLink(m, pathLookup!.get(m)!), candidates);
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
