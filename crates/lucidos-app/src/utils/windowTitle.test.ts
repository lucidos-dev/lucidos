import { describe, it, expect, beforeEach, vi } from 'vitest';

const platform = vi.hoisted(() => ({ isTauri: false }));
vi.mock('./platform', () => ({ isTauri: () => platform.isTauri }));

const setWindowTitle = vi.hoisted(() => vi.fn((_title: string) => Promise.resolve()));
vi.mock('./tauri', () => ({ setWindowTitle }));

const {
  documentTitle,
  nativeWindowTitle,
  pushNativeWindowTitle,
  resetWindowTitlePush,
} = await import('./windowTitle');

const flush = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  platform.isTauri = false;
  setWindowTitle.mockClear();
  setWindowTitle.mockResolvedValue(undefined);
  resetWindowTitlePush();
});

describe('the browser tab', () => {
  it('carries the product name, the workspace and the unread count', () => {
    expect(documentTitle('dev', 0)).toBe('Lucidos - dev');
    expect(documentTitle('dev', 2)).toBe('(2) Lucidos - dev');
  });

  it('degrades to the product name alone when no workspace is known', () => {
    expect(documentTitle('', 0)).toBe('Lucidos');
    expect(documentTitle('   ', 0)).toBe('Lucidos');
    expect(documentTitle('', 3)).toBe('(3) Lucidos');
  });
});

describe('the native window', () => {
  it('is named after the workspace alone, with no count', () => {
    expect(nativeWindowTitle('dev')).toBe('dev');
    expect(nativeWindowTitle('My Workspace')).toBe('My Workspace');
  });

  it('falls back to the product name with no workspace', () => {
    expect(nativeWindowTitle('')).toBe('Lucidos');
    expect(nativeWindowTitle('  ')).toBe('Lucidos');
  });
});

describe('the push', () => {
  it('says nothing in a browser', async () => {
    await pushNativeWindowTitle('dev');
    expect(setWindowTitle).not.toHaveBeenCalled();
  });

  it('names the window in the desktop client', async () => {
    platform.isTauri = true;
    await pushNativeWindowTitle('dev');
    expect(setWindowTitle).toHaveBeenCalledWith('dev');
  });

  it('does not repeat itself when the name has not changed', async () => {
    platform.isTauri = true;
    await pushNativeWindowTitle('dev');
    await pushNativeWindowTitle('dev');
    expect(setWindowTitle).toHaveBeenCalledTimes(1);
    await pushNativeWindowTitle('myws');
    expect(setWindowTitle).toHaveBeenLastCalledWith('myws');
    expect(setWindowTitle).toHaveBeenCalledTimes(2);
  });

  it('applies the newer name last however slow the first push is', async () => {
    platform.isTauri = true;
    let releaseFirst = () => {};
    setWindowTitle.mockImplementationOnce(
      () => new Promise<void>((resolve) => { releaseFirst = resolve; }),
    );
    const first = pushNativeWindowTitle('dev');
    await flush();
    const second = pushNativeWindowTitle('myws');
    await flush();
    expect(setWindowTitle).toHaveBeenCalledTimes(1);
    releaseFirst();
    await Promise.all([first, second]);
    expect(setWindowTitle.mock.calls.map((c) => c[0])).toEqual(['dev', 'myws']);
  });

  it('swallows a rejected push, since a stale menu entry is not worth a toast', async () => {
    platform.isTauri = true;
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    setWindowTitle.mockRejectedValueOnce(new Error('no such window'));
    await expect(pushNativeWindowTitle('dev')).resolves.toBeUndefined();
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });

  it('retries the same name after a rejected push', async () => {
    platform.isTauri = true;
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    setWindowTitle.mockRejectedValueOnce(new Error('no such window'));
    await pushNativeWindowTitle('dev');
    await pushNativeWindowTitle('dev');
    expect(setWindowTitle).toHaveBeenCalledTimes(2);
    warn.mockRestore();
  });
});
