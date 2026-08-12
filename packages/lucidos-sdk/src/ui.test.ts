import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
// The theme-precedence cases this file used to carry now live beside the rule
// itself, in `appearance.test.ts`. `resolveThemePreference` moved into the
// shared appearance contract when the four hand-copied versions of it were
// collapsed into one.
import { ui, webFontUrl } from './ui';

describe('webFontUrl', () => {
  it('serves the DEFAULT font from the local engine, never from Google', () => {
    const url = webFontUrl('fira-code');
    expect(url).toBe('/api/v1/fonts/fira-code.css');
    // The point of vendoring it: a workspace with no internet must still
    // render its own default font. A googleapis host here is the regression.
    expect(url).not.toContain('googleapis');
  });

  it('still fetches the opt-in fonts from Google', () => {
    for (const key of ['inter', 'jetbrains-mono', 'ibm-plex-mono'] as const) {
      expect(webFontUrl(key)).toContain('fonts.googleapis.com');
    }
  });

  it('has nothing to load for fonts already on the device', () => {
    // No garbage case: the parameter is a `FontFamily`, and `resolveFontKey`
    // (the only producer) cannot hand out a key outside the set.
    expect(webFontUrl('monospace')).toBeUndefined();
    expect(webFontUrl('system')).toBeUndefined();
  });
});

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

describe('lucidos.ui.toast', () => {
  let origParent: unknown;
  let postMessage: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    origParent = (globalThis as { parent?: unknown }).parent;
    postMessage = vi.fn();
    // A non-self parent so window.parent !== window — drives the postMessage path.
    (globalThis as { parent?: unknown }).parent = { postMessage };
  });
  afterEach(() => {
    (globalThis as { parent?: unknown }).parent = origParent;
    vi.unstubAllGlobals();
  });

  it('posts lucidos:ui:toast with the normalized payload and returns void', () => {
    const ret = ui.toast('Saved', 'success', { durationMs: 2000, dismissable: false });
    expect(ret).toBeUndefined();
    expect(postMessage).toHaveBeenCalledWith(
      { type: 'lucidos:ui:toast', payload: { message: 'Saved', type: 'success', durationMs: 2000, dismissable: false, key: undefined, spinning: undefined } },
      '*',
    );
  });

  it('defaults type to info and leaves opts undefined when omitted', () => {
    ui.toast('Heads up');
    expect(postMessage).toHaveBeenCalledWith(
      { type: 'lucidos:ui:toast', payload: { message: 'Heads up', type: 'info', durationMs: undefined, dismissable: undefined, key: undefined, spinning: undefined } },
      '*',
    );
  });

  it('forwards opts.spinning for an indeterminate "work in progress" toast', () => {
    ui.toast('Reindexing…', 'info', { key: 'reindex', spinning: true });
    expect(postMessage.mock.calls[0][0].payload.spinning).toBe(true);
  });

  // Same defensive shape as durationMs/dismissable: a bad value must not cross
  // the bridge as a truthy non-boolean the host would then treat as `true`.
  it('leaves spinning undefined when omitted or non-boolean', () => {
    ui.toast('No spinner');
    expect(postMessage.mock.calls[0][0].payload.spinning).toBeUndefined();
    ui.toast('Bad spinner', 'info', { spinning: 'yes' as unknown as boolean });
    expect(postMessage.mock.calls[1][0].payload.spinning).toBeUndefined();
  });

  it('forwards opts.key for in-place replacement when provided', () => {
    ui.toast('Opening from Drive…', 'info', { key: 'drive-open' });
    expect(postMessage.mock.calls[0][0].payload.key).toBe('drive-open');
  });

  it('leaves key undefined in the payload when omitted or empty', () => {
    ui.toast('No key here');
    expect(postMessage.mock.calls[0][0].payload.key).toBeUndefined();
    ui.toast('Empty key', 'info', { key: '' });
    expect(postMessage.mock.calls[1][0].payload.key).toBeUndefined();
  });

  it('degrades an unknown type to info rather than failing', () => {
    ui.toast('x', 'bogus' as unknown as 'info');
    expect(postMessage.mock.calls[0][0].payload.type).toBe('info');
  });

  it('throws TypeError on an empty/non-string message (programming error)', () => {
    expect(() => ui.toast('')).toThrow(TypeError);
    expect(() => ui.toast(123 as unknown as string)).toThrow(TypeError);
    expect(postMessage).not.toHaveBeenCalled();
  });

  it('standalone (no host parent): logs to console, never posts', () => {
    (globalThis as { parent?: unknown }).parent = globalThis; // window.parent === window
    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    const errSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    ui.toast('all good', 'success');
    ui.toast('broken', 'error');
    expect(logSpy).toHaveBeenCalled();
    expect(errSpy).toHaveBeenCalled();
    expect(postMessage).not.toHaveBeenCalled();
    logSpy.mockRestore();
    errSpy.mockRestore();
  });
});

