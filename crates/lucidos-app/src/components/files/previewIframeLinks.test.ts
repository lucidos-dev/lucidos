import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  openFilePreview: vi.fn(),
  openUrl: vi.fn(),
  openLocalFile: vi.fn(),
  openAppById: vi.fn(async () => {}),
  openThreadAcrossWorkspaces: vi.fn(),
  handleNavigationRequest: vi.fn(),
  showToast: vi.fn(),
  // Mutable stand-in for basePath's load-time `WORKSPACE_ID` const, read via a
  // getter so the module under test sees the current value at call time.
  workspaceId: 'myws' as string | null,
}));

vi.mock('../../store/actions/artifacts', () => ({
  openFilePreview: mocks.openFilePreview,
  openUrl: mocks.openUrl,
  openLocalFile: mocks.openLocalFile,
}));
vi.mock('../../store/actions/apps', () => ({ openAppById: mocks.openAppById }));
vi.mock('../../store/actions/cross-workspace', () => ({
  openThreadAcrossWorkspaces: mocks.openThreadAcrossWorkspaces,
}));
vi.mock('../../store/actions/navigation-request', () => ({
  handleNavigationRequest: mocks.handleNavigationRequest,
}));
vi.mock('../../store/store', async () => {
  const actual = await vi.importActual<typeof import('../../store/store')>('../../store/store');
  return { ...actual, showToast: mocks.showToast };
});
// Partial: other modules pulled in transitively read BASE_PATH / API off the
// same module, so only WORKSPACE_ID is overridden.
vi.mock('../../utils/basePath', async () => {
  const actual = await vi.importActual<typeof import('../../utils/basePath')>('../../utils/basePath');
  return {
    ...actual,
    get WORKSPACE_ID() {
      return mocks.workspaceId;
    },
  };
});

const {
  classifyPreviewLink,
  resolvePreviewRelativePath,
  handlePreviewLinkClick,
  bridgePreviewIframeLinks,
  previewBaseHref,
  withPreviewBase,
  documentDeclaresBase,
} = await import('./previewIframeLinks');

const TID = '961b9b83-53b7-47cd-8982-3c959d7f1137';

const ctx = (over: Partial<Parameters<typeof classifyPreviewLink>[1]> = {}) => ({
  artifactPath: 'artifacts/reports/pr-1573.html',
  hostOrigin: 'https://localhost:5251',
  hostPath: '/myws/',
  workspaceId: 'myws' as string | null,
  ...over,
});

// ---------------------------------------------------------------------------
// classifyPreviewLink: the routing table
// ---------------------------------------------------------------------------

