// @vitest-environment jsdom
import { expect, it, vi } from 'vitest';
import { postClientLog } from './clientLog';
import { installWorkspaceStorage } from './workspaceStorage';

vi.mock('./clientLog', () => ({ postClientLog: vi.fn() }));

it('samples migration and shell write failures by area without replacing quota errors', () => {
  const frame = document.createElement('iframe');
  document.body.append(frame);
  const { localStorage, sessionStorage } = frame.contentWindow!;
  const clock = vi.spyOn(performance, 'now').mockReturnValue(100);
  const quotaError = new DOMException('full', 'QuotaExceededError');
  const prototype: Storage = Object.getPrototypeOf(localStorage);
  const rawGet = prototype.getItem;
  const rawRemove = prototype.removeItem;
  try {
    localStorage.setItem('private-key', 'private value');
    vi.spyOn(prototype, 'setItem').mockImplementation(() => { throw quotaError; });
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    installWorkspaceStorage(localStorage, 'alpha');
    expect(postClientLog).toHaveBeenCalledTimes(1);
    expect(() => localStorage.setItem('lucidos-theme', 'dark')).toThrow(quotaError);
    expect(() => sessionStorage.setItem('lucidos-nav-history', '[]')).toThrow(quotaError);
    expect(postClientLog).toHaveBeenCalledTimes(2);
    expect(postClientLog).toHaveBeenNthCalledWith(1, 'storage-pressure', 'quota', {
      area: 'local',
      usage: expect.objectContaining({ other: { entries: 1, codeUnits: 24 } }),
    });
    expect(postClientLog).toHaveBeenNthCalledWith(2, 'storage-pressure', 'quota', {
      area: 'session',
      usage: expect.objectContaining({ other: { entries: 0, codeUnits: 0 } }),
    });
    clock.mockReturnValue(60_100);
    vi.spyOn(prototype, 'key').mockImplementationOnce(() => {
      throw new DOMException('blocked', 'SecurityError');
    });
    expect(() => localStorage.setItem('lucidos-theme', 'dark')).toThrow(quotaError);
    expect(postClientLog).toHaveBeenNthCalledWith(3, 'storage-pressure', 'quota', {
      area: 'local', inventoryUnavailable: true,
    });
  } finally {
    vi.restoreAllMocks();
    prototype.getItem = rawGet;
    prototype.removeItem = rawRemove;
    Reflect.deleteProperty(prototype, Symbol.for('lucidos.workspaceStorage.installed'));
    localStorage.clear();
    frame.remove();
  }
});
