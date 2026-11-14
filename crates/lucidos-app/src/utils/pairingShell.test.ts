import { describe, it, expect, vi, afterEach } from 'vitest';
import { isPairingShellDocument } from './pairingShell';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));

/**
 * The pairing marker and the one redirect it stands down.
 *
 * The gateway serves the pairing screen in place, at the url asked for, and
 * that screen IS the picker document. So it carries the picker's base href, and
 * the cold-start fast path keys on exactly that base. Before the marker existed
 * the two could not be told apart, and the redirect landed back on the screen
 * that had just served it: 8097 navigations in 25 seconds, measured.
 *
 * Rationale and the rest of the invariants:
 * `docs/plans/2026-08-21-the-pairing-shell-is-not-the-picker.md`.
 */

/** Install a document whose `querySelector` answers only the given selectors. */
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

  /** The redirect script: from its base-href guard to the redirect it guards.
   *  Read as source rather than executed, because it is an inline classic
   *  script in the document, not a module this suite can import. */
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
    // The escape that keeps the picker list reachable, and what stops a
    // since-deleted slug looping through the gateway's 302.
    expect(fastPath()).toContain("has('pick')");
  });

  it('still carries the deep link through the redirect', () => {
    // A push tap can cold-launch into picker context with the notification on
    // the url. Redirecting to a bare `/<slug>/` dropped it before the app booted.
    const redirect = html.slice(html.indexOf("location.replace('/' + encodeURIComponent(slug)"));
    expect(redirect.slice(0, 120)).toContain('location.search');
    expect(redirect.slice(0, 120)).toContain('location.hash');
  });
});
