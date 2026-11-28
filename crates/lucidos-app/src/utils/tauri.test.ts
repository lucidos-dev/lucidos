import { describe, it, expect, beforeEach, vi } from 'vitest';
import { checkAppUpdate, currentWindowLabel, listen, windowReadyToShow } from './tauri';

const invoke = vi.fn((): Promise<unknown> => Promise.resolve());

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
    windowReadyToShow();
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith('window_ready_to_show', undefined);

    windowReadyToShow();
    windowReadyToShow();
    expect(invoke).toHaveBeenCalledTimes(1);
  });
});

describe('listen', () => {
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
    invoke.mockResolvedValueOnce('0.26.4');
    expect(await checkAppUpdate()).toEqual({ version: '0.26.4', notes: null });
  });

  it('drops notes that are not markdown text', async () => {
    invoke.mockResolvedValueOnce({ version: '0.27.0', notes: { body: 'nope' } });
    expect(await checkAppUpdate()).toEqual({ version: '0.27.0', notes: null });
  });

  it('rejects a shape it cannot read, rather than reporting no update', async () => {
    invoke.mockResolvedValueOnce({ latest: { version: '0.27.0' } });
    await expect(checkAppUpdate()).rejects.toThrow(/check_app_update/);
  });

  it('rejects a blank version', async () => {
    invoke.mockResolvedValueOnce({ version: '  ', notes: null });
    await expect(checkAppUpdate()).rejects.toThrow(/check_app_update/);
  });
});
