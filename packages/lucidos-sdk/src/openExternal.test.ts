import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
vi.mock('./modelPicker', () => ({ ModelPicker: {} }));

const navigate = vi.hoisted(
  () => vi.fn<(path: string, body?: unknown) => Promise<void>>(() => Promise.resolve()),
);

const { ui } = await import('./ui');
const { adoptHostExternalLinks, SAFARI_SCHEME_PREFIX } = await import('./externalLinks');

const TARGET = 'https://example.com/docs';

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

async function setCachedTarget(target: string | undefined): Promise<void> {
  await ui.applyPreferences();
  if (target !== undefined) prefsResponse.external_link_target = target;
  else delete prefsResponse.external_link_target;
  await ui.applyPreferences();
}

let prefsResponse: Record<string, string>;
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
  apiUrl: (suffix: string) => `/api/v1${suffix}`,
  dataMountUrl: (path: string) => `/data/${path}`,
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
      void ui.openExternal(TARGET);
      expect(share).toHaveBeenCalledTimes(1);
    });
  });

  it('falls back to the host when the mode has not been cached yet', async () => {
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

describe('ui.openExternal: the frame opens the tab itself', () => {
  function stubWindowOpen(opened: { closed: boolean; opener: unknown } | null) {
    const open = vi.fn((_url: string, _target: string) => opened);
    vi.stubGlobal('window', { open });
    return open;
  }

  beforeEach(() => {
    navigate.mockClear();
    stubDesktopBrowser();
  });

  afterEach(() => {
    adoptHostExternalLinks(null);
    vi.unstubAllGlobals();
  });

  it('opens synchronously in the tap and posts no navigate', async () => {
    adoptHostExternalLinks({ frameOpens: 'tab' });
    const opened = { closed: false, opener: {} as unknown };
    const open = stubWindowOpen(opened);

    const done = ui.openExternal(TARGET);
    expect(open).toHaveBeenCalledWith(TARGET, '_blank');
    await done;

    expect(navigate).not.toHaveBeenCalled();
  });

  it('severs opener so the page cannot reach back into the app', async () => {
    adoptHostExternalLinks({ frameOpens: 'tab' });
    const opened = { closed: false, opener: {} as unknown };
    stubWindowOpen(opened);

    await ui.openExternal(TARGET);

    expect(opened.opener).toBeNull();
  });

  it('counts the tab as opened when the popup refuses an opener write', async () => {
    adoptHostExternalLinks({ frameOpens: 'tab' });
    const opened = {
      closed: false,
      get opener(): unknown {
        return {};
      },
      set opener(_value: unknown) {
        throw new DOMException('cross-origin', 'SecurityError');
      },
    };
    stubWindowOpen(opened);

    await expect(ui.openExternal(TARGET)).resolves.toBeUndefined();

    expect(navigate).not.toHaveBeenCalled();
  });

  it.each([
    ['null', null],
    ['an already closed window', { closed: true, opener: null }],
  ])('falls back to the host route when the open returns %s', async (_label, opened) => {
    adoptHostExternalLinks({ frameOpens: 'tab' });
    stubWindowOpen(opened);

    await ui.openExternal(TARGET);

    expect(navigate).toHaveBeenCalledWith('/ui/navigate', {
      target: 'url',
      params: { url: TARGET },
    });
  });

  it.each(['mailto:someone@example.com', 'javascript:alert(1)', 'tel:+4700000000'])(
    'never opens %s from the frame',
    async (url) => {
      adoptHostExternalLinks({ frameOpens: 'tab' });
      const open = stubWindowOpen({ closed: false, opener: null });

      await ui.openExternal(url);

      expect(open).not.toHaveBeenCalled();
      expect(navigate).toHaveBeenCalledTimes(1);
    },
  );

  it.each([
    ['no push at all', undefined],
    ['a host answer', { frameOpens: 'host' }],
    ['a malformed answer', { frameOpens: 'yes' }],
    ['the retired boolean answer', { opensInFrame: true }],
  ])('keeps the host route after %s', async (_label, push) => {
    if (push !== undefined) adoptHostExternalLinks(push);
    const open = stubWindowOpen({ closed: false, opener: null });

    await ui.openExternal(TARGET);

    expect(open).not.toHaveBeenCalled();
    expect(navigate).toHaveBeenCalledTimes(1);
  });
});

describe('ui.openExternal: the frame hands off to Safari itself', () => {
  function stubWindowOpen(opened: { closed: boolean; opener: unknown } | null) {
    const open = vi.fn((_url: string, _target: string) => opened);
    const postMessage = vi.fn();
    vi.stubGlobal('window', { open, location: { origin: 'http://localhost' }, parent: { postMessage } });
    return { open, postMessage };
  }

  beforeEach(() => {
    navigate.mockClear();
    stubIOSPwa();
    adoptHostExternalLinks({ frameOpens: 'safari' });
  });

  afterEach(() => {
    adoptHostExternalLinks(null);
    vi.unstubAllGlobals();
  });

  it('opens x-safari-<url> synchronously in the tap and posts no navigate', async () => {
    const { open, postMessage } = stubWindowOpen({ closed: false, opener: {} });

    const done = ui.openExternal(TARGET);
    expect(open).toHaveBeenCalledWith(`${SAFARI_SCHEME_PREFIX}${TARGET}`, '_blank');
    await done;

    expect(navigate).not.toHaveBeenCalled();
    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        op: 'log.app-link',
        args: expect.objectContaining({ step: 'frame-opened', href: TARGET, opens: 'safari' }),
      }),
      expect.anything(),
    );
  });

  it('falls back to the host route and traces it when the open returns null', async () => {
    const { postMessage } = stubWindowOpen(null);

    await ui.openExternal(TARGET);

    expect(navigate).toHaveBeenCalledWith('/ui/navigate', { target: 'url', params: { url: TARGET } });
    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        op: 'log.app-link',
        args: expect.objectContaining({ step: 'frame-open-blocked', href: TARGET, opens: 'safari' }),
      }),
      expect.anything(),
    );
  });

  it('never prefixes a mailto: link', async () => {
    const { open } = stubWindowOpen({ closed: false, opener: null });

    await ui.openExternal('mailto:someone@example.com');

    expect(open).not.toHaveBeenCalled();
    expect(navigate).toHaveBeenCalledTimes(1);
  });
});