describe('classifyPreviewLink', () => {
  it('claims a bare in-page anchor as a fragment scroll (the reported bug)', () => {
    // A generated report's table of contents. In an `about:srcdoc` document this
    // resolves against the HOST page URL, so unclaimed it loads the whole app
    // shell into the content pane.
    expect(classifyPreviewLink('#the-three-pinned-values-old-new', ctx())).toEqual({
      kind: 'fragment',
      id: 'the-three-pinned-values-old-new',
    });
  });

  it('claims the absolute spelling of that same in-page anchor', () => {
    expect(
      classifyPreviewLink('https://localhost:5251/myws/#the-three-pinned-values-old-new', ctx()),
    ).toEqual({ kind: 'fragment', id: 'the-three-pinned-values-old-new' });
  });

  it('treats a bare `#` as scroll-to-top', () => {
    expect(classifyPreviewLink('#', ctx())).toEqual({ kind: 'fragment', id: '' });
  });

  it('percent-decodes a fragment so it matches the element id', () => {
    expect(classifyPreviewLink('#a%20b', ctx())).toEqual({ kind: 'fragment', id: 'a b' });
  });

  it('routes a workspace-qualified thread: link', () => {
    expect(classifyPreviewLink(`thread:dev/${TID}`, ctx())).toEqual({
      kind: 'thread',
      workspace: 'dev',
      threadId: TID,
    });
  });

  it('routes a bare thread: link as same-workspace', () => {
    expect(classifyPreviewLink(`thread:${TID}`, ctx())).toEqual({
      kind: 'thread',
      workspace: undefined,
      threadId: TID,
    });
  });

  it('routes the `#thread=` landing form and drops the slug when it is our own', () => {
    expect(classifyPreviewLink(`https://localhost:5251/myws/#thread=${TID}`, ctx())).toEqual({
      kind: 'thread',
      workspace: undefined,
      threadId: TID,
    });
  });

  it('routes a `#thread=` link that names a different workspace', () => {
    expect(classifyPreviewLink(`https://localhost:5251/other-ws/#thread=${TID}`, ctx())).toEqual({
      kind: 'thread',
      workspace: 'other-ws',
      threadId: TID,
    });
  });

  it('sends an off-origin http link to a new tab', () => {
    expect(classifyPreviewLink('https://example.com/x', ctx())).toEqual({
      kind: 'external',
      url: 'https://example.com/x',
    });
  });

  it('sends a same-origin app URL with no in-app destination to a new tab, not the iframe', () => {
    expect(classifyPreviewLink('https://localhost:5251/other-ws/', ctx())).toEqual({
      kind: 'external',
      url: 'https://localhost:5251/other-ws/',
    });
  });

  it('routes an app entry-point href', () => {
    expect(classifyPreviewLink('app:habit-tracker', ctx())).toEqual({
      kind: 'app',
      appId: 'habit-tracker',
    });
  });

  it('routes a nav-panel href', () => {
    expect(classifyPreviewLink('notifications', ctx())).toEqual({
      kind: 'nav',
      target: 'notifications',
    });
  });

  it('routes an absolute filesystem path to the OS opener', () => {
    expect(classifyPreviewLink('/Applications/Thing.app', ctx())).toEqual({
      kind: 'local-file',
      target: '/Applications/Thing.app',
    });
  });

  it.each([
    ['/artifacts/report.pdf', 'artifacts/report.pdf'],
    ['/knowhow/myapp/notes.md', 'knowhow/myapp/notes.md'],
    ['/triggers/daily/run.md', 'triggers/daily/run.md'],
    ['/system-knowhow/js-sdk.md', 'system-knowhow/js-sdk.md'],
    ['/apps/todo/styles.css', 'apps/todo/styles.css'],
    ['/data/artifacts/report.pdf', 'artifacts/report.pdf'],
  ])('previews the absolute workspace route %s instead of OS-opening it', (href, path) => {
    // `extractLocalFileTarget` guards every `data/` sub-tree, not just `/data`
    // and `/apps`, so a root-relative link inside a previewed document reaches
    // the file branch below it. Before that widening, `/artifacts/report.pdf`
    // was handed to the OS opener as a disk path that does not exist.
    expect(classifyPreviewLink(href, ctx())).toEqual({ kind: 'file', path });
  });

  it('resolves a sibling file against the previewed artifact folder', () => {
    expect(classifyPreviewLink('pr-1573.md', ctx())).toEqual({
      kind: 'file',
      path: 'artifacts/reports/pr-1573.md',
    });
  });

  it('leaves a scheme it does not own to the browser', () => {
    expect(classifyPreviewLink('mailto:someone@example.com', ctx())).toBeNull();
    expect(classifyPreviewLink('tel:+4700000000', ctx())).toBeNull();
  });

  it('leaves an empty href alone', () => {
    expect(classifyPreviewLink('', ctx())).toBeNull();
    expect(classifyPreviewLink('   ', ctx())).toBeNull();
  });

  // `repo:` is a URL scheme, so before this arm the guard above handed a repo
  // citation back to the browser and the link dead-ended. That is why a report
  // citing repo code had to be published as an app rather than as an artifact.
  describe('a repo-encoded citation', () => {
    const ENCODED = 'repo:repo-1:file:src/main.rs';

    it('routes a bare repo file', () => {
      expect(classifyPreviewLink(ENCODED, ctx())).toEqual({ kind: 'repo-file', filePath: ENCODED });
    });

    it('carries a single cited line', () => {
      expect(classifyPreviewLink(`${ENCODED}#L510`, ctx())).toEqual({
        kind: 'repo-file',
        filePath: ENCODED,
        line: 510,
        lineEnd: undefined,
      });
    });

    it.each([
      ['#L510-L520', 510, 520],
      ['#L510-520', 510, 520],
    ])('carries a cited range written as %s', (frag, line, lineEnd) => {
      expect(classifyPreviewLink(`${ENCODED}${frag}`, ctx()))
        .toEqual({ kind: 'repo-file', filePath: ENCODED, line, lineEnd });
    });

    it('keeps a path that contains colons intact', () => {
      const weird = 'repo:repo-1:file:src/weird:name.rs';
      expect(classifyPreviewLink(`${weird}#L7`, ctx()))
        .toEqual({ kind: 'repo-file', filePath: weird, line: 7, lineEnd: undefined });
    });

    // Two `#` in one href, and they mean different things: the one inside the
    // mode segment names the revision, the trailing one names the line. The
    // line suffix is `$`-anchored and stripped before `parseRepoPath` is asked,
    // so the two never compete.
    it('carries a named ref and a cited range together', () => {
      const atRef = 'repo:repo-1:file#origin/main:src/main.rs';
      expect(classifyPreviewLink(`${atRef}#L10-L20`, ctx()))
        .toEqual({ kind: 'repo-file', filePath: atRef, line: 10, lineEnd: 20 });
    });

    it('routes a bare repo file at a named ref', () => {
      const atRef = 'repo:repo-1:file#v1.2.0:src/main.rs';
      expect(classifyPreviewLink(atRef, ctx())).toEqual({ kind: 'repo-file', filePath: atRef });
    });

    // parseRepoPath stays the single predicate: a structurally incomplete
    // encoding is not a repo path, so the href falls through to the existing
    // scheme guard and the browser keeps it, exactly as before.
    it.each([
      'repo::file:src/main.rs',
      'repo:repo-1:file:',
      'repo:repo-1:weird:a.md',
      'repo:repo-1:file#:src/main.rs',
      'repo:',
    ])('declines the malformed encoding %s', (href) => {
      expect(classifyPreviewLink(href, ctx())).toBeNull();
    });

    // Only `#L<n>` is a line reference. Anything else stays part of the path,
    // which then 404s in the preview: the same choice the data-path branch
    // makes, and better than letting the iframe navigate to the app shell.
    it('does not read a non-line fragment as a line', () => {
      expect(classifyPreviewLink(`${ENCODED}#section`, ctx())).toEqual({
        kind: 'repo-file',
        filePath: `${ENCODED}#section`,
      });
    });
  });
});

