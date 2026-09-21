import { describe, expect, it, afterEach } from 'vitest';
import {
  adoptCapability,
  capabilityCarrier,
  currentCapability,
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

afterEach(() => {
  globalThis.document.querySelector = originalQuerySelector;
});

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
