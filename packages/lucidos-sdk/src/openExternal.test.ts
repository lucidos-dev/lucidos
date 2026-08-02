/**
 * `lucidos.ui.openExternal` and the delegated anchor handler behind it.
 *
 * The load-bearing property is that the "Ask" mode's `navigator.share` runs
 * INSIDE the app iframe, synchronously, on the click that triggered it. The Web
 * Share API requires transient user activation, and activation survives neither
 * a `postMessage` hop nor an `await`, so a host-side share (or one behind a
 * preference fetch) would be refused on every single app link while chat links
 * kept working. Everything else routes to the host, which owns the one external
 * -link choke point.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const navigate = vi.hoisted(() => vi.fn(() => Promise.resolve()));

const { ui } = await import('./ui');

const TARGET = 'https://example.com/docs';

/** An installed-iOS-PWA navigator: iPhone UA plus standalone display mode. */
function stubIOSPwa(share?: (data: { url?: string }) => Promise<void>): void {
  vi.stubGlobal('navigator', {
    userAgent: 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15',
    platform: 'iPhone',
    maxTouchPoints: 5,
    standalone: true,
    ...(share ? { share } : {}),
  });
  vi.stubGlobal('matchMedia', () => ({ matches: true }));
}

function stubDesktopBrowser(): void {
  vi.stubGlobal('navigator', {
    userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/140',
    platform: 'MacIntel',
    maxTouchPoints: 0,
    share: () => Promise.resolve(),
  });
  vi.stubGlobal('matchMedia', () => ({ matches: false }));
}

/** Drive the cache the way `applyPreferences` does, since `openExternal` must
 *  read it without awaiting a fetch. */
async function setCachedTarget(target: string | undefined): Promise<void> {
  await ui.applyPreferences();
  if (target !== undefined) prefsResponse.external_link_target = target;
  else delete prefsResponse.external_link_target;
  await ui.applyPreferences();
}

let prefsResponse: Record<string, string>;
/** Indirected so a test can count fetches or make one fail. Reset per test to
 *  simply resolve `prefsResponse`. */
let prefsGet: () => Promise<Record<string, string>>;

vi.mock('./preferences', () => ({
  preferences: {
    get: () => prefsGet(),
    set: () => Promise.resolve(),
  },
}));

vi.mock('./_fetch', () => ({
  request: () => Promise.resolve({}),
  requestVoid: (path: string, init: { body?: string }) => {
    navigate(path, init.body ? JSON.parse(init.body) : undefined);
    return Promise.resolve();
  },
}));