describe('ui.openExternal: a top-level app page opens the tab itself', () => {
  function stubTopLevelWindow(
    opened: { closed: boolean; opener: unknown } | null,
    extra: Record<string, unknown> = {},
  ) {
    const open = vi.fn((_url: string, _target: string) => opened);
    const win: Record<string, unknown> = { open, ...extra };
    win.parent = win;
    vi.stubGlobal('window', win);
    return open;
  }

  beforeEach(() => {
    navigate.mockClear();
    stubDesktopBrowser();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('opens synchronously in the tap with no host answer', async () => {
    const open = stubTopLevelWindow({ closed: false, opener: {} });

    const done = ui.openExternal(TARGET);
    expect(open).toHaveBeenCalledWith(TARGET, '_blank');
    await done;

    expect(navigate).not.toHaveBeenCalled();
  });

  it('falls back to the host route when the open is blocked', async () => {
    stubTopLevelWindow(null);

    await ui.openExternal(TARGET);

    expect(navigate).toHaveBeenCalledTimes(1);
  });

  it('keeps the host route in an installed iOS PWA', async () => {
    stubIOSPwa();
    const open = stubTopLevelWindow({ closed: false, opener: null }, {
      matchMedia: () => ({ matches: true }),
    });

    await ui.openExternal(TARGET);

    expect(open).not.toHaveBeenCalled();
    expect(navigate).toHaveBeenCalledTimes(1);
  });

  it('keeps the host route in the desktop app', async () => {
    const open = stubTopLevelWindow({ closed: false, opener: null }, {
      __TAURI_INTERNALS__: {},
    });

    await ui.openExternal(TARGET);

    expect(open).not.toHaveBeenCalled();
    expect(navigate).toHaveBeenCalledTimes(1);
  });
});

describe('primeDevicePreferences: the cache cannot depend on theming', () => {
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

    await mod.primeDevicePreferences();
    await mod.ui.openExternal(TARGET);

    expect(share).toHaveBeenCalledWith({ url: TARGET });
    expect(navigate).not.toHaveBeenCalled();
  });

  it('reads on every client, since the Autocorrect switch is read everywhere', async () => {
    vi.resetModules();
    const mod = await import('./ui');
    stubDesktopBrowser();
    const get = vi.fn(() => Promise.resolve({} as Record<string, string>));
    prefsGet = get;

    await mod.primeDevicePreferences();

    expect(get).toHaveBeenCalledTimes(1);
  });

  it('does not read again once applyPreferences has landed', async () => {
    vi.resetModules();
    const mod = await import('./ui');
    stubDesktopBrowser();
    let calls = 0;
    prefsGet = () => { calls++; return Promise.resolve({}); };

    await mod.ui.applyPreferences();
    await mod.primeDevicePreferences();

    expect(calls).toBe(1);
  });

  it('lets a themed app read on its own, and never lets an older answer win', async () => {
    vi.resetModules();
    const mod = await import('./ui');
    const share = vi.fn(() => Promise.resolve());
    stubIOSPwa(share);
    let releasePrime: (prefs: Record<string, string>) => void = () => {};
    prefsGet = () => new Promise((resolve) => { releasePrime = resolve; });
    const primed = mod.primeDevicePreferences();
    prefsGet = () => Promise.resolve({ external_link_target: 'ask' });
    await mod.ui.applyPreferences();

    releasePrime({ external_link_target: 'safari' });
    await primed;
    await mod.ui.openExternal(TARGET);

    expect(share).toHaveBeenCalledWith({ url: TARGET });
  });

  it('fetches once even when called repeatedly', async () => {
    vi.resetModules();
    const mod = await import('./ui');
    stubIOSPwa(() => Promise.resolve());
    let calls = 0;
    prefsGet = () => { calls++; return Promise.resolve({ external_link_target: 'ask' }); };

    await Promise.all([
      mod.primeDevicePreferences(),
      mod.primeDevicePreferences(),
    ]);
    await mod.primeDevicePreferences();

    expect(calls).toBe(1);
  });

  it('a failed fetch leaves the host path working rather than wedging', async () => {
    vi.resetModules();
    const mod = await import('./ui');
    stubIOSPwa(() => Promise.resolve());
    prefsGet = () => Promise.reject(new Error('offline'));

    await expect(mod.primeDevicePreferences()).rejects.toThrow('offline');
    await mod.ui.openExternal(TARGET);

    expect(navigate).toHaveBeenCalledTimes(1);
  });
});
