
import { ALL_HIGHLIGHT, CURRENT_HIGHLIGHT } from '@lucidos/find';
import { WORKSPACE_ID } from '../../utils/basePath';
import { escapeHtmlAttr } from '../../utils/escapeHtmlAttr';
import {
  extractAppTargetFromHref,
  extractNavTargetFromHref,
  extractLocalFileTarget,
  extractWorkspaceDiskDataPath,
  extractTriggerIdFromHref,
  extractRepoFileTargetFromHref,
  extractSettingsTargetFromHref,
  hasUrlScheme,
  type RepoFileHrefTarget,
  type SettingsHrefTarget,
} from '../../utils/linkifyPaths';
import { openFilePreview, openUrl, openLocalFileOnConfirm } from '../../store/actions/artifacts';
import { openAppById } from '../../store/actions/apps';
import {
  openThreadAcrossWorkspaces,
  openWorkspaceThreadUrl,
  type WorkspaceThreadUrl,
} from '../../store/actions/cross-workspace';
import { handleNavigationRequest } from '../../store/actions/navigation-request';
import { openRepoFileLink } from '../../store/actions/repoFileLink';
import { encodeRepoPath, type RepoLocator } from '../../store/repoPath';
import { showToast, workspacePath } from '../../store/store';
import { HEADING_ID_PREFIX } from '../../utils/headingSlug';
import { scrollBehavior } from '../../utils/motion';
import { THREAD_LINK_RE } from '../../utils/threadLinkWorkspace';

const THREAD_FRAGMENT_RE = /^#?thread=([0-9a-f-]+)$/;

export interface PreviewLinkContext {
  artifactPath: string;
  hostOrigin: string;
  hostPath: string;
  workspaceId: string | null;
  workspacePath: string;
  documentBase?: string;
  repoLocator?: RepoLocator;
}

export type PreviewLinkAction =
  | { kind: 'fragment'; id: string }
  | { kind: 'thread'; workspace: string | undefined; threadId: string }
  | { kind: 'workspace-thread'; target: WorkspaceThreadUrl }
  | { kind: 'app'; appId: string; fragment?: string }
  | { kind: 'trigger'; triggerId: string }
  | { kind: 'nav'; target: string; settings?: SettingsHrefTarget }
  | { kind: 'local-file'; target: string }
  | { kind: 'file'; path: string }
  | ({ kind: 'repo-file' } & RepoFileHrefTarget)
  | { kind: 'external'; url: string };

export const PREVIEW_HOST_SCHEMES: readonly string[] =
  ['http', 'https', 'thread', 'app', 'trigger', 'repo', 'file', 'settings'];

function decodeFragment(id: string): string {
  try {
    return decodeURIComponent(id);
  } catch {
    return id;
  }
}

function normalizeSegments(path: string): string {
  const out: string[] = [];
  for (const segment of path.split('/')) {
    if (segment === '' || segment === '.') continue;
    if (segment === '..') {
      out.pop();
      continue;
    }
    out.push(segment);
  }
  return out.join('/');
}

