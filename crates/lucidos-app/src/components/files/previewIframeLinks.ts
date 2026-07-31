// Route link clicks inside an HTML artifact PREVIEW iframe through the host,
// instead of letting the iframe navigate.
//
// The bug this exists for: `FilePreviewInline` renders an `.html` artifact into
// `<iframe srcDoc={…}>`, and an `about:srcdoc` document has no URL of its own,
// so it resolves every relative and fragment href against the HOST PAGE's URL.
// A plain in-page table-of-contents link `<a href="#section">` therefore
// resolves to `https://<gateway>/<slug>/#section`, which from the iframe's point
// of view is a real cross-document navigation: the iframe loads the whole
// Lucidos app shell into the content pane. The same is true of every relative
// path in the document. The host's global `.thread-link` click handler
// (`useStartup`) cannot help, because the click happens in a different document
// and never reaches the host `document`.
//
// Preview iframes are SAME-ORIGIN (`about:srcdoc` inherits the host origin), so
// the host can listen on the iframe's own `contentDocument` and, having the real
// event, `preventDefault()` the navigation before it starts. That is exactly the
// mechanism `bridgePreviewIframeShortcuts` already uses for keyboard chords;
// this is its click twin, and the two are wired from the same `onLoad`.
//
// Everything the bridge claims is routed through an existing host entry point
// (`openFilePreview`, `openThreadAcrossWorkspaces`, `openAppById`,
// `handleNavigationRequest`, `openLocalFile`, `openUrl`) rather than by poking
// store signals, which is what gives content-pane navigation from a preview its
// nav-history entry for free: those helpers already push one.

import { WORKSPACE_ID } from '../../utils/basePath';
import { escapeHtmlAttr } from '../../utils/markedConfig';
import {
  extractAppIdFromHref,
  extractNavTargetFromHref,
  extractLocalFileTarget,
} from '../../utils/linkifyPaths';
import { openFilePreview, openUrl, openLocalFile } from '../../store/actions/artifacts';
import { openAppById } from '../../store/actions/apps';
import { openThreadAcrossWorkspaces } from '../../store/actions/cross-workspace';
import { handleNavigationRequest } from '../../store/actions/navigation-request';
import { showToast } from '../../store/store';

/** `thread:<workspace>/<uuid>` and the bare `thread:<uuid>` form, mirroring the
 *  markdown rewrite in `utils/renderMarkdown.ts`. */
const THREAD_SCHEME_RE = /^thread:(?:([a-zA-Z0-9_-]+)\/)?([0-9a-f-]+)$/;
/** The cross-workspace landing fragment (`store/actions/cross-workspace.ts`),
 *  matched here against a link's own hash rather than `window.location`. */
const THREAD_FRAGMENT_RE = /^#?thread=([0-9a-f-]+)$/;
/** Any URL scheme, e.g. `mailto:`, `tel:`, `data:`. */
const HAS_SCHEME_RE = /^[a-z][a-z0-9+.-]*:/i;

/** What the host page a preview is embedded in resolves the preview's relative
 *  and fragment hrefs against, plus which artifact is being previewed. */
export interface PreviewLinkContext {
  /** Workspace-relative path of the previewed artifact, e.g.
   *  `artifacts/reports/pr-1573.html`. Sibling links resolve against its folder. */
  artifactPath: string;
  /** The HOST page's origin and pathname. */
  hostOrigin: string;
  hostPath: string;
  /** This bundle's workspace slug, or null when not served behind the gateway. */
  workspaceId: string | null;
  /** The previewed document's effective base URI, supplied ONLY when the
   *  artifact declares its own `<base href>` (see `documentDeclaresBase`). In
   *  that case relative links resolve there rather than against `artifactPath`. */
  documentBase?: string;
}

/** How the host should handle a click on a link inside a preview. `null` means
 *  "not ours": the browser keeps its default, which is correct for `mailto:`,
 *  `tel:` and anything else no host surface owns. */
export type PreviewLinkAction =
  | { kind: 'fragment'; id: string }
  | { kind: 'thread'; workspace: string | undefined; threadId: string }
  | { kind: 'app'; appId: string }
  | { kind: 'nav'; target: string }
  | { kind: 'local-file'; target: string }
  | { kind: 'file'; path: string }
  | { kind: 'external'; url: string };

function decodeFragment(id: string): string {
  try {
    return decodeURIComponent(id);
  } catch {
    return id; // malformed escape: match on the raw token rather than drop it
  }
}

/** Collapse `.` / `..` / empty segments so a sibling link like `../data/x.md`
 *  yields a canonical workspace path. */
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

