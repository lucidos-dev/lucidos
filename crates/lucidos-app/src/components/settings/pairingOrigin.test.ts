/**
 * Where a pairing QR points, and the rule it must not fork from.
 *
 * The whole difficulty is that the machine minting a code is usually reading
 * the page over loopback, and a QR aimed at `127.0.0.1` helps nobody. So the
 * QR reuses the derivation the Connect URLs Tailscale row already had, and
 * these tests pin the two together rather than testing them apart.
 */
import { describe, it, expect } from 'vitest';
import { tailnetOrigin, pairingOrigin, tailnetConnectRows } from './MobileAccessPage';
import type { TailnetInput } from './MobileAccessPage';
import type { LanRowState } from './MobileAccessPage';

/** A laptop reading the page over loopback, on a machine that is on a tailnet
 *  and listening there. The case the whole feature exists for. */
const base: TailnetInput = {
  here: { protocol: 'http:', hostname: 'localhost', port: '5252' },
  tailnetIp: '100.64.0.7',
  magicDnsName: 'mymac.tailnet-name.ts.net',
  workspaceServeUrl: null,
  bind: 'all',
};

describe('tailnetOrigin', () => {
  it('prefers the MagicDNS name, which is what a person can retype', () => {
    expect(tailnetOrigin(base)).toBe('http://mymac.tailnet-name.ts.net:5252');
  });

  it('falls back to the address when MagicDNS is off', () => {
    expect(tailnetOrigin({ ...base, magicDnsName: null })).toBe('http://100.64.0.7:5252');
  });

  it('keeps the reader on TLS when that is how they got here', () => {
    const overTls = { ...base, here: { protocol: 'https:', hostname: 'localhost', port: '5251' } };
    expect(tailnetOrigin(overTls)).toBe('https://mymac.tailnet-name.ts.net:5251');
  });

  it('takes the origin of a verified serve URL, dropping its workspace path', () => {
    // A pairing link lands on the picker, never on a workspace. The path in the
    // verified URL is the one part that must not come along.
    const served = { ...base, workspaceServeUrl: 'https://mymac.tailnet-name.ts.net/dev/' };
    expect(tailnetOrigin(served)).toBe('https://mymac.tailnet-name.ts.net');
  });

  it('answers nothing when nothing is listening on the tailnet address', () => {
    expect(tailnetOrigin({ ...base, bind: 'loopback' })).toBeNull();
  });

  it('answers nothing when the machine is off a tailnet', () => {
    expect(tailnetOrigin({ ...base, tailnetIp: null })).toBeNull();
  });

  it('survives a serve URL that is not a URL', () => {
    // The engine only ever sets a verified one, so this is the belt: a garbage
    // value must yield no origin rather than throwing inside a render.
    expect(tailnetOrigin({ ...base, workspaceServeUrl: 'not a url' })).toBeNull();
  });
});

describe('tailnetConnectRows agrees with tailnetOrigin', () => {
  // The invariant the extraction exists for: the address a user copies and the
  // address a QR encodes name the same host, in every state.
  const cases: Array<[string, TailnetInput]> = [
    ['plain http over the tailnet', base],
    ['no MagicDNS', { ...base, magicDnsName: null }],
    ['over TLS', { ...base, here: { protocol: 'https:', hostname: 'localhost', port: '5251' } }],
    ['a verified serve URL', { ...base, workspaceServeUrl: 'https://mymac.tailnet-name.ts.net/dev/' }],
    ['loopback bind', { ...base, bind: 'loopback' }],
    ['off the tailnet', { ...base, tailnetIp: null }],
  ];

  for (const [name, input] of cases) {
    it(`names one host for ${name}`, () => {
      const origin = tailnetOrigin(input);
      const rows = tailnetConnectRows({ ...input, scope: '/dev/' });
      if (origin === null) {
        expect(rows).toEqual([]);
        return;
      }
      expect(rows).toHaveLength(1);
      expect(rows[0].url.startsWith(origin)).toBe(true);
    });
  }

  it('still prints the verified serve URL verbatim, path and all', () => {
    const served = { ...base, workspaceServeUrl: 'https://mymac.tailnet-name.ts.net/dev/' };
    const rows = tailnetConnectRows({ ...served, scope: '/dev/' });
    expect(rows[0].url).toBe('https://mymac.tailnet-name.ts.net/dev/');
  });
});

describe('pairingOrigin', () => {
  const lan: LanRowState = { kind: 'url', url: 'http://192.168.1.5:5252' };

  it('takes the tailnet address ahead of the LAN one', () => {
    expect(pairingOrigin({ ...base, lan })).toBe('http://mymac.tailnet-name.ts.net:5252');
  });

  it('falls back to the LAN address when there is no tailnet one', () => {
    expect(pairingOrigin({ ...base, tailnetIp: null, lan })).toBe('http://192.168.1.5:5252');
  });

  it('answers nothing rather than an address a phone cannot reach', () => {
    // The bug the whole derivation exists to avoid. Off any tailnet and bound
    // to loopback, this machine has no address to hand out. A QR aimed at the
    // reader's own `localhost` would be worse than no QR at all.
    for (const lanState of [null, { kind: 'disabled' } as const, { kind: 'none' } as const]) {
      expect(pairingOrigin({ ...base, tailnetIp: null, bind: 'loopback', lan: lanState })).toBeNull();
    }
  });

  it('never answers with a loopback address', () => {
    const loopbackReader = { ...base, here: { protocol: 'http:', hostname: '127.0.0.1', port: '5252' } };
    for (const lanState of [null, lan]) {
      const origin = pairingOrigin({ ...loopbackReader, lan: lanState });
      expect(origin === null || !/localhost|127\./.test(origin)).toBe(true);
    }
  });
});