export function resolvePreviewRelativePath(artifactPath: string, href: string): string {
  const [pathPart] = href.split(/[?#]/, 1);
  if (pathPart.startsWith('/data/')) return normalizeSegments(pathPart.slice('/data/'.length));
  if (pathPart.startsWith('data/')) return normalizeSegments(pathPart.slice('data/'.length));
  if (pathPart.startsWith('/')) return normalizeSegments(pathPart);
  const folder = artifactPath.slice(0, artifactPath.lastIndexOf('/') + 1);
  return normalizeSegments(folder + pathPart);
}

export function resolvePreviewRelativeRepoPath(locator: RepoLocator, href: string): RepoLocator {
  const [pathPart] = href.split(/[?#]/, 1);
  const path = pathPart.startsWith('/')
    ? normalizeSegments(pathPart.slice(1))
    : normalizeSegments(locator.path.slice(0, locator.path.lastIndexOf('/') + 1) + pathPart);
  return { ...locator, path };
}

function samePath(a: string, b: string): boolean {
  const trim = (p: string) => (p.length > 1 && p.endsWith('/') ? p.slice(0, -1) : p);
  return trim(a) === trim(b);
}

export function classifyPreviewLink(
  rawHref: string,
  ctx: PreviewLinkContext,
): PreviewLinkAction | null {
  const href = rawHref.trim();
  if (!href) return null;

  if (href.startsWith('#')) {
    const asThread = THREAD_FRAGMENT_RE.exec(href);
    if (asThread) return { kind: 'thread', workspace: undefined, threadId: asThread[1] };
    return { kind: 'fragment', id: decodeFragment(href.slice(1)) };
  }

  const asThreadScheme = THREAD_LINK_RE.exec(href);
  if (asThreadScheme) {
    return { kind: 'thread', workspace: asThreadScheme[1], threadId: asThreadScheme[2] };
  }

  if (/^https?:\/\//i.test(href)) {
    let url: URL;
    try {
      url = new URL(href);
    } catch {
      return null;
    }
    if (url.origin === ctx.hostOrigin) {
      const asThreadHash = THREAD_FRAGMENT_RE.exec(url.hash);
      if (asThreadHash) {
        const threadId = asThreadHash[1];
        const route = url.pathname.split('/').filter(Boolean)[0];
        return !route || route === ctx.workspaceId
          ? { kind: 'thread', workspace: undefined, threadId }
          : { kind: 'workspace-thread', target: { route, threadId } };
      }
      if (url.hash && samePath(url.pathname, ctx.hostPath)) {
        return { kind: 'fragment', id: decodeFragment(url.hash.slice(1)) };
      }
    }
    return { kind: 'external', url: href };
  }

  const appTarget = extractAppTargetFromHref(href);
  if (appTarget) return { kind: 'app', appId: appTarget.appId, fragment: appTarget.fragment ?? undefined };
  const triggerId = extractTriggerIdFromHref(href);
  if (triggerId) return { kind: 'trigger', triggerId };
  const settings = extractSettingsTargetFromHref(href);
  if (settings) return { kind: 'nav', target: 'settings', settings };
  const navTarget = extractNavTargetFromHref(href);
  if (navTarget) return { kind: 'nav', target: navTarget };
  if (!(ctx.repoLocator && href.startsWith('/'))) {
    const dataPath = extractWorkspaceDiskDataPath(href, ctx.workspacePath);
    if (dataPath) return { kind: 'file', path: dataPath };
    const localFile = extractLocalFileTarget(href);
    if (localFile) return { kind: 'local-file', target: localFile };
  }

  const repoFile = extractRepoFileTargetFromHref(href);
  if (repoFile) return { kind: 'repo-file', ...repoFile };

  if (hasUrlScheme(href)) return null;

  if (ctx.documentBase) {
    let resolved: URL | null = null;
    try {
      resolved = new URL(href, ctx.documentBase);
    } catch {
      resolved = null;
    }
    if (resolved) {
      if (resolved.origin !== ctx.hostOrigin) return { kind: 'external', url: resolved.href };
      const mount = resolved.pathname.indexOf('/data/');
      if (mount === -1) return { kind: 'external', url: resolved.href };
      return { kind: 'file', path: normalizeSegments(resolved.pathname.slice(mount + '/data/'.length)) };
    }
  }

  if (ctx.repoLocator) {
    return { kind: 'file', path: encodeRepoPath(resolvePreviewRelativeRepoPath(ctx.repoLocator, href)) };
  }
  return { kind: 'file', path: resolvePreviewRelativePath(ctx.artifactPath, href) };
}

export type PreviewNavigation = Exclude<PreviewLinkAction, { kind: 'fragment' }>;

export function runPreviewLinkAction(action: PreviewNavigation, artifactPath: string): void {
  switch (action.kind) {
    case 'thread':
      openThreadAcrossWorkspaces(action.workspace, action.threadId);
      return;
    case 'workspace-thread':
      openWorkspaceThreadUrl(action.target);
      return;
    case 'app':
      void openAppById(action.appId, 'a file preview', action.fragment);
      return;
    case 'trigger':
      void handleNavigationRequest(
        { target: 'trigger', id: action.triggerId },
        { source: 'a file preview' },
      );
      return;
    case 'nav':
      void handleNavigationRequest({
        target: action.target,
        settings_view: action.settings?.view,
        fragment: action.settings?.fragment ?? undefined,
      });
      return;
    case 'local-file':
      void openLocalFileOnConfirm(action.target, artifactPath);
      return;
    case 'file':
      openFilePreview(action.path);
      return;
    case 'repo-file':
      void openRepoFileLink(action, 'a file preview');
      return;
    case 'external':
      openUrl(action.url);
      return;
  }
}

export function previewLinkContext(
  artifactPath: string,
  documentBase?: string,
  repoLocator?: RepoLocator,
): PreviewLinkContext {
  const page = typeof location === 'undefined' ? null : location;
  return {
    artifactPath,
    hostOrigin: page?.origin ?? '',
    hostPath: page?.pathname ?? '/',
    workspaceId: WORKSPACE_ID,
    workspacePath: workspacePath.value,
    documentBase,
    repoLocator,
  };
}

function fragmentTarget(root: Element, id: string): Element | null {
  const candidates = Array.from(root.querySelectorAll('[id], [name]'));
  return candidates.find((el) => el.id === `${HEADING_ID_PREFIX}${id}`)
    ?? candidates.find((el) => el.id === id)
    ?? candidates.find((el) => el.getAttribute('name') === id)
    ?? null;
}

export function handlePreviewLinkClick(e: MouseEvent, artifactPath: string, repoLocator?: RepoLocator): void {
  if (e.defaultPrevented) return;
  if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || (e.button ?? 0) !== 0) return;
  const target = e.target as { closest?: (sel: string) => Element | null } | null;
  const anchor = target?.closest?.('a[href]') ?? null;
  if (!anchor) return;
  if (anchor.hasAttribute?.('download')) return;

  const linkedThreadId = anchor.getAttribute('data-thread-id');
  if (linkedThreadId) {
    e.preventDefault();
    e.stopPropagation();
    openThreadAcrossWorkspaces(anchor.getAttribute('data-thread-workspace') ?? undefined, linkedThreadId);
    return;
  }

  const action = classifyPreviewLink(
    anchor.getAttribute('href') ?? '',
    previewLinkContext(artifactPath, undefined, repoLocator),
  );
  if (!action) return;
  if (action.kind === 'fragment') {
    const root = anchor.closest?.('.markdown-content') ?? null;
    if (!root) return;
    e.preventDefault();
    e.stopPropagation();
    scrollToFragment(root, action.id, repoLocator?.path ?? artifactPath);
    return;
  }
  e.preventDefault();
  e.stopPropagation();
  runPreviewLinkAction(action, artifactPath);
}

function scrollToFragment(root: Element, id: string, documentPath: string): void {
  const target = id ? fragmentTarget(root, id) : root;
  if (!target) {
    showToast(`No "${id}" section in ${documentPath}`, 'error');
    return;
  }
  target.scrollIntoView({ behavior: scrollBehavior(), block: 'start' });
}

export function previewBaseHref(fileUrl: string): string {
  const queryAt = fileUrl.search(/[?#]/);
  const clean = queryAt === -1 ? fileUrl : fileUrl.slice(0, queryAt);
  const lastSlash = clean.lastIndexOf('/');
  return lastSlash === -1 ? clean : clean.slice(0, lastSlash + 1);
}

const DECLARED_BASE_RE = /<base\s[^>]*href\s*=/i;

export function documentDeclaresBase(html: string): boolean {
  return DECLARED_BASE_RE.test(html);
}

export function injectAtHeadStart(html: string, tag: string): string {
  const headOpen = /<head(\s[^>]*)?>/i.exec(html);
  if (headOpen) {
    const at = headOpen.index + headOpen[0].length;
    return html.slice(0, at) + tag + html.slice(at);
  }
  const htmlOpen = /<html(\s[^>]*)?>/i.exec(html);
  if (htmlOpen) {
    const at = htmlOpen.index + htmlOpen[0].length;
    return `${html.slice(0, at)}<head>${tag}</head>${html.slice(at)}`;
  }
  const doctype = /^\s*<!doctype[^>]*>/i.exec(html);
  if (doctype) {
    return html.slice(0, doctype[0].length) + tag + html.slice(doctype[0].length);
  }
  return tag + html;
}

export function withPreviewBase(html: string, baseHref: string): string {
  if (documentDeclaresBase(html)) return html;
  return injectAtHeadStart(html, `<base href="${escapeHtmlAttr(baseHref)}">`);
}

export function withPreviewSizing(html: string, scalePercent: number): string {
  return injectAtHeadStart(
    html,
    `<style>${previewZoomRule(scalePercent)}${PREVIEW_BODY_TEXT_RULE}${PREVIEW_FIND_RULES}</style>`,
  );
}

export const PREVIEW_FIND_RULES = ':root{--find-highlights:styled}'
  + `::highlight(${ALL_HIGHLIGHT}){background-color:Mark;color:MarkText}`
  + `::highlight(${CURRENT_HIGHLIGHT}){background-color:SelectedItem;color:SelectedItemText}`;

const PREVIEW_BODY_TEXT_RULE = ':where(body){font-size:0.75rem}';

function previewZoomRule(scalePercent: number): string {
  if (!Number.isFinite(scalePercent) || scalePercent <= 0) return '';
  const pct = Math.round(scalePercent * 10) / 10;
  if (pct === 100) return '';
  return `:root{zoom:${pct}%}`;
}
