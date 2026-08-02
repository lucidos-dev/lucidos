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

const { openExternalUrl } = await import('./openExternalUrl');

// The node test env has neither window.open nor window.location (test-setup.ts
// aliases window to globalThis), so both are installed as globals.
const windowOpen = vi.hoisted(() => vi.fn());
const APP_URL = 'https://app.example.com/ws/dev/';
let fakeLocation: { href: string };

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
    windowOpen.mockClear();
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

    expect(windowOpen).toHaveBeenCalledWith('mailto:someone@example.com', '_blank', 'noopener');
    expect(window.location.href).toBe(APP_URL);
  });

  it('iOS PWA + tel:/file:/data: pass through unrewritten as well', () => {
    platformMocks.isIOSPwa = true;

    const urls = ['tel:+15550000', 'file:///tmp/report.pdf', 'data:text/plain,hi'];
    for (const url of urls) {
      openExternalUrl(url);
      expect(windowOpen).toHaveBeenLastCalledWith(url, '_blank', 'noopener');
    }
    // Pins that each iteration opened, rather than a later one carrying an
    // earlier call past a silently skipped scheme.
    expect(windowOpen).toHaveBeenCalledTimes(urls.length);
    expect(window.location.href).toBe(APP_URL);
  });

  it('non-PWA browser: opens a new tab and never navigates the document', () => {
    platformMocks.isIOSPwa = false;

    openExternalUrl('https://example.com/docs');

    expect(windowOpen).toHaveBeenCalledWith('https://example.com/docs', '_blank', 'noopener');
    expect(window.location.href).toBe(APP_URL);
  });
});

describe('openExternalUrl: external link target modes', () => {
  const TARGET = 'https://example.com/docs';

  beforeEach(() => {
    platformMocks.isIOSPwa = true;
    prefMocks.target = 'safari';
    windowOpen.mockClear();
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

    expect(windowOpen).toHaveBeenCalledWith(TARGET, '_blank', 'noopener');
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
      expect(windowOpen).toHaveBeenCalledWith('mailto:someone@example.com', '_blank', 'noopener');
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
      expect(windowOpen).toHaveBeenCalledWith(TARGET, '_blank', 'noopener');
    }
    expect(share).not.toHaveBeenCalled();
    expect(window.location.href).toBe(APP_URL);
  });
});
