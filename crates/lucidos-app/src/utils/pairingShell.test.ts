import { describe, it, expect, vi, afterEach } from 'vitest';
import { isPairingShellDocument } from './pairingShell';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));


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

describe('the cold-start fast path in index.html', () => {
  const html: string = readFileSync(resolve(__dirname, '../../index.html'), 'utf-8');

  function fastPath(): string {
    const at = html.indexOf("base.getAttribute('href') !== '/~/'");
    expect(at, 'index.html still has the cold-start fast path').toBeGreaterThan(-1);
    const end = html.indexOf('lucidos-last-workspace', at);
    expect(end, 'the fast path still reads the remembered workspace').toBeGreaterThan(at);
    return html.slice(at, end);
  }

  it('stands down on a pairing document', () => {
    expect(
      fastPath(),
      'the redirect must not fire on the screen the gateway serves in its place',
    ).toContain('meta[name="lucidos-pairing"]');
  });

  it('still stands down on ?pick', () => {
    expect(fastPath()).toContain("has('pick')");
  });

  it('still carries the deep link through the redirect', () => {
    const redirect = html.slice(html.indexOf("location.replace('/' + encodeURIComponent(slug)"));
    expect(redirect.slice(0, 120)).toContain('location.search');
    expect(redirect.slice(0, 120)).toContain('location.hash');
  });
});