describe('lucidos.ui.dismissToast', () => {
  let origParent: unknown;
  let postMessage: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    origParent = (globalThis as { parent?: unknown }).parent;
    postMessage = vi.fn();
    (globalThis as { parent?: unknown }).parent = { postMessage };
  });
  afterEach(() => {
    (globalThis as { parent?: unknown }).parent = origParent;
    vi.unstubAllGlobals();
  });

  it('posts lucidos:ui:dismissToast with the key and returns void', () => {
    const ret = ui.dismissToast('reindex');
    expect(ret).toBeUndefined();
    expect(postMessage).toHaveBeenCalledWith(
      { type: 'lucidos:ui:dismissToast', payload: { key: 'reindex' } },
      '*',
    );
  });

  // A missing key is a programming error, not a runtime condition: without one
  // the host has nothing to look up, so failing loudly beats a silent no-op.
  it('throws TypeError on an empty/non-string key', () => {
    expect(() => ui.dismissToast('')).toThrow(TypeError);
    expect(() => ui.dismissToast(undefined as unknown as string)).toThrow(TypeError);
    expect(() => ui.dismissToast(7 as unknown as string)).toThrow(TypeError);
    expect(postMessage).not.toHaveBeenCalled();
  });

  it('standalone (no host parent): logs to console, never posts', () => {
    (globalThis as { parent?: unknown }).parent = globalThis; // window.parent === window
    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    ui.dismissToast('reindex');
    expect(logSpy).toHaveBeenCalled();
    expect(postMessage).not.toHaveBeenCalled();
    logSpy.mockRestore();
  });
});

describe('lucidos.ui.prompt', () => {
  let origParent: unknown;
  let postMessage: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    origParent = (globalThis as { parent?: unknown }).parent;
    postMessage = vi.fn();
    (globalThis as { parent?: unknown }).parent = { postMessage };
  });
  afterEach(() => {
    (globalThis as { parent?: unknown }).parent = origParent;
    vi.unstubAllGlobals();
  });

  // The SDK installs a single `message` listener; replay the host reply by
  // dispatching a synthetic message event with the id the SDK just posted.
  function replyToLastPrompt(value: unknown) {
    const calls = postMessage.mock.calls;
    const sent = calls[calls.length - 1][0] as { id: string };
    globalThis.dispatchEvent(
      Object.assign(new Event('message'), { data: { type: 'lucidos:ui:prompt:result', id: sent.id, value } }),
    );
  }

  it('posts lucidos:ui:prompt and resolves the string returned by the host', async () => {
    const p = ui.prompt({ message: 'Name?', defaultValue: 'Untitled' });
    const sent = postMessage.mock.calls[0][0];
    expect(sent.type).toBe('lucidos:ui:prompt');
    expect(typeof sent.id).toBe('string');
    expect(sent.payload).toMatchObject({ message: 'Name?', defaultValue: 'Untitled', okLabel: 'OK', cancelLabel: 'Cancel', multiline: false });
    replyToLastPrompt('Alice');
    await expect(p).resolves.toBe('Alice');
  });

  it('resolves null when the host reply carries a non-string value (cancel)', async () => {
    const p = ui.prompt({ message: 'Name?' });
    replyToLastPrompt(null);
    await expect(p).resolves.toBeNull();
  });

  it('rejects when message is empty/non-string, without posting', async () => {
    await expect(ui.prompt({ message: '' })).rejects.toThrow(TypeError);
    await expect(ui.prompt({ message: 123 as unknown as string })).rejects.toThrow(TypeError);
    expect(postMessage).not.toHaveBeenCalled();
  });

  it('standalone (no host parent): falls back to native window.prompt', async () => {
    (globalThis as { parent?: unknown }).parent = globalThis; // window.parent === window
    const nativePrompt = vi.fn(() => 'Bob');
    vi.stubGlobal('prompt', nativePrompt);
    await expect(ui.prompt({ message: 'Name?', defaultValue: 'd' })).resolves.toBe('Bob');
    expect(nativePrompt).toHaveBeenCalledWith('Name?', 'd');
    expect(postMessage).not.toHaveBeenCalled();
  });
});

