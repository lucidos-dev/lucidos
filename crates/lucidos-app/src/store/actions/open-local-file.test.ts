// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from 'vitest';

const { openExternalUrl } = vi.hoisted(() => ({ openExternalUrl: vi.fn() }));
vi.mock('../../utils/openExternalUrl', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/openExternalUrl')>()),
  openExternalUrl,
}));
vi.mock('../../utils/platform', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/platform')>()),
  isTauri: () => false,
}));

import { openLocalFile } from './artifacts';
import { toasts } from '../store';

beforeEach(() => {
  openExternalUrl.mockClear();
  toasts.value = [];
});

describe('openLocalFile in a browser', () => {
  it('opens a protocol-relative web link by the external-link preference, never by a bare window.open', () => {
    const open = vi.spyOn(window, 'open').mockReturnValue(null);
    openLocalFile('//example.com/page', 'a message');
    expect(openExternalUrl).toHaveBeenCalledWith(`${window.location.protocol}//example.com/page`, 'a message');
    expect(open).not.toHaveBeenCalled();
    open.mockRestore();
  });

  it('says a disk path cannot be opened here', () => {
    openLocalFile('/Applications/Thing.dmg');
    expect(openExternalUrl).not.toHaveBeenCalled();
    expect(toasts.value.some((t) => t.message.includes('outside this workspace'))).toBe(true);
  });
});
