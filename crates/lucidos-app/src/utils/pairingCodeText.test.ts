import { describe, expect, it } from 'vitest';
import { pairingCodeFromText } from './pairingCodeText';

describe('pairingCodeFromText', () => {
  it('reads the code out of the URL a QR encodes', () => {
    expect(pairingCodeFromText('https://mac.ts.net/~/?pair=01234567')).toBe('01234567');
    expect(pairingCodeFromText('http://192.168.1.5:5252/~/?pair=47118899')).toBe('47118899');
    // Trailing whitespace is what a scanner and a clipboard both add.
    expect(pairingCodeFromText('  https://mac.ts.net/~/?pair=01234567\n')).toBe('01234567');
  });

  it('takes a bare code, however the user spaced it', () => {
    expect(pairingCodeFromText('01234567')).toBe('01234567');
    expect(pairingCodeFromText(' 4711 8899 ')).toBe('47118899');
    expect(pairingCodeFromText('4711-8899')).toBe('47118899');
  });

  it('never mines digits out of text that merely contains some', () => {
    // The scanner points at whatever is on screen, so it reads other codes,
    // other links and other people's phone numbers. Each has to be refused so
    // it keeps looking rather than submitting a wrong code.
    for (const text of [
      '',
      '   ',
      'https://mac.ts.net/~/',
      'https://mac.ts.net:5252/dev/threads/12345678',
      'https://example.com/?pair=01234567abc',
      'WIFI:S:mynet;T:WPA;P:01234567;;',
      'call me on 555 0123 4567',
      '0123456',
      '012345678',
      '0123456a',
      'not a url ://',
    ]) {
      expect(pairingCodeFromText(text), text).toBeNull();
    }
  });
});