describe('lucidos.ui.previewFile', () => {
  let origParent: unknown;
  let postMessage: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    origParent = (globalThis as { parent?: unknown }).parent;
    postMessage = vi.fn();
    (globalThis as { parent?: unknown }).parent = { postMessage };
  });
  afterEach(() => {
    (globalThis as { parent?: unknown }).parent = origParent;
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  /** Replay the host's answer for the request the SDK just posted. */
  function replyToLastPreview(reply: { ok: boolean; error?: string }) {
    const calls = postMessage.mock.calls;
    const sent = calls[calls.length - 1][0] as { id: string };
    globalThis.dispatchEvent(
      Object.assign(new Event('message'), {
        data: { type: 'lucidos:ui:preview-file:result', id: sent.id, ...reply },
      }),
    );
  }

  it('posts the locator and its lines, and resolves once the host is showing it', async () => {
    const p = ui.previewFile({ file_path: 'repo:r-1:file:src/main.rs', line: 510, line_end: 520 });
    const sent = postMessage.mock.calls[0][0];
    expect(sent.type).toBe('lucidos:ui:preview-file');
    expect(typeof sent.id).toBe('string');
    expect(sent.payload).toEqual({ file_path: 'repo:r-1:file:src/main.rs', line: 510, line_end: 520 });
    replyToLastPreview({ ok: true });
    await expect(p).resolves.toBeUndefined();
  });

  it('omits lines that were not given', async () => {
    const p = ui.previewFile({ file_path: 'artifacts/notes.md' });
    expect(postMessage.mock.calls[0][0].payload).toEqual({
      file_path: 'artifacts/notes.md', line: undefined, line_end: undefined,
    });
    replyToLastPreview({ ok: true });
    await p;
  });

  // A non-numeric line is dropped here rather than shipped: the host applies the
  // documented degradation to what it receives, and there is no reason to make
  // it reject something the SDK can see is not a line.
  it('drops a non-numeric line rather than forwarding it', async () => {
    const p = ui.previewFile({ file_path: 'artifacts/notes.md', line: '12' as unknown as number });
    expect(postMessage.mock.calls[0][0].payload.line).toBeUndefined();
    replyToLastPreview({ ok: true });
    await p;
  });

  it('rejects with the host reason when the host refuses', async () => {
    const p = ui.previewFile({ file_path: 'artifacts/notes.md' });
    replyToLastPreview({ ok: false, error: 'previewFile: file_path must be a non-empty string' });
    await expect(p).rejects.toThrow('file_path must be a non-empty string');
  });

  it('throws on a missing or empty file_path, without posting', async () => {
    await expect(ui.previewFile({ file_path: '' })).rejects.toThrow(TypeError);
    await expect(ui.previewFile({} as unknown as { file_path: string })).rejects.toThrow(TypeError);
    expect(() => ui.previewFile(null as unknown as { file_path: string })).toThrow(TypeError);
    expect(postMessage).not.toHaveBeenCalled();
  });

  // Bounds a lost reply so the pending map can't leak. A second reply arriving
  // late finds no resolver and is dropped, rather than settling twice.
  it('rejects and forgets the request when the host never answers', async () => {
    vi.useFakeTimers();
    const p = ui.previewFile({ file_path: 'artifacts/notes.md' });
    const rejection = expect(p).rejects.toThrow('did not respond');
    await vi.advanceTimersByTimeAsync(60_000);
    await rejection;
    // The entry is gone: a late reply resolves nothing and throws nothing.
    expect(() => replyToLastPreview({ ok: true })).not.toThrow();
  });

  // A popped-out app (its own tab) has no host shell around it, so there is no
  // modal to show. It must NOT quietly fall back to `navigate`: that goes
  // through the engine and lands in whichever OTHER window is running the
  // shell, so the reader would see nothing while a different window navigated
  // its Files panel, and the promise would resolve as if it had worked. The
  // rejection is what lets the app decide (its documented catch escalates).
  it('standalone (no host parent): rejects instead of driving another window', async () => {
    (globalThis as { parent?: unknown }).parent = globalThis; // window.parent === window
    vi.stubGlobal('fetch', vi.fn(async () => new Response(null, { status: 200 })));
    await expect(ui.previewFile({ file_path: 'artifacts/notes.md', line: 3 }))
      .rejects.toThrow('no host to show the preview');
    expect(fetch).not.toHaveBeenCalled();
    expect(postMessage).not.toHaveBeenCalled();
  });
});
