import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

// isIOSPwa is cached at module load from the real user-agent, so the branch has
// to be driven through a mock rather than by faking navigator.
const platformMocks = vi.hoisted(() => ({ isIOSPwa: false }));
vi.mock('./platform', () => ({
  isIOSPwa: () => platformMocks.isIOSPwa,
}));

// Stubbed rather than driven through the real `preferences` signal so the mode
// matrix below can't be perturbed by store state, and so the "still loading"
// default is exercised by its own case in the preferences suite.
const prefMocks = vi.hoisted(() => ({ target: 'safari' as 'safari' | 'ask' | 'in-app' }));
vi.mock('../store/actions/preferences', () => ({
  currentExternalLinkTarget: () => prefMocks.target,
}));

// The blocked-popup recovery surfaces through the store's toast + the engine-log
// breadcrumb channel. Both are stubbed so the assertions read the calls directly
// and neither the real signal store nor a real fetch is pulled into this suite.
const toastMocks = vi.hoisted(() => ({ showToast: vi.fn(), dismissToast: vi.fn() }));
vi.mock('../store/store', () => toastMocks);
const postClientLog = vi.hoisted(() => vi.fn());
vi.mock('./clientLog', () => ({ postClientLog }));

const { openExternalUrl } = await import('./openExternalUrl');

// The node test env has neither window.open nor window.location (test-setup.ts
// aliases window to globalThis), so both are installed as globals.
const windowOpen = vi.hoisted(() => vi.fn());
const APP_URL = 'https://app.example.com/ws/dev/';
let fakeLocation: { href: string };

/** A window handle of the shape a SUCCESSFUL `window.open` hands back. Carries a
 *  writable `opener` because severing it is what replaces the `noopener`
 *  feature, which cannot be passed here: with it set, the spec makes
 *  `window.open` return null on success as well as on a block, so the return
 *  value would carry no signal. */
function fakeWindow(): { closed: boolean; opener: unknown } {
  return { closed: false, opener: {} };
}

/** The single toast the blocked path raises, or undefined. */
function blockedToast(): { message: string; opts: { key: string; action: { label: string; onClick: () => void } } } | undefined {
  const calls = toastMocks.showToast.mock.calls;
  const call = calls.length ? calls[calls.length - 1] : undefined;
  return call ? { message: call[0], opts: call[2] } : undefined;
}

/** Install a `navigator.share` for this test. The node env has no `navigator.share`,
 *  and `ask` mode's own no-share branch is asserted separately. */
function stubShare(impl: (data: { url?: string }) => Promise<void>): ReturnType<typeof vi.fn> {
  const share = vi.fn(impl);
  vi.stubGlobal('navigator', { ...navigator, share });
  return share;
}

