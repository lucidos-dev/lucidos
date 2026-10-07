import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  openFilePreview: vi.fn(),
  openUrl: vi.fn(),
  openLocalFileOnConfirm: vi.fn(async () => {}),
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
  openLocalFileOnConfirm: mocks.openLocalFileOnConfirm,
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
  resolvePreviewRelativeRepoPath,
  handlePreviewLinkClick,
  previewBaseHref,
  withPreviewBase,
  withPreviewSizing,
  documentDeclaresBase,
  PREVIEW_FIND_RULES,
} = await import('./previewIframeLinks');
const { ALL_HIGHLIGHT, CURRENT_HIGHLIGHT } = await import('@lucidos/find');
const { parseRepoPath, encodeRepoPath, repositories } = await import('../../store/store');

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
      fragment: undefined,
    });
  });

  it('carries an app fragment through to the action', () => {
    // A published report citing one item inside a shared app. Without the
    // fragment the citation opens the app on whatever the reader saw last.
    expect(classifyPreviewLink('app:pr-understanding#pr-1645', ctx())).toEqual({
      kind: 'app',
      appId: 'pr-understanding',
      fragment: 'pr-1645',
    });
  });

  it('keeps an in-page anchor an in-page anchor, not an app fragment', () => {
    // `#x` names a place in THIS document. The app arm must never see it.
    expect(classifyPreviewLink('#pr-1645', ctx())).toEqual({
      kind: 'fragment',
      id: 'pr-1645',
    });
  });

  it('routes a nav-panel href', () => {
    expect(classifyPreviewLink('notifications', ctx())).toEqual({
      kind: 'nav',
      target: 'notifications',
    });
  });

  it('routes a trigger href', () => {
    // `trigger:` is a scheme. Without its own arm the scheme bail-out below
    // hands it to the browser, and a report citing a trigger dead-ends.
    expect(classifyPreviewLink('trigger:3f9b21c4-0a7e', ctx())).toEqual({
      kind: 'trigger',
      triggerId: '3f9b21c4-0a7e',
    });
  });

  it('routes a settings page href to that page', () => {
    expect(classifyPreviewLink('settings:backup', ctx())).toEqual({
      kind: 'nav',
      target: 'settings',
      settingsView: 'backup',
    });
  });

  it('keeps the triggers PANEL on the nav arm', () => {
    expect(classifyPreviewLink('triggers', ctx())).toEqual({
      kind: 'nav',
      target: 'triggers',
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
      expect(classifyPreviewLink(ENCODED, ctx())).toEqual({ kind: 'repo-file', locator: parseRepoPath(ENCODED) });
    });

    it('carries a single cited line', () => {
      expect(classifyPreviewLink(`${ENCODED}#L510`, ctx())).toEqual({
        kind: 'repo-file',
        locator: parseRepoPath(ENCODED),
        line: 510,
        lineEnd: undefined,
      });
    });

    it.each([
      ['#L510-L520', 510, 520],
      ['#L510-520', 510, 520],
    ])('carries a cited range written as %s', (frag, line, lineEnd) => {
      expect(classifyPreviewLink(`${ENCODED}${frag}`, ctx()))
        .toEqual({ kind: 'repo-file', locator: parseRepoPath(ENCODED), line, lineEnd });
    });

    it('keeps a path that contains colons intact', () => {
      const weird = 'repo:repo-1:file:src/weird:name.rs';
      expect(classifyPreviewLink(`${weird}#L7`, ctx()))
        .toEqual({ kind: 'repo-file', locator: parseRepoPath(weird), line: 7, lineEnd: undefined });
    });

    // Two `#` in one href, and they mean different things: the one inside the
    // mode segment names the revision, the trailing one names the line. The
    // line suffix is `$`-anchored and stripped before `parseRepoPath` is asked,
    // so the two never compete.
    it('carries a named ref and a cited range together', () => {
      const atRef = 'repo:repo-1:file#origin/main:src/main.rs';
      expect(classifyPreviewLink(`${atRef}#L10-L20`, ctx()))
        .toEqual({ kind: 'repo-file', locator: parseRepoPath(atRef), line: 10, lineEnd: 20 });
    });

    it('routes a bare repo file at a named ref', () => {
      const atRef = 'repo:repo-1:file#v1.2.0:src/main.rs';
      expect(classifyPreviewLink(atRef, ctx())).toEqual({ kind: 'repo-file', locator: parseRepoPath(atRef) });
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
        locator: parseRepoPath(`${ENCODED}#section`),
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

describe('resolvePreviewRelativeRepoPath', () => {
  const locator = { repoId: 'repo-1', mode: 'file' as const, ref: 'main', path: 'docs/notes/start.md' };

  it('resolves a sibling inside the checkout', () => {
    expect(resolvePreviewRelativeRepoPath(locator, 'intro.md'))
      .toEqual({ ...locator, path: 'docs/notes/intro.md' });
  });

  it('resolves a parent-relative path, preserving repoId/mode/ref', () => {
    expect(resolvePreviewRelativeRepoPath(locator, '../guide.md'))
      .toEqual({ ...locator, path: 'docs/guide.md' });
  });

  it('anchors a leading slash at the checkout root, not the workspace data root', () => {
    expect(resolvePreviewRelativeRepoPath(locator, '/README.md'))
      .toEqual({ ...locator, path: 'README.md' });
  });

  it('never climbs past the checkout root, however many `..` the href carries', () => {
    expect(resolvePreviewRelativeRepoPath(locator, '../../../../../etc/passwd'))
      .toEqual({ ...locator, path: 'etc/passwd' });
  });

  it('drops a query string and fragment', () => {
    expect(resolvePreviewRelativeRepoPath(locator, 'intro.md?v=2#top'))
      .toEqual({ ...locator, path: 'docs/notes/intro.md' });
  });

  it('carries a diff locator\'s changeId through untouched', () => {
    const diffLocator = { repoId: 'repo-1', mode: 'diff' as const, changeId: 'change-7', path: 'docs/start.md' };
    expect(resolvePreviewRelativeRepoPath(diffLocator, '../guide.md'))
      .toEqual({ ...diffLocator, path: 'guide.md' });
  });
});

describe('classifyPreviewLink with a repo-flavored context', () => {
  const repoLocator = { repoId: 'repo-1', mode: 'file' as const, ref: 'main', path: 'docs/notes/start.md' };
  const repoCtx = () => ctx({ repoLocator });

  it('resolves a relative sibling against the checkout, encoded for the unified file preview', () => {
    expect(classifyPreviewLink('../guide.md', repoCtx())).toEqual({
      kind: 'file',
      path: encodeRepoPath({ ...repoLocator, path: 'docs/guide.md' }),
    });
  });

  it('anchors a root-relative href at the checkout, never the OS filesystem', () => {
    // Without a repoLocator, a leading `/` is an absolute OS path
    // (`extractLocalFileTarget`). A repo preview means something else by it.
    expect(classifyPreviewLink('/README.md', repoCtx())).toEqual({
      kind: 'file',
      path: encodeRepoPath({ ...repoLocator, path: 'README.md' }),
    });
  });

  it('still routes a thread/app/trigger/settings/repo/absolute-URL href unchanged', () => {
    expect(classifyPreviewLink(`thread:${TID}`, repoCtx())).toEqual({
      kind: 'thread', workspace: undefined, threadId: TID,
    });
    expect(classifyPreviewLink('app:habit-tracker', repoCtx())).toEqual({
      kind: 'app', appId: 'habit-tracker', fragment: undefined,
    });
    expect(classifyPreviewLink('trigger:3f9b21c4-0a7e', repoCtx())).toEqual({
      kind: 'trigger', triggerId: '3f9b21c4-0a7e',
    });
    expect(classifyPreviewLink('settings:backup', repoCtx())).toEqual({
      kind: 'nav', target: 'settings', settingsView: 'backup',
    });
    expect(classifyPreviewLink('repo:repo-2:file:src/main.rs', repoCtx())).toEqual({
      kind: 'repo-file', locator: parseRepoPath('repo:repo-2:file:src/main.rs'),
    });
    expect(classifyPreviewLink('https://example.com/x', repoCtx())).toEqual({
      kind: 'external', url: 'https://example.com/x',
    });
  });

  it('leaves an in-page fragment a fragment, not a repo-resolved file', () => {
    expect(classifyPreviewLink('#section', repoCtx())).toEqual({ kind: 'fragment', id: 'section' });
  });

  // A `file://` URL is unambiguous in any context, unlike a bare leading `/`:
  // it must keep going to the OS opener even when previewing a repo file.
  it('still hands a file:// URL to the OS opener', () => {
    expect(classifyPreviewLink('file:///Users/me/notes.txt', repoCtx())).toEqual({
      kind: 'local-file', target: 'file:///Users/me/notes.txt',
    });
  });
});

// ---------------------------------------------------------------------------
// Markdown preview clicks (rendered in the host document)
// ---------------------------------------------------------------------------

/** A click whose target resolves to an anchor with `href`. `href: null` models a
 *  click that hit no anchor at all; `attrs` adds the `data-thread-*` pair the
 *  markdown renderer stamps on a resolved thread link, or a `download` flag. */
function clickOn(
  href: string | null,
  attrs: Record<string, string> = {},
  modifiers: Partial<{ metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean; button: number }> = {},
  documentRoot: ReturnType<typeof renderedDocument>['root'] | null = null,
) {
  const anchor =
    href === null
      ? null
      : {
        getAttribute: (name: string) => (name === 'href' ? href : attrs[name] ?? null),
        hasAttribute: (name: string) => name in attrs,
        closest: (selector: string) => (selector === '.markdown-content' ? documentRoot : null),
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

/** A rendered markdown document holding elements with these ids or names. */
function renderedDocument(elements: { id?: string; name?: string }[]) {
  const nodes = elements.map((el) => ({
    id: el.id ?? '',
    getAttribute: (attr: string) => (attr === 'name' ? el.name ?? null : null),
    scrollIntoView: vi.fn(),
  }));
  return { nodes, root: { querySelectorAll: () => nodes, scrollIntoView: vi.fn() } };
}

function clickInMarkdown(
  href: string | null,
  artifactPath = 'artifacts/x.md',
  extra: { attrs?: Record<string, string>; modifiers?: Parameters<typeof clickOn>[2] } = {},
) {
  const e = clickOn(href, extra.attrs, extra.modifiers);
  handlePreviewLinkClick(e as unknown as MouseEvent, artifactPath);
  return e;
}

// A markdown artifact renders into the HOST document. So its relative links
// resolve against the engine-stamped `<base href="/<slug>/">`, and would reload
// the whole workspace through the SPA fallback.
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
    const e = clickInMarkdown('notes.md', 'artifacts/reports/pr-1573.md');
    expect(e.defaultPrevented).toBe(true);
    expect(mocks.openFilePreview).toHaveBeenCalledWith('artifacts/reports/notes.md');
  });

  it('routes a thread link', () => {
    const e = clickInMarkdown(`thread:other-ws/${TID}`);
    expect(e.defaultPrevented).toBe(true);
    expect(mocks.openThreadAcrossWorkspaces).toHaveBeenCalledWith('other-ws', TID);
  });

  it('routes a repo citation through the navigate router, lines and all', () => {
    repositories.value = { status: 'loaded', data: [{ id: 'repo-1', name: 'example-repo', path: '/src/example' }] };
    const e = clickInMarkdown('repo:repo-1:file:src/main.rs#L510-L520');
    expect(e.defaultPrevented).toBe(true);
    expect(mocks.handleNavigationRequest).toHaveBeenCalledWith(
      { target: 'file', file_path: 'repo:repo-1:file:src/main.rs', line: 510, line_end: 520 },
      { source: 'a file preview' },
    );
  });

  it('resolves a repo citation that names its repository', () => {
    repositories.value = { status: 'loaded', data: [{ id: 'repo-1', name: 'example-repo', path: '/src/example' }] };
    clickInMarkdown('repo:example-repo:file:src/main.rs');
    expect(mocks.handleNavigationRequest).toHaveBeenCalledWith(
      expect.objectContaining({ file_path: 'repo:repo-1:file:src/main.rs' }),
      { source: 'a file preview' },
    );
  });

  it('prefers a resolved thread link\'s data attributes over its slug-bearing href', () => {
    // The href carries the workspace SLUG; `data-thread-workspace` carries the
    // NAME the ref was written with, which is what the router compares against.
    const e = clickInMarkdown(`https://localhost:5251/my-workspace/#thread=${TID}`, 'artifacts/x.md', {
      attrs: { 'data-thread-id': TID, 'data-thread-workspace': 'My Workspace' },
    });
    expect(e.defaultPrevented).toBe(true);
    expect(mocks.openThreadAcrossWorkspaces).toHaveBeenCalledWith('My Workspace', TID);
  });

  // The host page's hash belongs to the deep-link router. A heading id carries
  // a prefix, so it cannot shadow one of the shell's own ids. So the preview
  // scrolls itself rather than letting the browser follow `#x`.
  describe('in-page fragments', () => {
    const SLUG = 'auth-tier-compatibility-api-key-vs-subscription--oauth';

    function clickFragment(href: string, doc: ReturnType<typeof renderedDocument>, repoLocator?: Parameters<typeof handlePreviewLinkClick>[2]) {
      const e = clickOn(href, {}, {}, doc.root);
      handlePreviewLinkClick(e as unknown as MouseEvent, 'artifacts/notes.md', repoLocator);
      return e;
    }

    it('scrolls to the heading a table-of-contents link names', () => {
      const doc = renderedDocument([{ id: 'user-content-intro' }, { id: `user-content-${SLUG}` }]);
      const e = clickFragment(`#${SLUG}`, doc);
      expect(e.defaultPrevented).toBe(true);
      expect(doc.nodes[1].scrollIntoView).toHaveBeenCalledWith(expect.objectContaining({ block: 'start' }));
      expect(doc.nodes[0].scrollIntoView).not.toHaveBeenCalled();
    });

    it('decodes the fragment before matching it', () => {
      const doc = renderedDocument([{ id: 'user-content-café' }]);
      clickFragment('#caf%C3%A9', doc);
      expect(doc.nodes[0].scrollIntoView).toHaveBeenCalled();
    });

    it('lands on an authored id or a named anchor too', () => {
      const doc = renderedDocument([{ id: 'custom' }, { name: 'legacy' }]);
      clickFragment('#custom', doc);
      clickFragment('#legacy', doc);
      expect(doc.nodes[0].scrollIntoView).toHaveBeenCalled();
      expect(doc.nodes[1].scrollIntoView).toHaveBeenCalled();
    });

    it('prefers the heading over an authored id with the bare slug', () => {
      const doc = renderedDocument([{ id: 'usage' }, { id: 'user-content-usage' }]);
      clickFragment('#usage', doc);
      expect(doc.nodes[1].scrollIntoView).toHaveBeenCalled();
      expect(doc.nodes[0].scrollIntoView).not.toHaveBeenCalled();
    });

    it('scrolls to the top for a bare #', () => {
      const doc = renderedDocument([]);
      expect(clickFragment('#', doc).defaultPrevented).toBe(true);
      expect(doc.root.scrollIntoView).toHaveBeenCalled();
    });

    it('says so when the document has no such section, naming the repo file by its path', () => {
      const doc = renderedDocument([{ id: 'user-content-intro' }]);
      clickFragment('#gone', doc, { repoId: 'repo-1', mode: 'file', path: 'docs/guide.md' });
      expect(mocks.showToast).toHaveBeenCalledWith('No "gone" section in docs/guide.md', 'error');
    });

    it('leaves a fragment outside a rendered document to the browser', () => {
      const e = clickInMarkdown('#section-two');
      expect(e.defaultPrevented).toBe(false);
      expect(mocks.showToast).not.toHaveBeenCalled();
    });
  });

  it('leaves an unclaimed href and a non-anchor click completely alone', () => {
    expect(clickInMarkdown('mailto:someone@example.com').defaultPrevented).toBe(false);
    expect(clickInMarkdown(null).defaultPrevented).toBe(false);
  });

  it('hands a modified or non-primary click, or a download, back to the browser', () => {
    for (const modifiers of [
      { metaKey: true },
      { ctrlKey: true },
      { shiftKey: true },
      { altKey: true },
      { button: 1 },
    ]) {
      expect(clickInMarkdown('notes.md', 'artifacts/x.md', { modifiers }).defaultPrevented).toBe(false);
    }
    expect(clickInMarkdown('report.csv', 'artifacts/x.md', { attrs: { download: '' } }).defaultPrevented)
      .toBe(false);
    expect(mocks.openFilePreview).not.toHaveBeenCalled();
  });

  it('respects a click another handler already claimed', () => {
    const e = clickOn('notes.md');
    e.defaultPrevented = true;
    handlePreviewLinkClick(e as unknown as MouseEvent, 'artifacts/x.md');
    expect(mocks.openFilePreview).not.toHaveBeenCalled();
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

// The sizing stamp makes an HTML artifact grow with the shell around it. It
// also gives unsized text the chat prose step (ADR 0319). It shares
// `injectAtHeadStart` with the base stamp, so the placement cases below are the
// same four, asserted through the other caller.
describe('withPreviewSizing', () => {
  const DOC = '<!DOCTYPE html><html><head><title>T</title></head><body>x</body></html>';
  const BODY = ':where(body){font-size:0.75rem}';
  /** Everything after the zoom: the body default, then the find highlights. */
  const AFTER_ZOOM = BODY + PREVIEW_FIND_RULES;

  it('stamps zoom and the body default as the first thing in <head>', () => {
    expect(withPreviewSizing(DOC, 125)).toContain(`<head><style>:root{zoom:125%}${AFTER_ZOOM}</style><title>`);
  });

  it('keeps the fractional step of the preference grid', () => {
    expect(withPreviewSizing(DOC, 112.5)).toContain('zoom:112.5%');
  });

  // Zero specificity, so any rule the artifact writes on `body` still wins.
  it('gives the body default no specificity to outrank an author rule', () => {
    expect(withPreviewSizing(DOC, 125)).toContain(BODY);
    expect(withPreviewSizing(DOC, 125)).not.toMatch(/(^|[^(])body\s*\{/);
  });

  // A root font-size would move every rem in an artifact that already follows
  // the type scale, shrinking it a second time.
  it('never sets a font-size on the root', () => {
    expect(withPreviewSizing(DOC, 125)).not.toMatch(/(:root|html)\s*\{[^}]*font-size/);
  });

  // The default scale stamps no zoom, so 100% zooms nothing. The body default
  // does not depend on the scale and still applies.
  it('stamps no zoom at 100%', () => {
    expect(withPreviewSizing(DOC, 100)).toBe(DOC.replace('<head>', `<head><style>${AFTER_ZOOM}</style>`));
  });

  // An unreadable preference must never cost the reader the document.
  it('stamps no zoom for a value that is not a usable scale', () => {
    for (const bad of [NaN, Infinity, 0, -50]) {
      expect(withPreviewSizing(DOC, bad)).not.toContain('zoom');
      expect(withPreviewSizing(DOC, bad)).toContain('<body>x</body>');
    }
  });

  it('creates a head when the document has <html> but no <head>', () => {
    expect(withPreviewSizing('<html><body>x</body></html>', 125))
      .toBe(`<html><head><style>:root{zoom:125%}${AFTER_ZOOM}</style></head><body>x</body></html>`);
  });

  it('keeps the doctype first, so the page stays out of quirks mode', () => {
    expect(withPreviewSizing('<!DOCTYPE html>\n<p>x</p>', 125).startsWith('<!DOCTYPE html>')).toBe(true);
  });

  it('prepends to a bare fragment', () => {
    expect(withPreviewSizing('<p>x</p>', 125)).toBe(`<style>:root{zoom:125%}${AFTER_ZOOM}</style><p>x</p>`);
  });

  // The two stamps compose the way the preview composes them, and the base has
  // to come first: it governs relative URLs the rest of the head may carry.
  it('leaves the base first when both are stamped', () => {
    const out = withPreviewBase(withPreviewSizing(DOC, 125), 'https://h/ws/data/artifacts/');
    expect(out).toContain('<head><base href="https://h/ws/data/artifacts/"><style>:root{zoom:125%}');
  });

  // The bridge asks `documentDeclaresBase` about the ORIGINAL artifact, so a
  // sized document with its own base must still route links against it.
  it('does not disturb an artifact that declares its own base', () => {
    const html = '<html><head><base href="https://elsewhere/"></head></html>';
    const sized = withPreviewSizing(html, 125);
    expect(documentDeclaresBase(sized)).toBe(true);
    expect(withPreviewBase(sized, 'https://h/ws/data/artifacts/')).toBe(sized);
  });

  it('styles the find bar\'s highlights with the system\'s own find colours', () => {
    const out = withPreviewSizing(DOC, 100);
    expect(out).toContain('--find-highlights:styled');
    expect(out).toContain(`::highlight(${ALL_HIGHLIGHT})`);
    expect(out).toContain(`::highlight(${CURRENT_HIGHLIGHT})`);
  });
});
