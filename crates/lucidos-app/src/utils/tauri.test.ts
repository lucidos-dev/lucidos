import { describe, it, expect, beforeEach, vi } from 'vitest';
import { checkAppUpdate, currentWindowLabel, listen, windowReadyToShow } from './tauri';

const invoke = vi.fn((): Promise<unknown> => Promise.resolve());

/** Install the Tauri bridge stub, optionally with the per-window metadata the
 *  runtime injects into every main frame. */
function stubInternals(windowLabel?: string): void {
  (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
    invoke,
    transformCallback: () => 0,
    ...(windowLabel === undefined
      ? {}
      : { metadata: { currentWindow: { label: windowLabel } } }),
  };
}

beforeEach(() => {
  invoke.mockClear();
  stubInternals();
});

describe('windowReadyToShow', () => {
  it('signals once and never again', () => {
    // The launch signal: the shell is holding a hidden window waiting for it.
    windowReadyToShow();
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith('window_ready_to_show', undefined);

    // Its callers repeat. `applyTheme` runs on every theme toggle and every
    // system-appearance change, and `boot()` and `applyTheme` both fire in the
    // same document. A second signal would re-show a window the user may have
    // dismissed to the menu bar since, so the module-level flag swallows it.
    windowReadyToShow();
    windowReadyToShow();
    expect(invoke).toHaveBeenCalledTimes(1);
  });
});

describe('listen', () => {
  /** The whole point of the target: tauri's dispatch short-circuits on
   *  `EventTarget::Any`, so an `Any` listener receives every OTHER window's
   *  `emit_to(label, ...)` too. Registering `Any` is what let one window's
   *  `native-window-active` overwrite every other window's cache, which reports
   *  a backgrounded window as active and suppresses its workspace's banner. */
  it('registers scoped to THIS window, not Any', async () => {
    stubInternals('window-2');
    await listen('native-window-active', () => {});
    expect(invoke).toHaveBeenCalledWith('plugin:event|listen', {
      event: 'native-window-active',
      target: { kind: 'AnyLabel', label: 'window-2' },
      handler: 0,
    });
  });

  it('falls back to Any when the window label is unreadable', async () => {
    // The previous behaviour: hearing too much beats hearing nothing.
    stubInternals();
    await listen('native-window-active', () => {});
    expect(invoke).toHaveBeenCalledWith(
      'plugin:event|listen',
      expect.objectContaining({ target: { kind: 'Any' } }),
    );
  });

  it('reads the injected window label, rejecting an empty or absent one', () => {
    stubInternals('main');
    expect(currentWindowLabel()).toBe('main');
    stubInternals('');
    expect(currentWindowLabel()).toBeNull();
    stubInternals();
    expect(currentWindowLabel()).toBeNull();
  });
});

/** The client BINARY owns `check_app_update` and the frontend BUNDLE owns this
 *  call. The two reach each other across releases, so every shape the command
 *  has ever returned arrives here. `invoke<T>` casts rather than checks, so an
 *  unparsed payload lands straight in UI copy. */
describe('checkAppUpdate', () => {
  it('reads the offer, notes included', async () => {
    invoke.mockResolvedValueOnce({ version: '0.27.0', notes: '## 0.27.0\n\nFixed things.' });
    expect(await checkAppUpdate()).toEqual({ version: '0.27.0', notes: '## 0.27.0\n\nFixed things.' });
  });

  it('null when there is no update', async () => {
    invoke.mockResolvedValueOnce(null);
    expect(await checkAppUpdate()).toBeNull();
  });

  it('reads a bare version string from a client older than the notes field', async () => {
    // The mirror of the bug that prompted this. There, a new client's struct
    // reached a page expecting a string, and rendered "Lucidos [object Object]
    // available". Here an old client's string reaches a page expecting the
    // struct, and nothing downstream may see the difference.
    invoke.mockResolvedValueOnce('0.26.4');
    expect(await checkAppUpdate()).toEqual({ version: '0.26.4', notes: null });
  });

  it('drops notes that are not markdown text', async () => {
    invoke.mockResolvedValueOnce({ version: '0.27.0', notes: { body: 'nope' } });
    expect(await checkAppUpdate()).toEqual({ version: '0.27.0', notes: null });
  });

  it('rejects a shape it cannot read, rather than reporting no update', async () => {
    // `null` is the answer for "up to date", so an unreadable payload must not
    // borrow it: the caller records the rejection on the check-error surface.
    invoke.mockResolvedValueOnce({ latest: { version: '0.27.0' } });
    await expect(checkAppUpdate()).rejects.toThrow(/check_app_update/);
  });

  it('rejects a blank version', async () => {
    invoke.mockResolvedValueOnce({ version: '  ', notes: null });
    await expect(checkAppUpdate()).rejects.toThrow(/check_app_update/);
  });
});