describe('ui.openExternal', () => {
  beforeEach(() => {
    prefsResponse = {};
    prefsGet = () => Promise.resolve(prefsResponse);
    navigate.mockClear();
    vi.stubGlobal('document', {
      documentElement: {
        setAttribute: () => {},
        getAttribute: () => 'dark',
        style: { setProperty: () => {}, background: '' },
      },
      head: { appendChild: () => {} },
      createElement: () => ({}),
    });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('rejects a non-string url rather than posting garbage to the host', async () => {
    stubDesktopBrowser();
    expect(() => ui.openExternal(undefined as unknown as string)).toThrow(TypeError);
    expect(navigate).not.toHaveBeenCalled();
  });

  it('routes to the host outside an installed iOS PWA, whatever the mode says', async () => {
    stubDesktopBrowser();
    await setCachedTarget('ask');

    await ui.openExternal(TARGET);

    expect(navigate).toHaveBeenCalledWith('/ui/navigate', {
      target: 'url',
      params: { url: TARGET },
    });
  });

  it('routes to the host on iOS in safari mode, so the host keeps one choke point', async () => {
    const share = vi.fn(() => Promise.resolve());
    stubIOSPwa(share);
    await setCachedTarget('safari');

    await ui.openExternal(TARGET);

    expect(share).not.toHaveBeenCalled();
    expect(navigate).toHaveBeenCalledTimes(1);
  });

  it('routes to the host on iOS in in-app mode', async () => {
    const share = vi.fn(() => Promise.resolve());
    stubIOSPwa(share);
    await setCachedTarget('in-app');

    await ui.openExternal(TARGET);

    expect(share).not.toHaveBeenCalled();
    expect(navigate).toHaveBeenCalledTimes(1);
  });

  it('ask mode shares from INSIDE the iframe and posts nothing to the host', async () => {
    const share = vi.fn(() => Promise.resolve());
    stubIOSPwa(share);
    await setCachedTarget('ask');

    await ui.openExternal(TARGET);

    expect(share).toHaveBeenCalledWith({ url: TARGET });
    expect(navigate).not.toHaveBeenCalled();
  });

  it('ask mode calls share SYNCHRONOUSLY, before the returned promise is awaited', () => {
    const share = vi.fn(() => Promise.resolve());
    stubIOSPwa(share);
    prefsResponse.external_link_target = 'ask';

    return ui.applyPreferences().then(() => {
      // No await between the call and the assertion: an intervening microtask
      // would have spent the click's transient activation and the OS would
      // refuse the sheet.
      void ui.openExternal(TARGET);
      expect(share).toHaveBeenCalledTimes(1);
    });
  });

  it('falls back to the host when the mode has not been cached yet', async () => {
    // A tap before applyPreferences() has ever run. Guessing "ask" here would
    // burn the activation on a sheet the user may not have asked for; the host
    // path always works.
    //
    // The cache is module state, so this needs a module instance no other test
    // has warmed. Re-importing after resetModules is the only honest way to
    // reach the cold branch.
    vi.resetModules();
    const { ui: coldUi } = await import('./ui');
    const share = vi.fn(() => Promise.resolve());
    stubIOSPwa(share);

    await coldUi.openExternal(TARGET);

    expect(share).not.toHaveBeenCalled();
    expect(navigate).toHaveBeenCalledTimes(1);
  });

  it('ask mode leaves non-http(s) schemes to the platform via the host', async () => {
    const share = vi.fn(() => Promise.resolve());
    stubIOSPwa(share);
    await setCachedTarget('ask');

    await ui.openExternal('mailto:someone@example.com');

    expect(share).not.toHaveBeenCalled();
    expect(navigate).toHaveBeenCalledWith('/ui/navigate', {
      target: 'url',
      params: { url: 'mailto:someone@example.com' },
    });
  });

  it('ask mode falls back to the host when the sheet itself fails', async () => {
    const err = new Error('sharing not permitted');
    err.name = 'NotAllowedError';
    stubIOSPwa(() => Promise.reject(err));
    await setCachedTarget('ask');

    await ui.openExternal(TARGET);

    expect(navigate).toHaveBeenCalledTimes(1);
  });

  it('ask mode respects the user dismissing the sheet and opens nothing', async () => {
    const err = new Error('share canceled');
    err.name = 'AbortError';
    stubIOSPwa(() => Promise.reject(err));
    await setCachedTarget('ask');

    await ui.openExternal(TARGET);

    expect(navigate).not.toHaveBeenCalled();
  });

  it('ask mode with no navigator.share at all still opens, via the host', async () => {
    stubIOSPwa();
    await setCachedTarget('ask');

    await ui.openExternal(TARGET);

    expect(navigate).toHaveBeenCalledTimes(1);
  });

  it('an unrecognized stored mode degrades to the host path, never to nothing', async () => {
    const share = vi.fn(() => Promise.resolve());
    stubIOSPwa(share);
    await setCachedTarget('chrome');

    await ui.openExternal(TARGET);

    expect(share).not.toHaveBeenCalled();
    expect(navigate).toHaveBeenCalledTimes(1);
  });
});

/**
 * `applyPreferences()` is optional: the app-authoring guidance tells apps that
 * ship their own complete visual identity to skip Lucidos theming entirely. But
 * `browser.ts` installs the delegated link handler for EVERY app that loads
 * sdk.js, so if the cache warmed only through theming, those apps would hold a
 * null mode forever and quietly ignore the user's "Ask" choice on every link.
 */
describe('primeExternalLinkTarget: the cache cannot depend on theming', () => {
  beforeEach(() => {
    prefsResponse = {};
    prefsGet = () => Promise.resolve(prefsResponse);
    navigate.mockClear();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('warms the mode for an app that never calls applyPreferences', async () => {
    vi.resetModules();
    const mod = await import('./ui');
    const share = vi.fn(() => Promise.resolve());
    stubIOSPwa(share);
    prefsResponse.external_link_target = 'ask';

    await mod.primeExternalLinkTarget();
    await mod.ui.openExternal(TARGET);

    expect(share).toHaveBeenCalledWith({ url: TARGET });
    expect(navigate).not.toHaveBeenCalled();
  });

  it('does not fetch off an installed iOS PWA, where the mode is never read', async () => {
    vi.resetModules();
    const mod = await import('./ui');
    stubDesktopBrowser();
    const get = vi.fn(() => Promise.resolve({} as Record<string, string>));
    prefsGet = get;

    await mod.primeExternalLinkTarget();

    expect(get).not.toHaveBeenCalled();
  });

  it('fetches once even when called repeatedly', async () => {
    vi.resetModules();
    const mod = await import('./ui');
    stubIOSPwa(() => Promise.resolve());
    let calls = 0;
    prefsGet = () => { calls++; return Promise.resolve({ external_link_target: 'ask' }); };

    await Promise.all([
      mod.primeExternalLinkTarget(),
      mod.primeExternalLinkTarget(),
    ]);
    await mod.primeExternalLinkTarget();

    expect(calls).toBe(1);
  });

  it('a failed fetch leaves the host path working rather than wedging', async () => {
    vi.resetModules();
    const mod = await import('./ui');
    stubIOSPwa(() => Promise.resolve());
    prefsGet = () => Promise.reject(new Error('offline'));

    await expect(mod.primeExternalLinkTarget()).rejects.toThrow('offline');
    await mod.ui.openExternal(TARGET);

    expect(navigate).toHaveBeenCalledTimes(1);
  });
});