describe('resolvePreviewRelativePath', () => {
  const from = 'artifacts/reports/pr-1573.html';

  it('resolves a sibling', () => {
    expect(resolvePreviewRelativePath(from, 'notes.md')).toBe('artifacts/reports/notes.md');
  });

  it('resolves a parent-relative path', () => {
    expect(resolvePreviewRelativePath(from, '../summary.md')).toBe('artifacts/summary.md');
  });

  it('anchors a data/-prefixed path at the data root', () => {
    expect(resolvePreviewRelativePath(from, 'data/artifacts/x.md')).toBe('artifacts/x.md');
    expect(resolvePreviewRelativePath(from, '/data/artifacts/x.md')).toBe('artifacts/x.md');
  });

  it('drops a query string and fragment', () => {
    expect(resolvePreviewRelativePath(from, 'notes.md?v=2#top')).toBe('artifacts/reports/notes.md');
  });
});

// ---------------------------------------------------------------------------
// The DOM bridge
// ---------------------------------------------------------------------------

/** A preview iframe's same-origin `contentDocument`, faked for the node test env
 *  (no jsdom): records click listeners, resolves fragment targets, and records
 *  what got scrolled. */
function fakeContentDoc(ids: string[] = []) {
  const handlers: { fn: (e: unknown) => void; capture: boolean }[] = [];
  const scrolled: string[] = [];
  let scrolledToTop = false;
  return {
    addEventListener: (type: string, fn: (e: unknown) => void, capture?: boolean) => {
      if (type === 'click') handlers.push({ fn, capture: capture === true });
    },
    removeEventListener: () => {},
    /** Resolves the `[id="…"]` / `[name="…"]` selectors the bridge builds. */
    querySelector: (selector: string) => {
      const m = /^\[(?:id|name)="(.*)"\]$/.exec(selector);
      const id = m?.[1].replace(/\\(["\\])/g, '$1');
      return id !== undefined && ids.includes(id)
        ? ({ scrollIntoView: () => { scrolled.push(id); } } as unknown as HTMLElement)
        : null;
    },
    defaultView: { scrollTo: () => { scrolledToTop = true; } },
    /** test-only */
    listenerCount: () => handlers.length,
    allCapture: () => handlers.every((h) => h.capture),
    scrolledIds: () => scrolled,
    didScrollToTop: () => scrolledToTop,
  };
}
type FakeDoc = ReturnType<typeof fakeContentDoc>;
const iframeWith = (doc: FakeDoc | null) =>
  ({ contentDocument: doc } as unknown as HTMLIFrameElement);
const BRIDGE_OPTS = { artifactPath: 'artifacts/reports/pr-1573.html', declaresOwnBase: false };

/** A click whose target resolves to an anchor with `href`. `href: null` models a
 *  click that hit no anchor at all; `attrs` adds the `data-thread-*` pair the
 *  markdown renderer stamps on a resolved thread link, or a `download` flag. */
function clickOn(
  href: string | null,
  attrs: Record<string, string> = {},
  modifiers: Partial<{ metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean; button: number }> = {},
) {
  const anchor =
    href === null
      ? null
      : {
        getAttribute: (name: string) => (name === 'href' ? href : attrs[name] ?? null),
        hasAttribute: (name: string) => name in attrs,
      };
  const e = {
    defaultPrevented: false,
    metaKey: false,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    button: 0,
    ...modifiers,
    target: { closest: () => anchor },
    preventDefault() { e.defaultPrevented = true; },
    stopPropagation() {},
  };
  return e;
}

/** Run the bridge's click handling against a fake preview document, as the
 *  srcdoc iframe does (fragments claimed). */
function click(
  doc: FakeDoc,
  href: string | null,
  artifactPath = 'artifacts/x.html',
  extra: { attrs?: Record<string, string>; modifiers?: Parameters<typeof clickOn>[2]; documentBase?: string } = {},
) {
  const e = clickOn(href, extra.attrs, extra.modifiers);
  handlePreviewLinkClick(e as unknown as MouseEvent, {
    doc: doc as unknown as Document,
    artifactPath,
    documentBase: extra.documentBase,
    claimFragments: true,
  });
  return e;
}

/** Same, as the host-rendered markdown preview does (fragments left to the
 *  browser, since they are an ordinary same-document hash change there). */
function clickInMarkdown(doc: FakeDoc, href: string | null, artifactPath = 'artifacts/x.md') {
  const e = clickOn(href);
  handlePreviewLinkClick(e as unknown as MouseEvent, {
    doc: doc as unknown as Document,
    artifactPath,
    claimFragments: false,
  });
  return e;
}

describe('bridgePreviewIframeLinks', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.workspaceId = 'myws';
    vi.stubGlobal('location', { origin: 'https://localhost:5251', pathname: '/myws/' });
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('registers a single capture-phase click listener on the preview document', () => {
    const doc = fakeContentDoc();
    bridgePreviewIframeLinks(iframeWith(doc), BRIDGE_OPTS);
    expect(doc.listenerCount()).toBe(1);
    expect(doc.allCapture()).toBe(true);
  });

  it('attaches once per document, so a second bridge call does not stack listeners', () => {
    const doc = fakeContentDoc();
    bridgePreviewIframeLinks(iframeWith(doc), BRIDGE_OPTS);
    bridgePreviewIframeLinks(iframeWith(doc), BRIDGE_OPTS);
    expect(doc.listenerCount()).toBe(1);
  });

  it('no-ops on a cross-origin preview (contentDocument access throws)', () => {
    const crossOrigin = {
      get contentDocument(): Document {
        throw new Error('cross-origin');
      },
    } as unknown as HTMLIFrameElement;
    expect(() => bridgePreviewIframeLinks(crossOrigin, BRIDGE_OPTS)).not.toThrow();
  });

  it('no-ops on a null iframe', () => {
    expect(() => bridgePreviewIframeLinks(null, BRIDGE_OPTS)).not.toThrow();
  });

  it('scrolls in-document for a fragment click and never navigates', () => {
    const doc = fakeContentDoc(['section-two']);
    const e = click(doc, '#section-two');
    expect(e.defaultPrevented).toBe(true);
    expect(doc.scrolledIds()).toEqual(['section-two']);
    expect(mocks.openUrl).not.toHaveBeenCalled();
    expect(mocks.openFilePreview).not.toHaveBeenCalled();
  });

  it('scrolls to the top for a bare `#`', () => {
    const doc = fakeContentDoc();
    expect(click(doc, '#').defaultPrevented).toBe(true);
    expect(doc.didScrollToTop()).toBe(true);
  });

  it('reports a fragment the document does not contain instead of failing silently', () => {
    const doc = fakeContentDoc();
    const e = click(doc, '#nope');
    expect(e.defaultPrevented).toBe(true); // still suppressed: navigating loads the app shell
    expect(mocks.showToast).toHaveBeenCalledTimes(1);
    expect(mocks.showToast.mock.calls[0][0]).toContain('nope');
  });

  it('routes a thread link through the host router', () => {
    const e = click(fakeContentDoc(), `thread:dev/${TID}`);
    expect(e.defaultPrevented).toBe(true);
    expect(mocks.openThreadAcrossWorkspaces).toHaveBeenCalledWith('dev', TID);
  });

  it('routes a sibling file through openFilePreview, which is what gives it a nav-history entry', () => {
    const e = click(fakeContentDoc(), 'pr-1573.md', 'artifacts/reports/pr-1573.html');
    expect(e.defaultPrevented).toBe(true);
    expect(mocks.openFilePreview).toHaveBeenCalledWith('artifacts/reports/pr-1573.md');
  });

  it('opens an external link in a new tab', () => {
    const e = click(fakeContentDoc(), 'https://example.com/docs');
    expect(e.defaultPrevented).toBe(true);
    expect(mocks.openUrl).toHaveBeenCalledWith('https://example.com/docs');
  });

  // The whole point of the repo arm: a citation written in an artifact lands on
  // the cited line, through the same navigate router the SDK call reaches.
  it('routes a repo citation through the navigate router, lines and all', () => {
    const e = click(fakeContentDoc(), 'repo:repo-1:file:src/main.rs#L510-L520');
    expect(e.defaultPrevented).toBe(true);
    expect(mocks.handleNavigationRequest).toHaveBeenCalledWith(
      { target: 'file', file_path: 'repo:repo-1:file:src/main.rs', line: 510, line_end: 520 },
      { source: 'a file preview' },
    );
    expect(mocks.openFilePreview).not.toHaveBeenCalled();
  });

  it('leaves an unclaimed href and a non-anchor click completely alone', () => {
    const doc = fakeContentDoc();
    expect(click(doc, 'mailto:someone@example.com').defaultPrevented).toBe(false);
    expect(click(doc, null).defaultPrevented).toBe(false);
  });

  it('hands a modified or non-primary click back to the browser', () => {
    // Command/ctrl/shift-click and a middle click all mean "browser, you take
    // this". Intercepting them would break the one behavior every link has.
    const doc = fakeContentDoc(['x']);
    for (const modifiers of [
      { metaKey: true },
      { ctrlKey: true },
      { shiftKey: true },
      { altKey: true },
      { button: 1 },
    ]) {
      expect(click(doc, 'pr-1573.md', 'artifacts/reports/pr.html', { modifiers }).defaultPrevented).toBe(false);
    }
    expect(mocks.openFilePreview).not.toHaveBeenCalled();
  });

  it('leaves a download link to the browser', () => {
    const e = click(fakeContentDoc(), 'report.csv', 'artifacts/reports/pr.html', {
      attrs: { download: '' },
    });
    expect(e.defaultPrevented).toBe(false);
    expect(mocks.openFilePreview).not.toHaveBeenCalled();
  });

  it('resolves a relative link against a base the artifact declared itself', () => {
    // `withPreviewBase` leaves an artifact-declared <base> alone, so the routing
    // has to honour it too: this is an external page, not a workspace file.
    const e = click(fakeContentDoc(), 'guide.html', 'artifacts/reports/pr.html', {
      documentBase: 'https://example.com/docs/',
    });
    expect(e.defaultPrevented).toBe(true);
    expect(mocks.openUrl).toHaveBeenCalledWith('https://example.com/docs/guide.html');
    expect(mocks.openFilePreview).not.toHaveBeenCalled();
  });

  it('maps a declared base that points back into the workspace data mount to a file', () => {
    const e = click(fakeContentDoc(), 'guide.md', 'artifacts/reports/pr.html', {
      documentBase: 'https://localhost:5251/myws/data/artifacts/manuals/',
    });
    expect(e.defaultPrevented).toBe(true);
    expect(mocks.openFilePreview).toHaveBeenCalledWith('artifacts/manuals/guide.md');
  });

  it('respects a click another handler already claimed', () => {
    const doc = fakeContentDoc(['x']);
    const e = clickOn('#x');
    e.defaultPrevented = true;
    handlePreviewLinkClick(e as unknown as MouseEvent, {
      doc: doc as unknown as Document,
      artifactPath: 'artifacts/x.html',
      claimFragments: true,
    });
    expect(doc.scrolledIds()).toEqual([]);
  });
});

// A markdown artifact renders into the HOST document, so its relative links
// resolve against the engine-stamped `<base href="/<slug>/">` and reload the
// whole workspace through the SPA fallback. Same routing, one difference:
// fragments there are a harmless same-document hash change, so they stay with
// the browser.
describe('markdown preview links (rendered in the host document)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.workspaceId = 'myws';
    vi.stubGlobal('location', { origin: 'https://localhost:5251', pathname: '/myws/' });
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('routes a sibling link through the file preview instead of reloading the workspace', () => {
    const e = clickInMarkdown(fakeContentDoc(), 'notes.md', 'artifacts/reports/pr-1573.md');
    expect(e.defaultPrevented).toBe(true);
    expect(mocks.openFilePreview).toHaveBeenCalledWith('artifacts/reports/notes.md');
  });

  it('routes a thread link', () => {
    const e = clickInMarkdown(fakeContentDoc(), `thread:other-ws/${TID}`);
    expect(e.defaultPrevented).toBe(true);
    expect(mocks.openThreadAcrossWorkspaces).toHaveBeenCalledWith('other-ws', TID);
  });

  it('prefers a resolved thread link\'s data attributes over its slug-bearing href', () => {
    // The href carries the workspace SLUG; `data-thread-workspace` carries the
    // NAME the ref was written with, which is what the router compares against.
    const e = clickOn(`https://localhost:5251/my-workspace/#thread=${TID}`, {
      'data-thread-id': TID,
      'data-thread-workspace': 'My Workspace',
    });
    handlePreviewLinkClick(e as unknown as MouseEvent, {
      doc: fakeContentDoc() as unknown as Document,
      artifactPath: 'artifacts/x.md',
      claimFragments: false,
    });
    expect(e.defaultPrevented).toBe(true);
    expect(mocks.openThreadAcrossWorkspaces).toHaveBeenCalledWith('My Workspace', TID);
  });

  it('leaves an in-page fragment to the browser', () => {
    const doc = fakeContentDoc(['section-two']);
    const e = clickInMarkdown(doc, '#section-two');
    expect(e.defaultPrevented).toBe(false);
    expect(doc.scrolledIds()).toEqual([]);
    expect(mocks.showToast).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// <base> stamping
// ---------------------------------------------------------------------------

describe('documentDeclaresBase', () => {
  it('detects an artifact that sets its own base, and only that', () => {
    expect(documentDeclaresBase('<html><head><base href="https://x/"></head></html>')).toBe(true);
    expect(documentDeclaresBase('<html><head><title>t</title></head></html>')).toBe(false);
    // A `<basefont>` is not a `<base>`, and neither is prose about one.
    expect(documentDeclaresBase('<p>set a base href in the head</p>')).toBe(false);
  });
});

describe('previewBaseHref', () => {
  it('is the artifact folder, with the cache-busting query dropped', () => {
    expect(previewBaseHref('https://h/myws/data/artifacts/reports/pr.html?v=3')).toBe(
      'https://h/myws/data/artifacts/reports/',
    );
  });
});

describe('withPreviewBase', () => {
  const BASE = 'https://h/myws/data/artifacts/reports/';

  it('inserts the base as the first thing in <head>', () => {
    const out = withPreviewBase(
      '<!DOCTYPE html><html><head><title>T</title></head><body>x</body></html>',
      BASE,
    );
    expect(out).toContain(`<head><base href="${BASE}"><title>`);
  });

  it('tolerates an attributed and uppercased head tag', () => {
    const out = withPreviewBase('<HTML><HEAD lang="en"><title>T</title></HEAD></HTML>', BASE);
    expect(out).toContain(`<HEAD lang="en"><base href="${BASE}">`);
  });

  it('creates a head when the document has <html> but no <head>', () => {
    const out = withPreviewBase('<html><body>x</body></html>', BASE);
    expect(out).toBe(`<html><head><base href="${BASE}"></head><body>x</body></html>`);
  });

  it('keeps the doctype first in a head-less document, so the page stays out of quirks mode', () => {
    const out = withPreviewBase('<!DOCTYPE html>\n<p>x</p>', BASE);
    expect(out.startsWith('<!DOCTYPE html>')).toBe(true);
    expect(out).toContain(`<base href="${BASE}">`);
  });

  it('prepends to a bare fragment', () => {
    expect(withPreviewBase('<p>x</p>', BASE)).toBe(`<base href="${BASE}"><p>x</p>`);
  });

  it('leaves a document that declares its own base untouched', () => {
    const html = '<html><head><base href="https://elsewhere/"></head></html>';
    expect(withPreviewBase(html, BASE)).toBe(html);
  });

  it('escapes the base href', () => {
    expect(withPreviewBase('<p>x</p>', 'https://h/a"b/')).toContain('href="https://h/a&quot;b/"');
  });
});