/** Resolve a scheme-less href written inside a previewed artifact to a
 *  workspace-relative path. Root-relative and `data/`-prefixed forms are
 *  anchored at the workspace data root; everything else is relative to the
 *  previewed artifact's own folder, which is what the document author means by
 *  `report-appendix.md`. */
export function resolvePreviewRelativePath(artifactPath: string, href: string): string {
  const [pathPart] = href.split(/[?#]/, 1);
  if (pathPart.startsWith('/data/')) return normalizeSegments(pathPart.slice('/data/'.length));
  if (pathPart.startsWith('data/')) return normalizeSegments(pathPart.slice('data/'.length));
  if (pathPart.startsWith('/')) return normalizeSegments(pathPart);
  const folder = artifactPath.slice(0, artifactPath.lastIndexOf('/') + 1);
  return normalizeSegments(folder + pathPart);
}

/** Two pathnames naming the same page, ignoring a trailing slash. */
function samePath(a: string, b: string): boolean {
  const trim = (p: string) => (p.length > 1 && p.endsWith('/') ? p.slice(0, -1) : p);
  return trim(a) === trim(b);
}

/** The workspace a same-origin app URL points at, or undefined when it is this
 *  page's own workspace (so the router focuses in place instead of hopping). */
function workspaceOfPath(pathname: string, ctx: PreviewLinkContext): string | undefined {
  const first = pathname.split('/').filter(Boolean)[0];
  if (!first || first === ctx.workspaceId) return undefined;
  return first;
}

/** Decide what a click on `rawHref` inside a preview should do. Pure, so the
 *  whole routing table is unit-testable without a DOM.
 *
 *  `rawHref` is the anchor's `href` ATTRIBUTE, not its resolved `.href`
 *  property: in an `about:srcdoc` document the resolved value has already been
 *  rewritten against the host page URL, which is the very confusion this bridge
 *  exists to undo. */
export function classifyPreviewLink(
  rawHref: string,
  ctx: PreviewLinkContext,
): PreviewLinkAction | null {
  const href = rawHref.trim();
  if (!href) return null;

  // In-page anchor. The reported case, and the one that must never navigate.
  if (href.startsWith('#')) {
    const asThread = THREAD_FRAGMENT_RE.exec(href);
    if (asThread) return { kind: 'thread', workspace: undefined, threadId: asThread[1] };
    return { kind: 'fragment', id: decodeFragment(href.slice(1)) };
  }

  const asThreadScheme = THREAD_SCHEME_RE.exec(href);
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
      // An artifact that spells the app URL out in full rather than leaving a
      // bare `#anchor`. Same destinations, so the same routing.
      const asThreadHash = THREAD_FRAGMENT_RE.exec(url.hash);
      if (asThreadHash) {
        return {
          kind: 'thread',
          workspace: workspaceOfPath(url.pathname, ctx),
          threadId: asThreadHash[1],
        };
      }
      if (url.hash && samePath(url.pathname, ctx.hostPath)) {
        return { kind: 'fragment', id: decodeFragment(url.hash.slice(1)) };
      }
    }
    // Anything else with an absolute http(s) URL, including a same-origin app
    // URL we can't resolve to an in-app destination: a new tab is the one answer
    // that is never wrong, and it leaves the preview standing.
    return { kind: 'external', url: href };
  }

  const appId = extractAppIdFromHref(href);
  if (appId) return { kind: 'app', appId };
  const navTarget = extractNavTargetFromHref(href);
  if (navTarget) return { kind: 'nav', target: navTarget };
  const localFile = extractLocalFileTarget(href);
  if (localFile) return { kind: 'local-file', target: localFile };

  // A scheme we don't own (`mailto:`, `tel:`, …): leave the browser to it.
  if (HAS_SCHEME_RE.test(href)) return null;

  // A document that declares its OWN `<base href>` means its relative links to
  // resolve there, not against the folder it happens to be stored in.
  // `withPreviewBase` deliberately leaves such a base alone, so the routing has
  // to honour it too: `guide.html` under `<base href="https://example.com/docs/">`
  // is an external page, not a workspace file.
  if (ctx.documentBase) {
    let resolved: URL | null = null;
    try {
      resolved = new URL(href, ctx.documentBase);
    } catch {
      resolved = null;
    }
    if (resolved) {
      if (resolved.origin !== ctx.hostOrigin) return { kind: 'external', url: resolved.href };
      // Same origin: only the engine's `/data/` mount maps back to a workspace
      // path. Anything else on this origin is a URL, not a file we can preview.
      // FIRST match, not last: the mount sits right after the workspace prefix,
      // and a workspace can legitimately hold a nested `data/` folder.
      const mount = resolved.pathname.indexOf('/data/');
      if (mount === -1) return { kind: 'external', url: resolved.href };
      return { kind: 'file', path: normalizeSegments(resolved.pathname.slice(mount + '/data/'.length)) };
    }
  }

  // Whatever is left is a path the document author meant as a workspace file.
  // Claimed even when it names nothing: `openFilePreview` renders a real load
  // error, whereas declining would let the iframe navigate to the app shell.
  return { kind: 'file', path: resolvePreviewRelativePath(ctx.artifactPath, href) };
}

