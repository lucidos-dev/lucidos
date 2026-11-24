import { describe, it, expect } from 'vitest';
import { tailnetOrigin, pairingOrigin, tailnetConnectRows } from './MobileAccessPage';
import type { TailnetInput } from './MobileAccessPage';

const base: TailnetInput = {
  here: { protocol: 'http:', hostname: 'localhost', port: '5252' },
  tailnetIp: '100.64.0.7',
  magicDnsName: 'mymac.tailnet-name.ts.net',
  workspaceServeUrl: null,
  gatewayHttps: null,
  bind: 'all',
};

const SERVED = 'https://mymac.tailnet-name.ts.net:5252';

describe('tailnetOrigin', () => {
  it('prefers the MagicDNS name, which is what a person can retype', () => {
    expect(tailnetOrigin(base)).toBe('http://mymac.tailnet-name.ts.net:5252');
  });

  it('falls back to the address when MagicDNS is off', () => {
    expect(tailnetOrigin({ ...base, magicDnsName: null })).toBe('http://100.64.0.7:5252');
  });

  it('never composes an https origin nobody verified', () => {
    const overTls = { ...base, here: { protocol: 'https:', hostname: 'localhost', port: '5251' } };
    expect(tailnetOrigin(overTls)).toBeNull();
  });

  it('takes the origin of a verified serve URL, dropping its workspace path', () => {
    const served = { ...base, workspaceServeUrl: 'https://mymac.tailnet-name.ts.net/dev/' };
    expect(tailnetOrigin(served)).toBe('https://mymac.tailnet-name.ts.net');
  });

  it('answers nothing when nothing is listening on the tailnet address', () => {
    expect(tailnetOrigin({ ...base, bind: 'loopback' })).toBeNull();
  });

  it('answers nothing when the machine is off a tailnet', () => {
    expect(tailnetOrigin({ ...base, tailnetIp: null })).toBeNull();
  });

  it("takes the gateway's own HTTPS origin first, even over a serve URL", () => {
    expect(tailnetOrigin({ ...base, gatewayHttps: SERVED })).toBe(SERVED);
    const both = { ...base, gatewayHttps: SERVED, workspaceServeUrl: 'https://mymac.tailnet-name.ts.net/dev/' };
    expect(tailnetOrigin(both)).toBe(SERVED);
  });

  it('survives a serve URL that is not a URL', () => {
    expect(tailnetOrigin({ ...base, workspaceServeUrl: 'not a url' })).toBeNull();
  });
});

