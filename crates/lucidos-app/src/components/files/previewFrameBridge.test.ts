import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import PREVIEW_SOURCE from './FilePreviewInline.tsx?raw';
import FILE_RESPONSE_RS from '../../../../lucidos-engine/src/api/file_response.rs?raw';

const mocks = vi.hoisted(() => ({
  openFilePreview: vi.fn(),
  openUrl: vi.fn(),
  openLocalFileOnConfirm: vi.fn(async (_target: string, _source?: string) => {}),
  openAppById: vi.fn(async () => {}),
  openThreadAcrossWorkspaces: vi.fn(),
  handleNavigationRequest: vi.fn(),
  dispatchForwardedChord: vi.fn(),
  showToast: vi.fn(),
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
vi.mock('../../hooks/useKeyboardShortcuts', () => ({
  dispatchForwardedChord: mocks.dispatchForwardedChord,
}));
vi.mock('../../store/store', async () => {
  const actual = await vi.importActual<typeof import('../../store/store')>('../../store/store');
  return { ...actual, showToast: mocks.showToast };
});
vi.mock('../../utils/basePath', async () => {
  const actual = await vi.importActual<typeof import('../../utils/basePath')>('../../utils/basePath');
  return { ...actual, WORKSPACE_ID: 'myws' };
});

const {
  ARTIFACT_PREVIEW_SANDBOX,
  PREVIEW_BRIDGE_SOURCE,
  newBridgeNonce,
  previewBridgeScript,
  previewMayNavigate,
  readPreviewFrameMessage,
  routePreviewFrameMessage,
  withPreviewBridge,
  withPreviewCapability,
} = await import('./previewFrameBridge');
const { repositories } = await import('../../store/store');
const { PREVIEW_HOST_SCHEMES, classifyPreviewLink, withPreviewBase } = await import('./previewIframeLinks');
const { PREVIEW_FRAME_MESSAGE, PREVIEW_HOST_MESSAGE } = await import('../../utils/previewFrameProtocol');

const TID = '961b9b83-53b7-47cd-8982-3c959d7f1137';
const NONCE = 'n0nce';
const ARTIFACT = 'artifacts/reports/pr-1573.html';
const MAXIMIZE = { mod: true, shift: true, alt: false, key: 'Enter' };
const ZOOM_IN = { mod: true, shift: false, alt: false, key: '=' };
const SEARCH = { mod: true, shift: false, alt: false, key: 'k' };

// ---------------------------------------------------------------------------
// The sandbox
// ---------------------------------------------------------------------------

describe('the preview frame sandbox', () => {
  const tokens = ARTIFACT_PREVIEW_SANDBOX.split(/\s+/);

  it('lets scripts run but never gives the artifact the shell origin', () => {
    expect(tokens).toContain('allow-scripts');
    // The fix itself. With this token, `about:srcdoc` runs as the shell.
    expect(tokens).not.toContain('allow-same-origin');
    expect(tokens).not.toContain('allow-top-navigation');
    expect(tokens).not.toContain('allow-top-navigation-by-user-activation');
    // A window the artifact opens must stay sandboxed too.
    expect(tokens).not.toContain('allow-popups-to-escape-sandbox');
  });

  it('is the exact set the engine serves a data file under', () => {
    // The preview and a served file are the same document on two routes, so an
    // artifact must get one set of powers on both.
    const rust: string = FILE_RESPONSE_RS;
    const csp = /DOCUMENT_SANDBOX_CSP: &str =\s*"sandbox ([^"]+)"/.exec(rust)?.[1];
    expect(csp, 'DOCUMENT_SANDBOX_CSP not found in file_response.rs').toBeDefined();
    expect(new Set(csp!.split(/\s+/))).toEqual(new Set(tokens));
  });

  it('is what the HTML body frame actually carries', () => {
    // Source scan: the constant is only safe if the frame uses it, and uses
    // nothing that re-grants the origin.
    const src: string = PREVIEW_SOURCE;
    const frame = /function HtmlPreviewFrame[\s\S]*?<iframe([\s\S]*?)\/>/.exec(src)?.[1] ?? '';
    expect(frame).toContain('sandbox={ARTIFACT_PREVIEW_SANDBOX}');
    expect(frame).toContain('srcDoc={srcDoc}');
    expect(src).not.toMatch(/allow-same-origin/);
    expect(src).not.toMatch(/contentDocument/);
  });
});

// ---------------------------------------------------------------------------
// The in-frame script, run against a fake window
// ---------------------------------------------------------------------------

type Listener = (e: unknown) => void;

interface FakeFrame {
  posted: Record<string, unknown>[];
  scrolled: string[];
  scrolledToTop: boolean;
  base: { href: string | null };
  fire: (target: 'doc' | 'win', type: string, e: unknown) => void;
  listenerCount: (target: 'doc' | 'win', type: string) => number;
  host: object;
  scriptRemoved?: boolean;
}

/** Install the bridge in a fake frame. `ids` are the anchors the document holds,
 *  `baseHref` the `<base>` it carries (`null` for none). */
function installBridge(opts: { ids?: string[]; baseHref?: string | null; topLevel?: boolean; mac?: boolean } = {}): FakeFrame {
  const posted: Record<string, unknown>[] = [];
  const scrolled: string[] = [];
  const listeners: Record<string, Record<string, Listener[]>> = { doc: {}, win: {} };
  const host = { postMessage: (msg: Record<string, unknown>) => { posted.push(msg); } };
  const base = { href: opts.baseHref === undefined ? '/myws/~cap/old~artifact..preview~sig/data/artifacts/' : opts.baseHref };
  const frame: FakeFrame = {
    posted,
    scrolled,
    scrolledToTop: false,
    base,
    fire: (target, type, e) => { for (const fn of listeners[target][type] ?? []) fn(e); },
    listenerCount: (target, type) => (listeners[target][type] ?? []).length,
    host,
  };
  const on = (target: 'doc' | 'win') => (type: string, fn: Listener) => {
    (listeners[target][type] ??= []).push(fn);
  };
  const win: Record<string, unknown> = {
    addEventListener: on('win'),
    scrollTo: () => { frame.scrolledToTop = true; },
  };
  win.parent = opts.topLevel ? win : host;
  const removed: unknown[] = [];
  const script = { parentNode: { removeChild: (node: unknown) => { removed.push(node); } } };
  const doc = {
    currentScript: script,
    addEventListener: on('doc'),
    baseURI: 'https://localhost:5251/myws/data/artifacts/reports/',
    querySelector: (selector: string) => {
      if (selector === 'base') {
        return base.href === null ? null : {
          getAttribute: () => base.href,
          setAttribute: (_: string, v: string) => { base.href = v; },
        };
      }
      const m = /^\[(?:id|name)="(.*)"\]$/.exec(selector);
      const id = m?.[1].replace(/\\(["\\])/g, '$1');
      return id !== undefined && (opts.ids ?? []).includes(id)
        ? { scrollIntoView: () => { scrolled.push(id); } }
        : null;
    },
  };
  const cfg = {
    nonce: NONCE,
    bindings: [MAXIMIZE, ZOOM_IN, SEARCH],
    mac: opts.mac ?? false,
    schemes: PREVIEW_HOST_SCHEMES,
    frameType: PREVIEW_FRAME_MESSAGE,
    hostType: PREVIEW_HOST_MESSAGE,
  };
  const install = new Function(`return (${PREVIEW_BRIDGE_SOURCE});`)() as
    (c: typeof cfg, w: unknown, d: unknown) => void;
  install(cfg, win, doc);
  frame.scriptRemoved = removed.includes(script);
  return frame;
}

function click(href: string | null, over: Record<string, unknown> = {}, attrs: Record<string, string> = {}) {
  const anchor = href === null ? null : {
    getAttribute: (name: string) => (name === 'href' ? href : attrs[name] ?? null),
    hasAttribute: (name: string) => name in attrs,
  };
  const e = {
    isTrusted: true,
    defaultPrevented: false,
    stopped: false,
    metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, button: 0,
    ...over,
    target: { closest: () => anchor },
    preventDefault() { e.defaultPrevented = true; },
    stopPropagation() { e.stopped = true; },
  };
  return e;
}

function key(k: string, over: Record<string, unknown> = {}) {
  const e = {
    isTrusted: true,
    key: k, metaKey: false, ctrlKey: false, shiftKey: false, altKey: false,
    target: null as unknown,
    ...over,
    defaultPrevented: false,
    preventDefault() { e.defaultPrevented = true; },
  };
  return e;
}

describe('the in-frame bridge script', () => {
  it('removes its own script element, so the nonce is left in no markup', () => {
    expect(installBridge().scriptRemoved).toBe(true);
  });

  it('posts nothing for a click or a key the artifact dispatched itself', () => {
    const frame = installBridge();
    const scripted = click('pr-1573.md', { isTrusted: false });
    const scriptedDownload = click('a.csv', { isTrusted: false }, { download: '' });
    const scriptedChord = key('Enter', { metaKey: true, shiftKey: true, isTrusted: false });
    frame.fire('doc', 'click', scripted);
    frame.fire('doc', 'click', scriptedDownload);
    frame.fire('doc', 'keydown', scriptedChord);
    expect(frame.posted).toEqual([]);
    expect(scripted.defaultPrevented).toBe(false);
  });

  it('installs nothing when it is not framed', () => {
    const frame = installBridge({ topLevel: true });
    expect(frame.listenerCount('doc', 'click')).toBe(0);
    expect(frame.listenerCount('win', 'message')).toBe(0);
  });

  it('cancels a click on a host-owned link and posts it up, stamped', () => {
    const frame = installBridge();
    for (const href of ['#section-two', 'pr-1573.md', `thread:dev/${TID}`, 'https://example.com', 'repo:r:file:a.rs']) {
      const e = click(href);
      frame.fire('doc', 'click', e);
      expect(e.defaultPrevented, href).toBe(true);
      expect(e.stopped, href).toBe(true);
    }
    expect(frame.posted[0]).toEqual({
      kind: 'link',
      href: '#section-two',
      baseUri: 'https://localhost:5251/myws/data/artifacts/reports/',
      type: PREVIEW_FRAME_MESSAGE,
      nonce: NONCE,
    });
    expect(frame.posted).toHaveLength(5);
  });

  it('leaves a scheme the host does not route to the browser', () => {
    const frame = installBridge();
    for (const href of ['mailto:someone@example.com', 'tel:+4712345678', 'javascript:void(0)', '']) {
      const e = click(href);
      frame.fire('doc', 'click', e);
      expect(e.defaultPrevented, href).toBe(false);
    }
    expect(frame.posted).toEqual([]);
  });

  it('hands a modified click, a non-primary click or a claimed click back', () => {
    const frame = installBridge();
    for (const over of [{ metaKey: true }, { ctrlKey: true }, { shiftKey: true }, { altKey: true }, { button: 1 },
      { defaultPrevented: true }]) {
      frame.fire('doc', 'click', click('notes.md', over));
    }
    frame.fire('doc', 'click', click(null));
    expect(frame.posted).toEqual([]);
  });

  it('claims a download of a sibling file, which only the host origin can save', () => {
    const frame = installBridge();
    const download = click('data/results.csv', {}, { download: 'results.csv' });
    frame.fire('doc', 'click', download);
    expect(download.defaultPrevented).toBe(true);
    expect(frame.posted).toEqual([
      {
        kind: 'download', href: 'data/results.csv', name: 'results.csv',
        baseUri: 'https://localhost:5251/myws/data/artifacts/reports/', type: PREVIEW_FRAME_MESSAGE, nonce: NONCE,
      },
    ]);
  });

  it('leaves a download the frame can save itself to the browser', () => {
    // A blob or data URL the artifact built is its own origin's; an external
    // one is the browser's business, as it always was.
    const frame = installBridge();
    for (const href of ['blob:null/1234', 'data:text/csv,a,b', 'https://example.com/x.csv']) {
      const e = click(href, {}, { download: '' });
      frame.fire('doc', 'click', e);
      expect(e.defaultPrevented, href).toBe(false);
    }
    expect(frame.posted).toEqual([]);
  });

  it('cancels a chord bound to a shell shortcut and posts it up', () => {
    const frame = installBridge();
    const maximize = key('Enter', { metaKey: true, shiftKey: true });
    const zoom = key('+', { ctrlKey: true }); // `+` folds to `=`, as the registry does
    frame.fire('doc', 'keydown', maximize);
    frame.fire('doc', 'keydown', zoom);
    expect(maximize.defaultPrevented).toBe(true);
    expect(zoom.defaultPrevented).toBe(true);
    expect(frame.posted.map((m) => [m.kind, m.key])).toEqual([['chord', 'Enter'], ['chord', '+']]);
  });

  it('leaves Ctrl+letter to a text field in the artifact on a Mac, where it edits text', () => {
    const frame = installBridge({ mac: true });
    const inField = key('k', { ctrlKey: true, target: { tagName: 'TEXTAREA' } });
    const withCmd = key('k', { metaKey: true, target: { tagName: 'TEXTAREA' } });
    frame.fire('doc', 'keydown', inField);
    frame.fire('doc', 'keydown', withCmd);
    expect(inField.defaultPrevented).toBe(false);
    expect(withCmd.defaultPrevented).toBe(true);
    expect(frame.posted.map((m) => m.metaKey)).toEqual([true]);
  });

  it('forwards Escape without cancelling it, and keeps typing and copy private', () => {
    const frame = installBridge();
    const escape = key('Escape');
    frame.fire('doc', 'keydown', escape);
    frame.fire('doc', 'keydown', key('a'));
    frame.fire('doc', 'keydown', key('c', { metaKey: true }));
    expect(escape.defaultPrevented).toBe(false);
    expect(frame.posted.map((m) => m.key)).toEqual(['Escape']);
  });

  it('scrolls to an anchor the host sends back, and to the top for an empty one', () => {
    const frame = installBridge({ ids: ['section "two"'] });
    frame.fire('win', 'message', { source: frame.host, data: { type: PREVIEW_HOST_MESSAGE, kind: 'scroll', id: 'section "two"' } });
    frame.fire('win', 'message', { source: frame.host, data: { type: PREVIEW_HOST_MESSAGE, kind: 'scroll', id: '' } });
    expect(frame.scrolled).toEqual(['section "two"']);
    expect(frame.scrolledToTop).toBe(true);
  });

  it('reports an anchor the document does not hold', () => {
    const frame = installBridge();
    frame.fire('win', 'message', { source: frame.host, data: { type: PREVIEW_HOST_MESSAGE, kind: 'scroll', id: 'nope' } });
    expect(frame.posted).toEqual([{ kind: 'fragment-missing', id: 'nope', type: PREVIEW_FRAME_MESSAGE, nonce: NONCE }]);
  });

  it('swaps a renewed pass into its base, and takes orders from the host only', () => {
    const frame = installBridge();
    frame.fire('win', 'message', { source: {}, data: { type: PREVIEW_HOST_MESSAGE, kind: 'capability', capability: 'evil' } });
    expect(frame.base.href).toBe('/myws/~cap/old~artifact..preview~sig/data/artifacts/');
    frame.fire('win', 'message', { source: frame.host, data: { type: PREVIEW_HOST_MESSAGE, kind: 'capability', capability: 'bad/pass' } });
    expect(frame.base.href).toBe('/myws/~cap/old~artifact..preview~sig/data/artifacts/');
    frame.fire('win', 'message', { source: frame.host, data: { type: PREVIEW_HOST_MESSAGE, kind: 'capability', capability: 'new~artifact..preview~sig2' } });
    expect(frame.base.href).toBe('/myws/~cap/new~artifact..preview~sig2/data/artifacts/');
  });

  it('never touches a base that carries no pass, such as one the artifact declared', () => {
    const frame = installBridge({ baseHref: 'https://example.com/docs/' });
    frame.fire('win', 'message', { source: frame.host, data: { type: PREVIEW_HOST_MESSAGE, kind: 'capability', capability: 'new~x~y' } });
    expect(frame.base.href).toBe('https://example.com/docs/');
  });
});

describe('the claim rule and the host router agree', () => {
  const ctx = {
    artifactPath: ARTIFACT, hostOrigin: 'https://localhost:5251', hostPath: '/myws/', workspaceId: 'myws',
  };
  const example: Record<string, string> = {
    http: 'http://example.com/',
    https: 'https://example.com/',
    thread: `thread:${TID}`,
    app: 'app:habit-tracker',
    trigger: 'trigger:t1',
    repo: 'repo:repo-1:file:src/main.rs',
    file: 'file:///Users/me/report.pdf',
  };

  it('routes every scheme the frame claims', () => {
    expect(Object.keys(example).sort()).toEqual([...PREVIEW_HOST_SCHEMES].sort());
    for (const scheme of PREVIEW_HOST_SCHEMES) {
      expect(classifyPreviewLink(example[scheme], ctx), scheme).not.toBeNull();
    }
  });

  it('hands back every scheme the frame leaves alone', () => {
    for (const href of ['mailto:a@example.com', 'tel:+47', 'sms:+47', 'javascript:void(0)', 'data:text/html,x']) {
      expect(classifyPreviewLink(href, ctx), href).toBeNull();
    }
  });
});

// ---------------------------------------------------------------------------
// The host side
// ---------------------------------------------------------------------------

describe('readPreviewFrameMessage', () => {
  const frameWindow = {} as Window;
  const link = { type: PREVIEW_FRAME_MESSAGE, nonce: NONCE, kind: 'link', href: 'notes.md', baseUri: 'https://x/' };
  const from = (data: unknown, over: Partial<{ source: unknown; origin: string }> = {}) =>
    ({ source: frameWindow, origin: 'null', data, ...over }) as unknown as MessageEvent;

  it('accepts each kind from its own frame', () => {
    expect(readPreviewFrameMessage(from(link), frameWindow, NONCE))
      .toEqual({ kind: 'link', href: 'notes.md', baseUri: 'https://x/' });
    expect(readPreviewFrameMessage(from({
      type: PREVIEW_FRAME_MESSAGE, nonce: NONCE, kind: 'chord',
      key: 'Enter', metaKey: true, ctrlKey: false, shiftKey: true, altKey: false,
    }), frameWindow, NONCE)).toEqual({
      kind: 'chord', chord: { key: 'Enter', metaKey: true, ctrlKey: false, shiftKey: true, altKey: false },
    });
    expect(readPreviewFrameMessage(from({ type: PREVIEW_FRAME_MESSAGE, nonce: NONCE, kind: 'fragment-missing', id: 'x' }),
      frameWindow, NONCE)).toEqual({ kind: 'fragment-missing', id: 'x' });
  });

  it('drops a message from any other window, an app frame included', () => {
    expect(readPreviewFrameMessage(from(link, { source: {} }), frameWindow, NONCE)).toBeNull();
    expect(readPreviewFrameMessage(from(link, { source: null }), frameWindow, NONCE)).toBeNull();
    expect(readPreviewFrameMessage(from(link), null, NONCE)).toBeNull();
  });

  it('drops a message whose origin is not opaque, since the sandbox must be gone', () => {
    expect(readPreviewFrameMessage(from(link, { origin: 'https://localhost:5251' }), frameWindow, NONCE)).toBeNull();
  });

  it('drops a stale or forged nonce, and a foreign type', () => {
    expect(readPreviewFrameMessage(from({ ...link, nonce: 'old' }), frameWindow, NONCE)).toBeNull();
    expect(readPreviewFrameMessage(from({ ...link, nonce: undefined }), frameWindow, NONCE)).toBeNull();
    expect(readPreviewFrameMessage(from({ ...link, type: 'lucidos:keydown' }), frameWindow, NONCE)).toBeNull();
  });

  it('drops a malformed body', () => {
    for (const data of [
      null, 'link', 42,
      { ...link, href: 7 },
      { ...link, href: 'x'.repeat(9000) },
      { ...link, kind: 'navigate' },
      { type: PREVIEW_FRAME_MESSAGE, nonce: NONCE, kind: 'chord', key: 'k', metaKey: 'yes', ctrlKey: false, shiftKey: false, altKey: false },
      { type: PREVIEW_FRAME_MESSAGE, nonce: NONCE, kind: 'chord', key: 'k'.repeat(40), metaKey: false, ctrlKey: false, shiftKey: false, altKey: false },
      { type: PREVIEW_FRAME_MESSAGE, nonce: NONCE, kind: 'fragment-missing' },
    ]) {
      expect(readPreviewFrameMessage(from(data), frameWindow, NONCE)).toBeNull();
    }
  });

  it('accepts a download with its suggested name, and drops one without', () => {
    const download = { type: PREVIEW_FRAME_MESSAGE, nonce: NONCE, kind: 'download', href: 'a.csv', name: '' };
    expect(readPreviewFrameMessage(from(download), frameWindow, NONCE))
      .toEqual({ kind: 'download', href: 'a.csv', name: '', baseUri: null });
    expect(readPreviewFrameMessage(from({ ...download, name: undefined }), frameWindow, NONCE)).toBeNull();
  });

  it('keeps a link whose reported base is not a string, without the base', () => {
    expect(readPreviewFrameMessage(from({ ...link, baseUri: 5 }), frameWindow, NONCE))
      .toEqual({ kind: 'link', href: 'notes.md', baseUri: null });
  });
});

describe('previewMayNavigate', () => {
  it('lets a click navigate, and refuses a message with no click behind it', () => {
    expect(previewMayNavigate(true)).toBe(true);
    expect(previewMayNavigate(false)).toBe(false);
    // A browser that cannot tell keeps the link working.
    expect(previewMayNavigate(null)).toBe(true);
  });
});

describe('routePreviewFrameMessage', () => {
  const posted: unknown[] = [];
  const frameWindow = { postMessage: (m: unknown) => { posted.push(m); } } as unknown as Window;
  const route = (href: string, declaresOwnBase = false, baseUri: string | null = null) =>
    routePreviewFrameMessage({ kind: 'link', href, baseUri }, { artifactPath: ARTIFACT, declaresOwnBase, frameWindow });

  beforeEach(() => {
    vi.clearAllMocks();
    posted.length = 0;
    vi.stubGlobal('location', { origin: 'https://localhost:5251', pathname: '/myws/' });
  });
  afterEach(() => { vi.unstubAllGlobals(); });

  it('sends a fragment back to the frame to scroll, and never navigates', () => {
    route('#section-two');
    expect(posted).toEqual([{ type: PREVIEW_HOST_MESSAGE, kind: 'scroll', id: 'section-two', smooth: expect.any(Boolean) }]);
    expect(mocks.openFilePreview).not.toHaveBeenCalled();
    expect(mocks.openUrl).not.toHaveBeenCalled();
  });

  it('routes a sibling file through openFilePreview, which gives it a nav-history entry', () => {
    route('pr-1573.md');
    expect(mocks.openFilePreview).toHaveBeenCalledWith('artifacts/reports/pr-1573.md');
  });

  it('routes a thread, an external page and a repo citation', () => {
    repositories.value = { status: 'loaded', data: [{ id: 'repo-1', name: 'example-repo', path: '/src/example' }] };
    route(`thread:dev/${TID}`);
    route('https://example.com/docs');
    route('repo:repo-1:file:src/main.rs#L510-L520');
    expect(mocks.openThreadAcrossWorkspaces).toHaveBeenCalledWith('dev', TID);
    expect(mocks.openUrl).toHaveBeenCalledWith('https://example.com/docs');
    expect(mocks.handleNavigationRequest).toHaveBeenCalledWith(
      { target: 'file', file_path: 'repo:repo-1:file:src/main.rs', line: 510, line_end: 520 },
      { source: 'a file preview' },
    );
  });

  it('honours the frame\'s base only when the artifact declared its own', () => {
    route('guide.html', true, 'https://example.com/docs/');
    expect(mocks.openUrl).toHaveBeenCalledWith('https://example.com/docs/guide.html');
    vi.clearAllMocks();
    // A forged base on an artifact that declared none changes nothing.
    route('guide.html', false, 'https://example.com/docs/');
    expect(mocks.openUrl).not.toHaveBeenCalled();
    expect(mocks.openFilePreview).toHaveBeenCalledWith('artifacts/reports/guide.html');
  });

  it('says so when a claimed link routes nowhere', () => {
    route('http://[broken');
    expect(mocks.showToast).toHaveBeenCalledTimes(1);
    expect(mocks.showToast.mock.calls[0][0]).toContain(ARTIFACT);
  });

  it('refuses a link posted with no click behind it, and says so', () => {
    vi.stubGlobal('navigator', { userActivation: { isActive: false } });
    route('pr-1573.md');
    route('#section-two');
    expect(mocks.openFilePreview).not.toHaveBeenCalled();
    expect(mocks.showToast.mock.calls[0][0]).toContain('without a click');
    // A scroll inside the document itself needs no click.
    expect(posted).toHaveLength(1);
  });

  it('opens a local file only through the confirm, naming the artifact', () => {
    vi.stubGlobal('navigator', { userActivation: { isActive: true } });
    route('file:///Users/me/report.pdf');
    expect(mocks.openLocalFileOnConfirm).toHaveBeenCalledWith('file:///Users/me/report.pdf', ARTIFACT);
  });

  // An artifact that declares its own base routes a download like a link, so
  // it can name a local file too. That arm used to skip the confirm.
  it('sends a download that resolves to a local file through the confirm', () => {
    vi.stubGlobal('navigator', { userActivation: { isActive: true } });
    routePreviewFrameMessage(
      { kind: 'download', href: 'file:///Users/me/report.pdf', name: '', baseUri: 'https://example.com/docs/' },
      { artifactPath: ARTIFACT, declaresOwnBase: true, frameWindow },
    );
    expect(mocks.openLocalFileOnConfirm).toHaveBeenCalledWith('file:///Users/me/report.pdf', ARTIFACT);
  });

  it('refuses a download posted with no click behind it', () => {
    vi.stubGlobal('navigator', { userActivation: { isActive: false } });
    routePreviewFrameMessage({ kind: 'download', href: 'a.csv', name: '', baseUri: null }, { artifactPath: ARTIFACT, declaresOwnBase: false, frameWindow });
    expect(mocks.showToast.mock.calls[0][0]).toContain('without a click');
  });

  it('downloads a percent-encoded sibling once-encoded, from the shell origin', () => {
    vi.stubGlobal('navigator', { userActivation: { isActive: true } });
    const anchor = { href: '', download: '', rel: '', click: vi.fn(), remove: vi.fn() };
    vi.stubGlobal('document', { createElement: () => anchor, querySelector: () => null, body: { appendChild: () => {} } });
    routePreviewFrameMessage(
      { kind: 'download', href: 'Q3%20summary.csv', name: '', baseUri: null },
      { artifactPath: ARTIFACT, declaresOwnBase: false, frameWindow },
    );
    expect(anchor.href).toMatch(/\/data\/artifacts\/reports\/Q3%20summary\.csv$/);
    expect(anchor.download).toBe('Q3 summary.csv');
    expect(anchor.click).toHaveBeenCalledOnce();
  });

  it('keeps an encoded dot segment encoded, so a download cannot leave the data mount', () => {
    vi.stubGlobal('navigator', { userActivation: { isActive: true } });
    const anchor = { href: '', download: '', rel: '', click: vi.fn(), remove: vi.fn() };
    vi.stubGlobal('document', { createElement: () => anchor, querySelector: () => null, body: { appendChild: () => {} } });
    routePreviewFrameMessage(
      { kind: 'download', href: '%2e%2e/%2E%2E/%2e%2e%2fapi/v1/x', name: '', baseUri: null },
      { artifactPath: ARTIFACT, declaresOwnBase: false, frameWindow },
    );
    expect(anchor.href).not.toMatch(/\/\.\.(\/|$)/);
    expect(anchor.href).toMatch(/\/data\/artifacts\/reports\/%252e%252e\/%252E%252E\/%252e%252e%252fapi\/v1\/x$/);
  });

  it('sends a download under a declared off-site base where that base points', () => {
    vi.stubGlobal('navigator', { userActivation: { isActive: true } });
    routePreviewFrameMessage(
      { kind: 'download', href: 'data.zip', name: '', baseUri: 'https://example.com/files/' },
      { artifactPath: ARTIFACT, declaresOwnBase: true, frameWindow },
    );
    expect(mocks.openUrl).toHaveBeenCalledWith('https://example.com/files/data.zip');
  });

  it('runs a forwarded chord through the shell shortcut dispatcher', () => {
    const chord = { key: '=', metaKey: true, ctrlKey: false, shiftKey: false, altKey: false };
    routePreviewFrameMessage({ kind: 'chord', chord }, { artifactPath: ARTIFACT, declaresOwnBase: false, frameWindow });
    expect(mocks.dispatchForwardedChord).toHaveBeenCalledWith(chord);
  });

  it('reports a missing anchor, naming the document', () => {
    routePreviewFrameMessage({ kind: 'fragment-missing', id: 'nope' }, { artifactPath: ARTIFACT, declaresOwnBase: false, frameWindow });
    expect(mocks.showToast.mock.calls[0][0]).toContain('nope');
    expect(mocks.showToast.mock.calls[0][0]).toContain(ARTIFACT);
  });
});

// ---------------------------------------------------------------------------
// Stamping
// ---------------------------------------------------------------------------

describe('withPreviewCapability', () => {
  it('splices the pass in ahead of the data segment', () => {
    expect(withPreviewCapability('/myws/data/artifacts/r/a.html?rev=2', 'tok'))
      .toBe('/myws/~cap/tok/data/artifacts/r/a.html?rev=2');
    expect(withPreviewCapability('/data/artifacts/a.html', 'tok')).toBe('/~cap/tok/data/artifacts/a.html');
  });

  it('leaves the URL alone with no pass, or off the artifacts tree of the mount', () => {
    expect(withPreviewCapability('/myws/data/artifacts/a.html', null)).toBe('/myws/data/artifacts/a.html');
    for (const url of [
      '/myws/api/v1/data/system-knowhow/x.html',
      '/myws/api/v1/data/artifacts/x.html',
      '/myws/data/knowhow/x.html',
    ]) {
      expect(withPreviewCapability(url, 'tok')).toBe(url);
    }
  });
});

describe('the stamped bridge script', () => {
  const cfg = { nonce: NONCE, bindings: [MAXIMIZE], hostSchemes: PREVIEW_HOST_SCHEMES };

  it('runs first: ahead of the base, and ahead of the artifact\'s own script', () => {
    const html = '<!DOCTYPE html><html><head><script>window.mine=1</script></head><body></body></html>';
    const out = withPreviewBridge(withPreviewBase(html, 'https://h/myws/data/artifacts/'), cfg);
    const bridgeAt = out.indexOf(PREVIEW_FRAME_MESSAGE);
    expect(bridgeAt).toBeGreaterThan(-1);
    expect(bridgeAt).toBeLessThan(out.indexOf('<base'));
    expect(out.indexOf('<base')).toBeLessThan(out.indexOf('window.mine'));
  });

  it('runs before anything the artifact puts ahead of its head', () => {
    // A script before `<head>`, or a `<head>` inside a comment, would otherwise
    // run first and watch the bridge arrive.
    for (const html of [
      '<!DOCTYPE html><script>window.early=1</script><html><head></head></html>',
      '<!-- <head> --><script>window.early=1</script><html><head></head></html>',
    ]) {
      const out = withPreviewBridge(html, cfg);
      expect(out.indexOf(PREVIEW_FRAME_MESSAGE), html).toBeLessThan(out.indexOf('window.early'));
    }
    // And never ahead of the doctype, which would drop the page into quirks mode.
    expect(withPreviewBridge('<!DOCTYPE html><p>x</p>', cfg).startsWith('<!DOCTYPE html><script>')).toBe(true);
    expect(withPreviewBridge('<!-- generated -->\n<!DOCTYPE html><p>x</p>', cfg))
      .toMatch(/^<!-- generated -->\n<!DOCTYPE html><script>/);
  });

  it('cannot be closed early by anything in its config', () => {
    const out = previewBridgeScript({ ...cfg, nonce: '</script><script>alert(1)</script>' });
    expect(out.match(/<\/script>/g)).toHaveLength(1);
    expect(out.endsWith('</script>')).toBe(true);
  });

  it('mints a fresh 128-bit nonce each render', () => {
    const a = newBridgeNonce();
    expect(a).toMatch(/^[0-9a-f]{32}$/);
    expect(newBridgeNonce()).not.toBe(a);
  });
});
