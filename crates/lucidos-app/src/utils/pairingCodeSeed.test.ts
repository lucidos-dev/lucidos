import { describe, it, expect, beforeEach } from 'vitest';
import {
  pairingCodeToAdopt,
  hasUnspentPairingLaunch,
  releasePairingLaunch,
  takePairingCodeFromUrl,
  takeUnspentPairingCodeFromUrl,
  resetPairingCodeSeedForTest,
  PAIR_PARAM,
} from './pairingCodeSeed';

const CODE = '01234567';
const SECRET = 'ldp_abcdefghijklmnopqrstuvwxyz';

describe('pairingCodeToAdopt', () => {
  it('adopts the eight digits a scanned QR carried', () => {
    expect(pairingCodeToAdopt(`?${PAIR_PARAM}=${CODE}`)).toBe(CODE);
    expect(pairingCodeToAdopt(`?a=1&${PAIR_PARAM}=${CODE}&b=2`)).toBe(CODE);
    expect(pairingCodeToAdopt(`?${PAIR_PARAM}=%20${CODE}%20`)).toBe(CODE);
    expect(pairingCodeToAdopt(`?${PAIR_PARAM}=00000001`)).toBe('00000001');
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
        pairingCodeToAdopt(`?${PAIR_PARAM}=${encodeURIComponent(bad)}`),
        `should have rejected ${JSON.stringify(bad)}`,
      ).toBeNull();
    }
  });

  it('adopts the long secret a current gateway puts in the link', () => {
    expect(pairingCodeToAdopt(`?${PAIR_PARAM}=${SECRET}`)).toBe(SECRET);
    for (const bad of [
      SECRET.slice(0, -1),
      `${SECRET}a`,
      SECRET.toUpperCase(),
      SECRET.replace('ldp_', 'lgp_'),
      `${SECRET.slice(0, -1)}"`,
    ]) {
      expect(pairingCodeToAdopt(`?${PAIR_PARAM}=${encodeURIComponent(bad)}`), bad).toBeNull();
    }
  });

  it('is null when the parameter is absent', () => {
    expect(pairingCodeToAdopt('')).toBeNull();
    expect(pairingCodeToAdopt('?other=1')).toBeNull();
  });
});

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

  function takeAt(search: string): { code: string | null; after: string } {
    currentUrl.value = `https://mac.tail1234.ts.net/~/${search}`;
    const code = takePairingCodeFromUrl();
    return { code, after: new URL(currentUrl.value).search };
  }

  it('hands back the code and strips the parameter', () => {
    const { code, after } = takeAt(`?${PAIR_PARAM}=${CODE}`);
    expect(code).toBe(CODE);
    expect(after).toBe('');
  });

  it('leaves the other parameters where they were', () => {
    const { code, after } = takeAt(`?pick=&${PAIR_PARAM}=${CODE}`);
    expect(code).toBe(CODE);
    expect(after).toBe('?pick=');
  });

  it('strips an invalid code too, rather than leaving a dead one on screen', () => {
    const { code, after } = takeAt(`?${PAIR_PARAM}=nope`);
    expect(code).toBeNull();
    expect(after).toBe('');
  });

  it('reads once, so a remount cannot re-read a URL already cleaned', () => {
    expect(takeAt(`?${PAIR_PARAM}=${CODE}`).code).toBe(CODE);
    expect(takePairingCodeFromUrl()).toBe(CODE);
  });

  it('touches nothing when there is no parameter', () => {
    const { code, after } = takeAt('?pick=');
    expect(code).toBeNull();
    expect(after).toBe('?pick=');
  });
});

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

  function launchWith(code: string): string | null {
    resetPairingCodeSeedForTest();
    currentUrl.value = `https://mac.tail1234.ts.net/~/?${PAIR_PARAM}=${code}`;
    return takeUnspentPairingCodeFromUrl();
  }

  it('hands back a code this client has not tried', () => {
    expect(launchWith(CODE)).toBe(CODE);
  });

  it('refuses the same code on a later launch', () => {
    expect(launchWith(CODE)).toBe(CODE);
    expect(launchWith(CODE)).toBeNull();
    expect(launchWith(CODE)).toBeNull();
  });

  it('still takes a different code', () => {
    expect(launchWith(CODE)).toBe(CODE);
    expect(launchWith('76543210')).toBe('76543210');
    expect(launchWith(CODE)).toBeNull();
  });

  it('is null when the launch URL carries no code', () => {
    resetPairingCodeSeedForTest();
    currentUrl.value = 'https://mac.tail1234.ts.net/~/';
    expect(takeUnspentPairingCodeFromUrl()).toBeNull();
  });

  it('answers the same code twice within one page load', () => {
    expect(launchWith(CODE)).toBe(CODE);
    expect(takeUnspentPairingCodeFromUrl()).toBe(CODE);
    expect(takeUnspentPairingCodeFromUrl()).toBe(CODE);
  });
});

describe('fragment pairing', () => {
  beforeEach(resetPairingCodeSeedForTest);
  it('prefers the fragment and erases both forms, preserving neighbours', () => {
    currentUrl.value = `https://example.test/~/?pair=${CODE}&pick=#pair=${SECRET}&thread=demo`;
    expect(takePairingCodeFromUrl()).toBe(SECRET);
    expect(currentUrl.value).toBe('https://example.test/~/?pick=#thread=demo');
  });
  it('erases duplicate invalid tokens without falling back to a query credential', () => {
    currentUrl.value = `https://example.test/~/?pair=${CODE}#pair=bad&pair=${SECRET}`;
    expect(takePairingCodeFromUrl()).toBeNull();
    expect(currentUrl.value).toBe('https://example.test/~/');
  });
});

it('consumes the head capture once after the URL was erased', () => {
  resetPairingCodeSeedForTest();
  currentUrl.value = 'https://example.test/~/';
  const boot = window as Window & { lucidosPairingSeed?: string };
  boot.lucidosPairingSeed = SECRET;
  expect(takePairingCodeFromUrl()).toBe(SECRET);
  expect(boot.lucidosPairingSeed).toBeUndefined();
  expect(takePairingCodeFromUrl()).toBe(SECRET);
});

it('blocks bundle redirects only while a captured launch token is unspent', () => {
  resetPairingCodeSeedForTest();
  stored.clear();
  currentUrl.value = 'https://example.test/~/#pair=01234567';
  const boot = window as Window & { lucidosPairingLaunch?: boolean };
  boot.lucidosPairingLaunch = true;
  expect(hasUnspentPairingLaunch()).toBe(true);
  expect(takeUnspentPairingCodeFromUrl()).toBe(CODE);
  expect(hasUnspentPairingLaunch()).toBe(false);
  delete boot.lucidosPairingLaunch;
});

it('allows a verified paired browser to navigate without spending its install seed', () => {
  resetPairingCodeSeedForTest();
  stored.clear();
  currentUrl.value = 'https://example.test/~/#pair=01234567';
  const boot = window as Window & { lucidosPairingLaunch?: boolean };
  boot.lucidosPairingLaunch = true;
  expect(hasUnspentPairingLaunch()).toBe(true);
  releasePairingLaunch();
  expect(hasUnspentPairingLaunch()).toBe(false);
  expect(stored.size).toBe(0);
  expect(takePairingCodeFromUrl()).toBe(CODE);
  delete boot.lucidosPairingLaunch;
});