function scrollPreviewToFragment(doc: Document, id: string): void {
  if (!id) {
    doc.defaultView?.scrollTo({ top: 0, behavior: 'smooth' });
    return;
  }
  // `querySelector`, not `getElementById` (the `#app`-only ban in
  // .claude/rules/frontend.md). An attribute selector rather than `#<id>` keeps
  // it safe for author-written ids without depending on `CSS.escape`: only the
  // quote and the backslash need escaping inside `[id="…"]`. `[name=…]` covers
  // the legacy `<a name>` anchor a hand-written report may still use.
  const quoted = id.replace(/["\\]/g, '\\$&');
  const target = doc.querySelector(`[id="${quoted}"]`) ?? doc.querySelector(`[name="${quoted}"]`);
  if (!target) {
    showToast(`No "${id}" section in this document`, 'error');
    return;
  }
  (target as HTMLElement).scrollIntoView({ behavior: 'smooth', block: 'start' });
}

function runPreviewLinkAction(action: PreviewLinkAction, doc: Document): void {
  switch (action.kind) {
    case 'fragment':
      scrollPreviewToFragment(doc, action.id);
      return;
    case 'thread':
      openThreadAcrossWorkspaces(action.workspace, action.threadId);
      return;
    case 'app':
      void openAppById(action.appId, 'a file preview');
      return;
    case 'nav':
      handleNavigationRequest({ target: action.target });
      return;
    case 'local-file':
      openLocalFile(action.target);
      return;
    case 'file':
      openFilePreview(action.path);
      return;
    case 'external':
      openUrl(action.url);
      return;
  }
}

/** Where a previewed document lives, and how much of its link behavior the host
 *  has to take over. */
export interface PreviewLinkHost {
  /** The document the previewed content is IN: the iframe's `contentDocument`
   *  for an HTML artifact, the host document for a markdown one. */
  doc: Document;
  /** Workspace-relative path of the previewed artifact. */
  artifactPath: string;
  /** The previewed document's own `<base href>`, when it declares one. Left
   *  undefined otherwise, which is the normal case. */
  documentBase?: string;
  /** Whether an in-page fragment must be claimed.
   *
   *  TRUE inside a srcdoc iframe, where the browser resolves `#x` against the
   *  HOST page URL and so navigates the iframe to the app shell.
   *
   *  FALSE for markdown rendered straight into the host document, where `#x`
   *  resolves to the page the user is already on: an ordinary same-document hash
   *  change that neither reloads nor destroys anything. (`marked` emits no
   *  heading ids either, so claiming it would only toast on every link.) */
  claimFragments: boolean;
}

/** Route one click on a previewed document's link. Exported (rather than kept
 *  behind `bridgePreviewIframeLinks`) because the markdown preview renders into
 *  the HOST document and so has a Preact `onClick` instead of a bridged
 *  listener, and because it lets the tests drive a synthetic event. */
export function handlePreviewLinkClick(e: MouseEvent, host: PreviewLinkHost): void {
  if (e.defaultPrevented) return;
  // A modifier or a non-primary button is the user explicitly asking the BROWSER
  // to act (new tab, new window, save link). Claiming those would break the one
  // behavior every link on the web has. Safe to hand back now that
  // `withPreviewBase` re-anchors the document: what the browser opens is the
  // artifact and its siblings, not the app shell.
  if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || (e.button ?? 0) !== 0) return;
  const target = e.target as { closest?: (sel: string) => Element | null } | null;
  const anchor = target?.closest?.('a[href]') ?? null;
  if (!anchor) return;
  // `download` says "save this", not "navigate here". Only the browser can.
  if (anchor.hasAttribute?.('download')) return;

  // A `.thread-link` the markdown renderer already resolved. Prefer its data
  // attributes over re-deriving from the href: `data-thread-workspace` carries
  // the workspace NAME the ref was written with, whereas the href carries its
  // SLUG, and only the name is what `openThreadAcrossWorkspaces` compares
  // against. Present only for host-rendered markdown; an artifact's own HTML
  // never carries them, so the srcdoc path falls straight through.
  const linkedThreadId = anchor.getAttribute('data-thread-id');
  if (linkedThreadId) {
    e.preventDefault();
    e.stopPropagation();
    openThreadAcrossWorkspaces(anchor.getAttribute('data-thread-workspace') ?? undefined, linkedThreadId);
    return;
  }

  // Guarded like `threadLinkHref` in utils/renderMarkdown.ts: `location` is
  // always there in the app, never in a bare unit-test environment.
  const page = typeof location === 'undefined' ? null : location;
  const action = classifyPreviewLink(anchor.getAttribute('href') ?? '', {
    artifactPath: host.artifactPath,
    hostOrigin: page?.origin ?? '',
    hostPath: page?.pathname ?? '/',
    workspaceId: WORKSPACE_ID,
    documentBase: host.documentBase,
  });
  if (!action) return;
  if (action.kind === 'fragment' && !host.claimFragments) return;
  e.preventDefault();
  e.stopPropagation();
  runPreviewLinkAction(action, host.doc);
}

// Each iframe load installs a fresh `contentDocument`; track the ones already
// wired so a reload that reuses a document (or a double `load`) can't stack
// listeners. WeakSet so a discarded document is collected with its listener.
const bridged = new WeakSet<Document>();

/** Wire host link routing into a preview iframe's same-origin document. Call
 *  from the iframe's `onLoad` (the `contentDocument` only exists once loaded),
 *  alongside `bridgePreviewIframeShortcuts`. No-op for a missing iframe or a
 *  cross-origin document. Capture phase so the routing decision is made before
 *  the previewed document's own handlers can act on the click. */
export function bridgePreviewIframeLinks(
  iframe: HTMLIFrameElement | null,
  opts: { artifactPath: string; declaresOwnBase: boolean },
): void {
  if (!iframe) return;
  let doc: Document | null;
  try {
    doc = iframe.contentDocument;
  } catch {
    return; // cross-origin preview: can't reach in
  }
  if (!doc || bridged.has(doc)) return;
  bridged.add(doc);
  const previewDoc = doc;
  // Only an artifact-declared base is threaded through: the one we stamp already
  // means "resolve against the artifact's folder", which is what the default
  // artifactPath resolution does.
  const documentBase = opts.declaresOwnBase ? previewDoc.baseURI : undefined;
  previewDoc.addEventListener(
    'click',
    (e) => {
      handlePreviewLinkClick(e as MouseEvent, {
        doc: previewDoc,
        artifactPath: opts.artifactPath,
        documentBase,
        claimFragments: true,
      });
    },
    true,
  );
}

/** The `<base href>` a previewed artifact should carry: its own folder, so
 *  relative asset refs (`img/chart.png`, `style.css`) resolve to its siblings.
 *  Without it they resolve against the HOST page URL and fetch the app shell. */
export function previewBaseHref(fileUrl: string): string {
  const queryAt = fileUrl.search(/[?#]/);
  const clean = queryAt === -1 ? fileUrl : fileUrl.slice(0, queryAt);
  const lastSlash = clean.lastIndexOf('/');
  return lastSlash === -1 ? clean : clean.slice(0, lastSlash + 1);
}

const DECLARED_BASE_RE = /<base\s[^>]*href\s*=/i;

/** Whether an artifact declares its own `<base href>`. Both `withPreviewBase`
 *  (which then leaves it alone) and the click bridge (which then resolves
 *  relative links against it) key off this. */
export function documentDeclaresBase(html: string): boolean {
  return DECLARED_BASE_RE.test(html);
}

/** Stamp `<base href>` into an artifact's HTML so the srcdoc document stops
 *  inheriting the host page's URL. A document that already declares its own
 *  `<base>` is returned untouched (only the first one counts per the HTML spec,
 *  and an artifact that set one meant it). */
export function withPreviewBase(html: string, baseHref: string): string {
  if (documentDeclaresBase(html)) return html;
  const tag = `<base href="${escapeHtmlAttr(baseHref)}">`;
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
  // No <head>/<html>: the parser builds them implicitly and a leading <base>
  // lands in the implicit head. It must still come AFTER any doctype, or the
  // doctype stops being a doctype and the document renders in quirks mode.
  const doctype = /^\s*<!doctype[^>]*>/i.exec(html);
  if (doctype) {
    return html.slice(0, doctype[0].length) + tag + html.slice(doctype[0].length);
  }
  return tag + html;
}