describe('openExternalUrl', () => {
  beforeEach(() => {
    platformMocks.isIOSPwa = false;
    prefMocks.target = 'safari';
    windowOpen.mockReset();
    windowOpen.mockReturnValue(fakeWindow());
    toastMocks.showToast.mockClear();
    toastMocks.dismissToast.mockClear();
    postClientLog.mockClear();
    fakeLocation = { href: APP_URL };
    vi.stubGlobal('open', windowOpen);
    vi.stubGlobal('location', fakeLocation);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('iOS PWA + https: hands off to Safari via the x-safari- scheme, never window.open', () => {
    platformMocks.isIOSPwa = true;

    openExternalUrl('https://example.com/docs');

    expect(window.location.href).toBe('x-safari-https://example.com/docs');
    expect(windowOpen).not.toHaveBeenCalled();
  });

  it('iOS PWA + http: prefixed too (the in-app view traps plain http the same way)', () => {
    platformMocks.isIOSPwa = true;

    openExternalUrl('http://example.com/');

    expect(window.location.href).toBe('x-safari-http://example.com/');
    expect(windowOpen).not.toHaveBeenCalled();
  });

  it('iOS PWA + mailto: passes through unrewritten so the Mail app still gets it', () => {
    platformMocks.isIOSPwa = true;

    openExternalUrl('mailto:someone@example.com');

    expect(windowOpen).toHaveBeenCalledWith('mailto:someone@example.com', '_blank');
    expect(window.location.href).toBe(APP_URL);
  });

  it('iOS PWA + tel:/file:/data: pass through unrewritten as well', () => {
    platformMocks.isIOSPwa = true;

    const urls = ['tel:+15550000', 'file:///tmp/report.pdf', 'data:text/plain,hi'];
    for (const url of urls) {
      openExternalUrl(url);
      expect(windowOpen).toHaveBeenLastCalledWith(url, '_blank');
    }
    // Pins that each iteration opened, rather than a later one carrying an
    // earlier call past a silently skipped scheme.
    expect(windowOpen).toHaveBeenCalledTimes(urls.length);
    expect(window.location.href).toBe(APP_URL);
  });

  it('non-PWA browser: opens a new tab and never navigates the document', () => {
    platformMocks.isIOSPwa = false;

    openExternalUrl('https://example.com/docs');

    expect(windowOpen).toHaveBeenCalledWith('https://example.com/docs', '_blank');
    expect(window.location.href).toBe(APP_URL);
  });

  it('never passes the `noopener` feature, which would make the return value meaningless', () => {
    openExternalUrl('https://example.com/docs');

    // The spec returns null from window.open whenever `noopener` is set, on a
    // successful open exactly as on a blocked one, so the feature and the block
    // detection below cannot coexist. Verified against Chromium and WebKit.
    expect(windowOpen).toHaveBeenCalledWith('https://example.com/docs', '_blank');
    expect(windowOpen.mock.calls[0]).toHaveLength(2);
  });

  it('severs `opener` on the new tab, which is what replaces the feature', () => {
    const opened = fakeWindow();
    windowOpen.mockReturnValue(opened);

    openExternalUrl('https://example.com/docs');

    expect(opened.opener).toBeNull();
  });
});

describe('openExternalUrl: a popup the browser blocked', () => {
  const TARGET = 'https://example.com/report';

  beforeEach(() => {
    platformMocks.isIOSPwa = false;
    prefMocks.target = 'safari';
    windowOpen.mockReset();
    toastMocks.showToast.mockClear();
    toastMocks.dismissToast.mockClear();
    postClientLog.mockClear();
    fakeLocation = { href: APP_URL };
    vi.stubGlobal('open', windowOpen);
    vi.stubGlobal('location', fakeLocation);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('a null return raises a recovery toast naming the url, and does not throw', () => {
    windowOpen.mockReturnValue(null);

    expect(() => openExternalUrl(TARGET)).not.toThrow();

    const toast = blockedToast();
    expect(toast?.message).toContain(TARGET);
    expect(toast?.opts.action.label).toBe('Open');
  });

  it('a window that is already closed counts as blocked too', () => {
    windowOpen.mockReturnValue({ closed: true, opener: {} });

    openExternalUrl(TARGET);

    expect(blockedToast()?.opts.action.label).toBe('Open');
  });

  it('a real window raises no toast and writes no breadcrumb', () => {
    windowOpen.mockReturnValue(fakeWindow());

    openExternalUrl(TARGET);

    expect(toastMocks.showToast).not.toHaveBeenCalled();
    expect(postClientLog).not.toHaveBeenCalled();
  });

  it('writes a durable [Client/nav] breadcrumb carrying the blocked url', () => {
    windowOpen.mockReturnValue(null);

    openExternalUrl(TARGET);

    expect(postClientLog).toHaveBeenCalledWith('nav', 'external-url-blocked', { url: TARGET, source: null });
  });

  it('names who asked, since nothing the user did produced this toast', () => {
    windowOpen.mockReturnValue(null);

    openExternalUrl(TARGET, 'thread "Book a flight"');

    expect(blockedToast()?.message).toContain('(requested by thread "Book a flight")');
    expect(postClientLog).toHaveBeenCalledWith('nav', 'external-url-blocked', {
      url: TARGET,
      source: 'thread "Book a flight"',
    });
  });

  it('keeps the attribution on a retry that is blocked again', () => {
    windowOpen.mockReturnValue(null);
    openExternalUrl(TARGET, 'an app');

    blockedToast()!.opts.action.onClick();

    expect(blockedToast()?.message).toContain('(requested by an app)');
  });

  it('the toast action retries the open, and dismisses the toast once it lands', () => {
    windowOpen.mockReturnValue(null);
    openExternalUrl(TARGET);
    const toast = blockedToast()!;

    // The retry runs inside a real click, which carries transient user
    // activation, so the blocker lets this one through.
    windowOpen.mockReturnValue(fakeWindow());
    toast.opts.action.onClick();

    expect(windowOpen).toHaveBeenLastCalledWith(TARGET, '_blank');
    expect(toastMocks.dismissToast).toHaveBeenCalledWith(toast.opts.key);
    expect(postClientLog).toHaveBeenLastCalledWith('nav', 'external-url-opened-on-retry', { url: TARGET, source: null });
  });

  it('a retry that is blocked again keeps the offer up and says pop-ups are off', () => {
    windowOpen.mockReturnValue(null);
    openExternalUrl(TARGET);

    blockedToast()!.opts.action.onClick();

    expect(toastMocks.dismissToast).not.toHaveBeenCalled();
    expect(blockedToast()?.message).toContain('Allow pop-ups');
    expect(blockedToast()?.opts.action.label).toBe('Open');
  });

  it('keys the toast per url, so a repeat refreshes one offer and a second url is its own', () => {
    windowOpen.mockReturnValue(null);

    openExternalUrl(TARGET);
    const first = blockedToast()!.opts.key;
    openExternalUrl(TARGET);
    const repeat = blockedToast()!.opts.key;
    openExternalUrl('https://example.com/other');
    const other = blockedToast()!.opts.key;

    expect(repeat).toBe(first);
    expect(other).not.toBe(first);
  });

  it('never auto-dismisses: losing the toast would lose the url', () => {
    windowOpen.mockReturnValue(null);

    openExternalUrl(TARGET);

    expect(blockedToast()?.opts).not.toHaveProperty('autoDismissMs');
  });

  it('truncates a huge url so the 4KB breadcrumb cap cannot swallow the line', () => {
    windowOpen.mockReturnValue(null);
    const huge = `data:text/plain,${'x'.repeat(5000)}`;

    openExternalUrl(huge);

    const logged = postClientLog.mock.calls[0][2] as { url: string };
    expect(logged.url.length).toBeLessThan(250);
    // The button still carries the WHOLE url: only the text was shortened.
    windowOpen.mockReturnValue(fakeWindow());
    blockedToast()!.opts.action.onClick();
    expect(windowOpen).toHaveBeenLastCalledWith(huge, '_blank');
  });

  it('the iOS-PWA safari branch is untouched: no window.open, so nothing to block', () => {
    platformMocks.isIOSPwa = true;
    prefMocks.target = 'safari';
    windowOpen.mockReturnValue(null);

    openExternalUrl(TARGET);

    expect(window.location.href).toBe(`x-safari-${TARGET}`);
    expect(windowOpen).not.toHaveBeenCalled();
    expect(toastMocks.showToast).not.toHaveBeenCalled();
    expect(postClientLog).not.toHaveBeenCalled();
  });

  it('the iOS-PWA ask branch is untouched: the share sheet is not a popup', () => {
    platformMocks.isIOSPwa = true;
    prefMocks.target = 'ask';
    windowOpen.mockReturnValue(null);
    const share = vi.fn(() => Promise.resolve());
    vi.stubGlobal('navigator', { ...navigator, share });

    openExternalUrl(TARGET);

    expect(share).toHaveBeenCalledWith({ url: TARGET });
    expect(windowOpen).not.toHaveBeenCalled();
    expect(toastMocks.showToast).not.toHaveBeenCalled();
    expect(postClientLog).not.toHaveBeenCalled();
  });

  it('the iOS-PWA in-app branch DOES report a block, since it opens a window', () => {
    platformMocks.isIOSPwa = true;
    prefMocks.target = 'in-app';
    windowOpen.mockReturnValue(null);

    openExternalUrl(TARGET);

    expect(blockedToast()?.message).toContain(TARGET);
    expect(postClientLog).toHaveBeenCalledWith('nav', 'external-url-blocked', { url: TARGET, source: null });
  });
});

describe('openExternalUrl: external link target modes', () => {
  const TARGET = 'https://example.com/docs';

  beforeEach(() => {
    platformMocks.isIOSPwa = true;
    prefMocks.target = 'safari';
    windowOpen.mockReset();
    windowOpen.mockReturnValue(fakeWindow());
    toastMocks.showToast.mockClear();
    toastMocks.dismissToast.mockClear();
    postClientLog.mockClear();
    fakeLocation = { href: APP_URL };
    vi.stubGlobal('open', windowOpen);
    vi.stubGlobal('location', fakeLocation);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('in-app: keeps the link in the PWA web view, the pre-2026-08 behaviour', () => {
    prefMocks.target = 'in-app';

    openExternalUrl(TARGET);

    expect(windowOpen).toHaveBeenCalledWith(TARGET, '_blank');
    expect(window.location.href).toBe(APP_URL);
  });

  it('ask: opens the OS share sheet, so iOS offers the real default browser', () => {
    prefMocks.target = 'ask';
    const share = stubShare(() => Promise.resolve());

    openExternalUrl(TARGET);

    expect(share).toHaveBeenCalledWith({ url: TARGET });
    expect(window.location.href).toBe(APP_URL);
    expect(windowOpen).not.toHaveBeenCalled();
  });

  it('ask: shares SYNCHRONOUSLY, since an await would spend the user activation', () => {
    prefMocks.target = 'ask';
    const share = stubShare(() => Promise.resolve());

    openExternalUrl(TARGET);

    // Asserted with no `await` between the call and the check: the Web Share
    // API refuses without transient activation, and a microtask boundary before
    // share() loses it.
    expect(share).toHaveBeenCalledTimes(1);
  });

  it('ask: no navigator.share at all falls back to the Safari hand-off', () => {
    prefMocks.target = 'ask';
    vi.stubGlobal('navigator', { ...navigator, share: undefined });

    openExternalUrl(TARGET);

    expect(window.location.href).toBe(`x-safari-${TARGET}`);
    expect(windowOpen).not.toHaveBeenCalled();
  });

  it('ask: a failed share falls back to Safari rather than dead-ending', async () => {
    prefMocks.target = 'ask';
    const err = new Error('sharing not permitted');
    err.name = 'NotAllowedError';
    stubShare(() => Promise.reject(err));

    openExternalUrl(TARGET);
    await Promise.resolve();
    await Promise.resolve();

    expect(window.location.href).toBe(`x-safari-${TARGET}`);
  });

  it('ask: the user dismissing the sheet is respected, not overridden with Safari', async () => {
    prefMocks.target = 'ask';
    const err = new Error('share canceled');
    err.name = 'AbortError';
    stubShare(() => Promise.reject(err));

    openExternalUrl(TARGET);
    await Promise.resolve();
    await Promise.resolve();

    expect(window.location.href).toBe(APP_URL);
    expect(windowOpen).not.toHaveBeenCalled();
  });

  it('every mode leaves non-http(s) schemes alone', () => {
    const share = stubShare(() => Promise.resolve());

    for (const target of ['safari', 'ask', 'in-app'] as const) {
      prefMocks.target = target;
      windowOpen.mockClear();
      openExternalUrl('mailto:someone@example.com');
      expect(windowOpen).toHaveBeenCalledWith('mailto:someone@example.com', '_blank');
    }
    expect(share).not.toHaveBeenCalled();
    expect(window.location.href).toBe(APP_URL);
  });

  it('no mode has any effect off an installed iOS PWA', () => {
    platformMocks.isIOSPwa = false;
    const share = stubShare(() => Promise.resolve());

    for (const target of ['safari', 'ask', 'in-app'] as const) {
      prefMocks.target = target;
      windowOpen.mockClear();
      openExternalUrl(TARGET);
      expect(windowOpen).toHaveBeenCalledWith(TARGET, '_blank');
    }
    expect(share).not.toHaveBeenCalled();
    expect(window.location.href).toBe(APP_URL);
  });
});
