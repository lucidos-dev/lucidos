import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { ui } from './ui';

describe('lucidos.ui.startThread', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response(null, { status: 200 })));
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('POSTs to /api/v1/ui/navigate with target=new-chat (no prompt)', async () => {
    await ui.startThread();
    expect(fetch).toHaveBeenCalledTimes(1);
    const [path, init] = (fetch as unknown as ReturnType<typeof vi.fn>).mock.calls[0];
    expect(path).toBe('/api/v1/ui/navigate');
    expect(init.method).toBe('POST');
    const body = JSON.parse(init.body);
    expect(body.target).toBe('new-chat');
    expect(body.params).toEqual({});
  });

  it('passes prompt through params when provided', async () => {
    await ui.startThread({ prompt: 'Set up a daily standup trigger' });
    const [, init] = (fetch as unknown as ReturnType<typeof vi.fn>).mock.calls[0];
    const body = JSON.parse(init.body);
    expect(body.target).toBe('new-chat');
    expect(body.params).toEqual({ prompt: 'Set up a daily standup trigger' });
  });

  it('omits params.prompt when prompt is empty string', async () => {
    await ui.startThread({ prompt: '' });
    const [, init] = (fetch as unknown as ReturnType<typeof vi.fn>).mock.calls[0];
    const body = JSON.parse(init.body);
    expect(body.params).toEqual({});
  });

  it('rejects when prompt is not a string', async () => {
    await expect(ui.startThread({ prompt: 123 as unknown as string })).rejects.toThrow(TypeError);
    expect(fetch).not.toHaveBeenCalled();
  });
});

describe('isIOSAgent', () => {
  it('true for iPhone / iPad / iPod user agents', async () => {
    const { isIOSAgent } = await import('./ui');
    expect(isIOSAgent({ userAgent: 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)' })).toBe(true);
    expect(isIOSAgent({ userAgent: 'Mozilla/5.0 (iPad; CPU OS 16_0 like Mac OS X)' })).toBe(true);
    expect(isIOSAgent({ userAgent: 'Mozilla/5.0 (iPod touch; CPU iPhone OS 15_0 like Mac OS X)' })).toBe(true);
  });

  it('true for iPadOS masquerading as a desktop Mac with touch', async () => {
    const { isIOSAgent } = await import('./ui');
    expect(isIOSAgent({
      userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15)',
      platform: 'MacIntel',
      maxTouchPoints: 5,
    })).toBe(true);
  });

  it('false for desktop browsers and touchless Macs', async () => {
    const { isIOSAgent } = await import('./ui');
    expect(isIOSAgent({ userAgent: 'Mozilla/5.0 (Windows NT 10.0) Chrome/120', platform: 'Win32', maxTouchPoints: 0 })).toBe(false);
    expect(isIOSAgent({ userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15)', platform: 'MacIntel', maxTouchPoints: 0 })).toBe(false);
  });

  it('false when navigator is unavailable', async () => {
    const { isIOSAgent } = await import('./ui');
    expect(isIOSAgent(undefined)).toBe(false);
  });
});

describe('ui.watchPreferences — live theme reaction wiring', () => {
  let sseOn: ReturnType<typeof vi.fn>;
  let sseConnect: ReturnType<typeof vi.fn>;
  let getMock: ReturnType<typeof vi.fn>;
  let mqChangeListeners: Array<() => void>;
  let mqQueries: string[];
  let origMatchMedia: unknown;

  beforeEach(() => {
    vi.resetModules();
    sseOn = vi.fn();
    sseConnect = vi.fn();
    getMock = vi.fn().mockResolvedValue({ theme: 'system' });
    mqChangeListeners = [];
    mqQueries = [];
    vi.doMock('./sse', () => ({ sse: { on: sseOn, connect: sseConnect } }));
    vi.doMock('./preferences', () => ({ preferences: { get: getMock } }));
    origMatchMedia = (globalThis as { matchMedia?: unknown }).matchMedia;
    (globalThis as { matchMedia?: unknown }).matchMedia = (q: string) => {
      mqQueries.push(q);
      return {
        matches: false,
        addEventListener: (type: string, fn: () => void) => { if (type === 'change') mqChangeListeners.push(fn); },
        removeEventListener: () => {},
      };
    };
  });

  afterEach(() => {
    vi.doUnmock('./sse');
    vi.doUnmock('./preferences');
    (globalThis as { matchMedia?: unknown }).matchMedia = origMatchMedia;
  });

  it('subscribes to PreferencesChanged and connects SSE', async () => {
    const { ui: freshUi } = await import('./ui');
    freshUi.watchPreferences();
    expect(sseOn).toHaveBeenCalledWith('PreferencesChanged', expect.any(Function));
    expect(sseConnect).toHaveBeenCalledTimes(1);
  });

  it('off iOS: attaches a prefers-color-scheme listener that re-applies preferences', async () => {
    // The node test env's navigator UA carries no iPhone/iPad token, so
    // isIOSAgent() is false and the OS-appearance listener is installed.
    const { ui: freshUi } = await import('./ui');
    freshUi.watchPreferences();
    expect(mqQueries).toContain('(prefers-color-scheme: light)');
    expect(mqChangeListeners).toHaveLength(1);
    // Firing the OS light/dark flip re-applies (applyPreferences fetches prefs).
    mqChangeListeners[0]();
    expect(getMock).toHaveBeenCalled();
  });

  it('is idempotent — a second watchPreferences() does not double-subscribe', async () => {
    const { ui: freshUi } = await import('./ui');
    freshUi.watchPreferences();
    freshUi.watchPreferences();
    expect(sseOn).toHaveBeenCalledTimes(1);
    expect(sseConnect).toHaveBeenCalledTimes(1);
    expect(mqChangeListeners).toHaveLength(1);
  });
});
