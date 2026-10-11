import { describe, expect, it, afterEach } from 'vitest';
import {
  adoptCapability,
  anchorHistoryToDocument,
  capabilityCarrier,
  currentCapability,
  documentRelative,
  inPageFragment,
  splitCapability,
} from './frameCapability';

const PASS = '6a0b~site-publisher~00112233445566778899aabbccddeeff';

const originalQuerySelector = globalThis.document.querySelector;

/**
 * Stand a `<base href>` up for the module to read and write.
 *
 * The suite runs without jsdom (`src/test-setup.ts`), so the element is a stub
 * with the two methods this module uses. `null` is a document the engine
 * stamped nothing into, which is every direct-to-engine load.
 */
function stampBase(href: string | null): { href: string | null } {
  const element = { href };
  globalThis.document.querySelector = ((): Element | null =>
    href === null
      ? null
      : ({
          getAttribute: () => element.href,
          setAttribute: (_name: string, value: string) => {
            element.href = value;
          },
        } as unknown as Element)) as typeof document.querySelector;
  return element;
}

const DOCUMENT_HREF = 'https://host.example/dev/app/site-publisher/?device=abc123';

afterEach(() => {
  globalThis.document.querySelector = originalQuerySelector;
  delete (globalThis as { location?: unknown }).location;
});

/** Where the document itself sits. The suite has no `location` of its own. */
function placeDocument(href = DOCUMENT_HREF): void {
  (globalThis as { location?: unknown }).location = { href };
}

describe('splitCapability', () => {
  it('takes a capability path apart into its three pieces', () => {
    expect(splitCapability(`/dev/~cap/${PASS}/app/site-publisher/`)).toEqual({
      prefix: '/dev',
      capability: PASS,
      rest: '/app/site-publisher/',
    });
    expect(splitCapability(`/dev/~cap/${PASS}/data/artifacts/x.png`)).toEqual({
      prefix: '/dev',
      capability: PASS,
      rest: '/data/artifacts/x.png',
    });
  });

  it('answers null for a path carrying none', () => {
    for (const path of ['/dev/app/x/', '/dev/', '/', '', '/dev/~cap/', '/dev/~cap/tok']) {
      expect(splitCapability(path), path).toBeNull();
    }
  });

});

describe('the pass is read from the base the engine stamped', () => {
  it('finds it, and builds the carrier a URL splices in', () => {
    stampBase(`/dev/~cap/${PASS}/app/site-publisher/`);
    expect(currentCapability()).toBe(PASS);
    expect(capabilityCarrier()).toBe(`/~cap/${PASS}`);
  });

  it('answers nothing direct to an engine, where no base is stamped', () => {
    stampBase(null);
    expect(currentCapability()).toBeNull();
    // `''` rather than null, so every URL builder needs no branch of its own.
    expect(capabilityCarrier()).toBe('');
  });

  it('answers nothing under the SPA shell base, which carries no pass', () => {
    stampBase('/dev/');
    expect(currentCapability()).toBeNull();
    expect(capabilityCarrier()).toBe('');
  });
});

describe('a renewal replaces the pass in place', () => {
  it('swaps the token and keeps everything around it', () => {
    const base = stampBase(`/dev/~cap/${PASS}/app/site-publisher/`);
    expect(adoptCapability('fresh~site-publisher~ff')).toBe(true);
    expect(base.href).toBe('/dev/~cap/fresh~site-publisher~ff/app/site-publisher/');
    // Read live, so a URL built after the renewal carries the new one.
    expect(currentCapability()).toBe('fresh~site-publisher~ff');
  });

  it('does nothing where there is nothing to renew', () => {
    stampBase(null);
    expect(adoptCapability('fresh')).toBe(false);
    const shell = stampBase('/dev/');
    expect(adoptCapability('fresh')).toBe(false);
    expect(shell.href).toBe('/dev/');
  });

  it('refuses a token that would open a second path segment', () => {
    stampBase(`/dev/~cap/${PASS}/app/site-publisher/`);
    expect(adoptCapability('a/b')).toBe(false);
    expect(adoptCapability('')).toBe(false);
    expect(currentCapability()).toBe(PASS);
  });
});

describe('a relative URL the app moves to resolves against its own document', () => {
  const CAPABILITY_BASE = `/dev/~cap/${PASS}/app/site-publisher/`;

  it('keeps the document path and query under a capability base', () => {
    stampBase(CAPABILITY_BASE);
    placeDocument();
    expect(documentRelative('#30d')).toBe(`${DOCUMENT_HREF}#30d`);
    expect(documentRelative('?range=7d')).toBe('https://host.example/dev/app/site-publisher/?range=7d');
    expect(documentRelative(new URL('https://host.example/dev/app/site-publisher/#x'))).toBe(
      'https://host.example/dev/app/site-publisher/#x',
    );
  });

  it('leaves the browser in charge where no pass is stamped', () => {
    placeDocument();
    for (const base of [null, '/dev/', './']) {
      stampBase(base);
      expect(documentRelative('#30d'), String(base)).toBe('#30d');
    }
  });

  it('passes an unparseable URL through, so the browser throws its own error', () => {
    stampBase(CAPABILITY_BASE);
    placeDocument();
    expect(documentRelative('http://[')).toBe('http://[');
  });

  it('anchors pushState and replaceState, and leaves a missing URL alone', () => {
    stampBase(CAPABILITY_BASE);
    placeDocument();
    const calls: unknown[][] = [];
    const record = function (this: unknown, ...args: unknown[]) {
      calls.push([this, ...args]);
    } as History['pushState'];
    const target = { pushState: record, replaceState: record } as Pick<History, 'pushState' | 'replaceState'>;
    anchorHistoryToDocument(target);
    target.replaceState.call(target, { a: 1 }, '', '#probe');
    target.pushState.call(target, null, '', '#pushed');
    target.replaceState.call(target, null, '');
    expect(calls).toEqual([
      [target, { a: 1 }, '', `${DOCUMENT_HREF}#probe`],
      [target, null, '', `${DOCUMENT_HREF}#pushed`],
      [target, null, '', undefined],
    ]);
  });
});

describe('an in-page link stays on this document', () => {
  it('names the fragment on the document URL under a capability base', () => {
    stampBase(`/dev/~cap/${PASS}/app/site-publisher/`);
    placeDocument();
    expect(inPageFragment('#section')).toBe(`${DOCUMENT_HREF}#section`);
    expect(inPageFragment('#')).toBe(`${DOCUMENT_HREF}#`);
  });

  it('leaves every other link, and every document without a pass, to the browser', () => {
    stampBase(`/dev/~cap/${PASS}/app/site-publisher/`);
    placeDocument();
    for (const href of [null, '', 'page.html#section', '?q=1', '/dev/app/x/#y', 'https://example.com/#z']) {
      expect(inPageFragment(href), String(href)).toBeNull();
    }
    stampBase(null);
    expect(inPageFragment('#section')).toBeNull();
  });
});
