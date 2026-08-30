import { describe, it, expect, beforeEach } from 'vitest';
import {
  pairingCodeToAdopt,
  takePairingCodeFromUrl,
  takeUnspentPairingCodeFromUrl,
  resetPairingCodeSeedForTest,
  PAIR_CODE_PARAM,
} from './pairingCodeSeed';

const CODE = '01234567';

describe('pairingCodeToAdopt', () => {
  it('adopts the eight digits a scanned QR carried', () => {
    expect(pairingCodeToAdopt(`?${PAIR_CODE_PARAM}=${CODE}`)).toBe(CODE);
    // Order and neighbours do not matter.
    expect(pairingCodeToAdopt(`?a=1&${PAIR_CODE_PARAM}=${CODE}&b=2`)).toBe(CODE);
    // A code that picked up padding on the way here is still that code.
    expect(pairingCodeToAdopt(`?${PAIR_CODE_PARAM}=%20${CODE}%20`)).toBe(CODE);
    // Leading zeros are part of the code, never trimmed off as a number.
    expect(pairingCodeToAdopt(`?${PAIR_CODE_PARAM}=00000001`)).toBe('00000001');
  });

  it('drops anything that is not eight digits rather than posting it', () => {
    for (const bad of [
      '',
      '   ',
      '1234567',
      '123456789',
      '0123456a',
      '1234 5678',
      '-1234567',
      '../../etc',
      '<script>',
    ]) {
      expect(
        pairingCodeToAdopt(`?${PAIR_CODE_PARAM}=${encodeURIComponent(bad)}`),
        `should have rejected ${JSON.stringify(bad)}`,
      ).toBeNull();
    }
  });

  it('is null when the parameter is absent', () => {
    expect(pairingCodeToAdopt('')).toBeNull();
    expect(pairingCodeToAdopt('?other=1')).toBeNull();
  });
});

// Vitest runs in Node with no jsdom, so `window.location` and `window.history`
// are faked with just enough surface for the read and the strip. Same shape
// `hash-deeplink-router.test.ts` uses.
const currentUrl = { value: 'https://mac.tail1234.ts.net/~/' };
Object.defineProperty(window, 'location', {
  configurable: true,
  get() {
    const u = new URL(currentUrl.value);
    return { href: u.href, search: u.search, pathname: u.pathname, origin: u.origin };
  },
});
Object.defineProperty(window, 'history', {
  configurable: true,
  value: {
    replaceState: (_state: unknown, _title: string, url: string) => {
      currentUrl.value = new URL(url, currentUrl.value).href;
    },
  },
});

describe('takePairingCodeFromUrl', () => {
  beforeEach(() => {
    resetPairingCodeSeedForTest();
  });

  /** Land on `search`, take the code, and report what the address bar holds. */
  function takeAt(search: string): { code: string | null; after: string } {
    currentUrl.value = `https://mac.tail1234.ts.net/~/${search}`;
    const code = takePairingCodeFromUrl();
    return { code, after: new URL(currentUrl.value).search };
  }

  it('hands back the code and strips the parameter', () => {
    const { code, after } = takeAt(`?${PAIR_CODE_PARAM}=${CODE}`);
    expect(code).toBe(CODE);
    // The code works once and expires, so a URL still carrying it is a URL
    // that will stop working. A reload, a bookmark and an installed PWA's
    // start URL must not keep it.
    expect(after).toBe('');
  });

  it('leaves the other parameters where they were', () => {
    const { code, after } = takeAt(`?pick=&${PAIR_CODE_PARAM}=${CODE}`);
    expect(code).toBe(CODE);
    expect(after).toBe('?pick=');
  });

  it('strips an invalid code too, rather than leaving a dead one on screen', () => {
    const { code, after } = takeAt(`?${PAIR_CODE_PARAM}=nope`);
    expect(code).toBeNull();
    expect(after).toBe('');
  });

  it('reads once, so a remount cannot re-read a URL already cleaned', () => {
    expect(takeAt(`?${PAIR_CODE_PARAM}=${CODE}`).code).toBe(CODE);
    // The same answer, not null. `main.tsx` calls this for the strip and
    // `PairingGate` for the value, and neither may depend on running first.
    expect(takePairingCodeFromUrl()).toBe(CODE);
  });

  it('touches nothing when there is no parameter', () => {
    const { code, after } = takeAt('?pick=');
    expect(code).toBeNull();
    expect(after).toBe('?pick=');
  });
});

// A minimal localStorage, since Vitest runs in Node with none.
const stored = new Map<string, string>();
Object.defineProperty(window, 'localStorage', {
  configurable: true,
  value: {
    getItem: (k: string) => stored.get(k) ?? null,
    setItem: (k: string, v: string) => void stored.set(k, v),
    removeItem: (k: string) => void stored.delete(k),
  },
});

describe('takeUnspentPairingCodeFromUrl', () => {
  beforeEach(() => {
    resetPairingCodeSeedForTest();
    stored.clear();
  });

  /** Land on a launch URL carrying `code` and ask for it, as a cold start does. */
  function launchWith(code: string): string | null {
    resetPairingCodeSeedForTest();
    currentUrl.value = `https://mac.tail1234.ts.net/~/?${PAIR_CODE_PARAM}=${code}`;
    return takeUnspentPairingCodeFromUrl();
  }

  it('hands back a code this client has not tried', () => {
    expect(launchWith(CODE)).toBe(CODE);
  });

  it('refuses the same code on a later launch', () => {
    // iOS relaunches from the `start_url` it stored at install, so an installed
    // icon carries one code for good. Redeeming it again fails and spends part
    // of the gateway's wrong-guess budget.
    expect(launchWith(CODE)).toBe(CODE);
    expect(launchWith(CODE)).toBeNull();
    expect(launchWith(CODE)).toBeNull();
  });

  it('still takes a different code', () => {
    expect(launchWith(CODE)).toBe(CODE);
    expect(launchWith('76543210')).toBe('76543210');
    // And the first stays spent, so both records are kept.
    expect(launchWith(CODE)).toBeNull();
  });

  it('is null when the launch URL carries no code', () => {
    resetPairingCodeSeedForTest();
    currentUrl.value = 'https://mac.tail1234.ts.net/~/';
    expect(takeUnspentPairingCodeFromUrl()).toBeNull();
  });

  it('answers the same code twice within one page load', () => {
    // Handing it out is what spends it, so an unmemoized second call would
    // answer null to the same document. A remount of the pairing form would
    // then lose the code the first mount was still redeeming.
    expect(launchWith(CODE)).toBe(CODE);
    expect(takeUnspentPairingCodeFromUrl()).toBe(CODE);
    expect(takeUnspentPairingCodeFromUrl()).toBe(CODE);
  });
});