describe('tailnetConnectRows agrees with tailnetOrigin', () => {
  const cases: Array<[string, TailnetInput]> = [
    ['plain http over the tailnet', base],
    ['no MagicDNS', { ...base, magicDnsName: null }],
    ['over TLS', { ...base, here: { protocol: 'https:', hostname: 'localhost', port: '5251' } }],
    ['a verified serve URL', { ...base, workspaceServeUrl: 'https://mymac.tailnet-name.ts.net/dev/' }],
    ['loopback bind', { ...base, bind: 'loopback' }],
    ["the gateway's own HTTPS", { ...base, gatewayHttps: SERVED }],
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
  const lan = 'http://192.168.1.5:5252';

  it('takes the tailnet address ahead of the LAN one', () => {
    expect(pairingOrigin({ ...base, external: null, lan })).toBe('http://mymac.tailnet-name.ts.net:5252');
  });

  it('falls back to the LAN address when there is no tailnet one', () => {
    expect(pairingOrigin({ ...base, external: null, tailnetIp: null, lan })).toBe('http://192.168.1.5:5252');
  });

  it("keeps this gateway's port, so the code is redeemed where it was minted", () => {
    const devGateway = {
      ...base,
      here: { protocol: 'http:', hostname: '100.64.0.7', port: '5251' },
    };
    expect(pairingOrigin({ ...devGateway, external: null, lan: null })).toBe(
      'http://mymac.tailnet-name.ts.net:5251',
    );
  });

  it('points the DMG reader behind serve on 443 at the verified HTTPS origin', () => {
    const dmg = { ...base, bind: 'loopback', workspaceServeUrl: 'https://mymac.tailnet-name.ts.net/myws/' };
    expect(pairingOrigin({ ...dmg, external: null, lan })).toBe('https://mymac.tailnet-name.ts.net');
  });

  it('points the dev reader behind serve on 10000 at the verified HTTPS origin', () => {
    const dev = {
      ...base,
      here: { protocol: 'https:', hostname: 'localhost', port: '5251' },
      workspaceServeUrl: 'https://mymac.tailnet-name.ts.net:10000/dev/',
    };
    expect(pairingOrigin({ ...dev, external: null, lan })).toBe('https://mymac.tailnet-name.ts.net:10000');
  });

  it('gives an https reader with nothing verified no tailnet QR', () => {
    const tlsReader = { ...base, here: { protocol: 'https:', hostname: 'localhost', port: '5251' } };
    expect(pairingOrigin({ ...tlsReader, external: null, lan: null })).toBeNull();
    expect(pairingOrigin({ ...tlsReader, external: null, lan })).toBe('http://192.168.1.5:5252');
  });

  it("never hands out an https reader's own origin, which only this desktop may trust", () => {
    const lanTlsReader = { ...base, here: { protocol: 'https:', hostname: '192.168.1.5', port: '5251' } };
    expect(pairingOrigin({ ...lanTlsReader, external: null, lan: null })).toBeNull();
    expect(pairingOrigin({ ...lanTlsReader, external: null, lan })).toBe('http://192.168.1.5:5252');
  });

  it('turns every verified origin into an https QR, ahead of plain HTTP', () => {
    const verified: Array<Partial<TailnetInput>> = [
      { gatewayHttps: SERVED },
      { workspaceServeUrl: 'https://mymac.tailnet-name.ts.net/dev/' },
      { workspaceServeUrl: 'https://mymac.tailnet-name.ts.net:8443/dev/' },
    ];
    for (const proof of verified) {
      for (const here of [base.here, { protocol: 'https:', hostname: 'localhost', port: '5251' }]) {
        const origin = pairingOrigin({ ...base, ...proof, here, external: null, lan });
        expect(origin?.startsWith('https://mymac.tailnet-name.ts.net')).toBe(true);
      }
    }
  });

  it('answers nothing rather than an address a phone cannot reach', () => {
    expect(pairingOrigin({ ...base, external: null, tailnetIp: null, bind: 'loopback', lan: null })).toBeNull();
  });

  it('never answers with a loopback address', () => {
    const loopbackReader = { ...base, here: { protocol: 'http:', hostname: '127.0.0.1', port: '5252' } };
    for (const lanState of [null, lan]) {
      const origin = pairingOrigin({ ...loopbackReader, external: null, lan: lanState });
      expect(origin === null || !/localhost|127\./.test(origin)).toBe(true);
    }
  });

  it("points the QR at the gateway's own HTTPS origin whenever it serves one", () => {
    expect(pairingOrigin({ ...base, gatewayHttps: SERVED, external: null, lan })).toBe(SERVED);
    const plainReader = { ...base, here: { protocol: 'http:', hostname: '100.64.0.7', port: '5252' } };
    expect(pairingOrigin({ ...plainReader, gatewayHttps: SERVED, external: null, lan })).toBe(SERVED);
  });

  it('takes the external address ahead of the tailnet, since the user typed it', () => {
    const external = 'https://lucidos.example.com';
    expect(pairingOrigin({ ...base, external, lan })).toBe(external);
    expect(pairingOrigin({ ...base, gatewayHttps: SERVED, external, lan })).toBe(external);
    expect(pairingOrigin({ ...base, external, tailnetIp: null, bind: 'loopback', lan: null })).toBe(external);
  });

  it("takes the reader's own origin after the tailnet, when it is not loopback", () => {
    const vpnReader = { ...base, here: { protocol: 'http:', hostname: '10.8.0.2', port: '5252' } };
    expect(pairingOrigin({ ...vpnReader, external: null, tailnetIp: null, lan })).toBe('http://10.8.0.2:5252');
    expect(pairingOrigin({ ...vpnReader, external: null, lan })).toBe('http://mymac.tailnet-name.ts.net:5252');
  });
});
