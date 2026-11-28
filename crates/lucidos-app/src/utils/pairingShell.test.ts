import { describe, it, expect, vi, afterEach } from 'vitest';
import { isPairingShellDocument } from './pairingShell';

function documentWith(present: string[]) {
  vi.stubGlobal('document', {
    querySelector: (sel: string) => (present.includes(sel) ? { tagName: 'META' } : null),
  });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('isPairingShellDocument', () => {
  it('is true only when the gateway stamped the marker', () => {
    documentWith(['meta[name="lucidos-pairing"]']);
    expect(isPairingShellDocument()).toBe(true);
  });

  it('is false on an ordinary picker document', () => {
    documentWith([]);
    expect(isPairingShellDocument()).toBe(false);
  });

  it('does not throw where there is no document at all', () => {
    vi.stubGlobal('document', undefined);
    expect(isPairingShellDocument()).toBe(false);
  });
});
