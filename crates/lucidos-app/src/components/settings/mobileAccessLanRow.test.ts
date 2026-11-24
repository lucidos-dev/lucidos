import { describe, it, expect } from 'vitest';
import {
  lanUrl,
  tailnetHttpUrl,
  lanUrlAvoidingTailnet,
  workspaceUrlAt,
  originAtHost,
  servingBind,
  tailnetServesThisReader,
  tailnetAddressIsServed,
  tailnetConnectRows,
} from './MobileAccessPage';

describe('lanUrl', () => {
  it('hides the URL under the loopback bind (the packaged default)', () => {
    expect(lanUrl('loopback', '192.168.1.20', 5252)).toBeNull();
    expect(lanUrl('', '192.168.1.20', 5252)).toBeNull();
    expect(lanUrl('  loopback  ', '192.168.1.20', 5252)).toBeNull();
  });

  it('shows the detected LAN URL when bound to all interfaces', () => {
    expect(lanUrl('all', '192.168.1.20', 5252)).toBe('http://192.168.1.20:5252');
  });

  it('reports no address when bound to all but no LAN IP was detected', () => {
    expect(lanUrl('all', null, 5252)).toBeNull();
  });

  it('shows the bound IP for a specific-IP bind, ignoring the detected LAN IP', () => {
    expect(lanUrl('100.64.0.7', '192.168.1.20', 5252)).toBe('http://100.64.0.7:5252');
  });
});

describe('tailnetHttpUrl', () => {
  it('stays silent when the gateway does not listen on the tailnet address', () => {
    expect(tailnetHttpUrl('loopback', '100.64.0.7', 5252)).toBeNull();
    expect(tailnetHttpUrl('', '100.64.0.7', 5252)).toBeNull();
    expect(tailnetHttpUrl('192.168.1.20', '100.64.0.7', 5252)).toBeNull();
  });

  it('shows the URL when the bind covers the tailnet address', () => {
    expect(tailnetHttpUrl('all', '100.64.0.7', 5252)).toBe('http://100.64.0.7:5252');
    expect(tailnetHttpUrl('100.64.0.7', '100.64.0.7', 5252)).toBe('http://100.64.0.7:5252');
    expect(tailnetHttpUrl('  100.64.0.7  ', '100.64.0.7', 5252)).toBe('http://100.64.0.7:5252');
  });

  it('has nothing to show without a tailnet address', () => {
    expect(tailnetHttpUrl('all', null, 5252)).toBeNull();
  });
});

describe('lanUrlAvoidingTailnet', () => {
  it('never prints the same address as both Local network and Tailscale', () => {
    expect(lanUrlAvoidingTailnet('100.64.0.7', '192.168.1.20', '100.64.0.7', 5252))
      .toBeNull();
  });

  it('returns the LAN URL when the gateway is on all interfaces', () => {
    expect(lanUrlAvoidingTailnet('all', '192.168.1.20', '100.64.0.7', 5252))
      .toBe('http://192.168.1.20:5252');
  });

  it('shows nothing under the loopback default', () => {
    expect(lanUrlAvoidingTailnet('loopback', '192.168.1.20', '100.64.0.7', 5252))
      .toBeNull();
  });

  it('leaves a genuine LAN bind alone', () => {
    expect(lanUrlAvoidingTailnet('192.168.1.20', '192.168.1.20', '100.64.0.7', 5252))
      .toBe('http://192.168.1.20:5252');
  });
});

describe('workspaceUrlAt', () => {
  it('lands on the workspace, not the gateway root', () => {
    expect(workspaceUrlAt('http://localhost:5252', '/dev/')).toBe('http://localhost:5252/dev/');
    expect(workspaceUrlAt('https://mymac.tailnet-name.ts.net', '/dev/'))
      .toBe('https://mymac.tailnet-name.ts.net/dev/');
  });

  it('adds no prefix on a direct engine port, which has none', () => {
    expect(workspaceUrlAt('http://localhost:5173', '/')).toBe('http://localhost:5173/');
  });

  it('never doubles the separator', () => {
    expect(workspaceUrlAt('http://localhost:5252/', '/dev/')).toBe('http://localhost:5252/dev/');
    expect(workspaceUrlAt('http://localhost:5252//', '/dev/')).toBe('http://localhost:5252/dev/');
  });
});

describe('originAtHost', () => {
  it('keeps the scheme, so a TLS gateway is not addressed over http', () => {
    expect(originAtHost({ protocol: 'https:', port: '5251' }, '100.64.0.7'))
      .toBe('https://100.64.0.7:5251');
    expect(originAtHost({ protocol: 'http:', port: '5252' }, '100.64.0.7'))
      .toBe('http://100.64.0.7:5252');
  });

  it('carries no port when the reader is on the default one', () => {
    expect(originAtHost({ protocol: 'https:', port: '' }, 'mymac.tailnet-name.ts.net'))
      .toBe('https://mymac.tailnet-name.ts.net');
  });
});

describe('servingBind', () => {
  const config = { engine_bind: '100.64.0.7', inherit: false, gateway_bind: 'loopback' };

  it('takes the gateway bind behind the gateway, whose origin this is', () => {
    expect(servingBind(config, true)).toBe('loopback');
    expect(servingBind({ ...config, inherit: true }, true)).toBe('loopback');
  });

  it('takes the engine bind on a direct engine port', () => {
    expect(servingBind(config, false)).toBe('100.64.0.7');
  });

  it('follows the gateway bind on a direct port while inherit is on', () => {
    expect(servingBind({ ...config, inherit: true }, false)).toBe('loopback');
  });
});

describe('tailnetServesThisReader', () => {
  it('believes the address it was served on over the stored bind', () => {
    expect(tailnetServesThisReader('loopback', '100.64.0.7', '100.64.0.7')).toBe(true);
    expect(tailnetServesThisReader('loopback', '100.64.0.7', 'mymac.tailnet-name.ts.net'))
      .toBe(true);
  });

  it('accepts a bind that covers the address', () => {
    expect(tailnetServesThisReader('all', '100.64.0.7', 'localhost')).toBe(true);
    expect(tailnetServesThisReader('100.64.0.7', '100.64.0.7', 'localhost')).toBe(true);
  });

  it('reports the loopback default as unserved, read from anywhere else', () => {
    expect(tailnetServesThisReader('loopback', '100.64.0.7', 'localhost')).toBe(false);
    expect(tailnetServesThisReader('', '100.64.0.7', '192.168.1.20')).toBe(false);
  });
});

describe('tailnetConnectRows', () => {
  const here = { protocol: 'http:', hostname: 'localhost', port: '5252' };
  const base = {
    scope: '/dev/',
    here,
    tailnetIp: '100.64.0.7',
    magicDnsName: 'mymac.tailnet-name.ts.net',
    workspaceServeUrl: null as string | null,
    gatewayHttps: null as string | null,
    bind: 'all',
  };

  it('prints the verified HTTPS URL verbatim, and nothing else', () => {
    const rows = tailnetConnectRows({
      ...base,
      workspaceServeUrl: 'https://mymac.tailnet-name.ts.net/dev/',
    });
    expect(rows).toHaveLength(1);
    expect(rows[0].url).toBe('https://mymac.tailnet-name.ts.net/dev/');
    expect(rows[0].hint).toContain('HTTPS');
  });

  it('publishes the HTTPS URL under the loopback default', () => {
    const rows = tailnetConnectRows({
      ...base,
      bind: 'loopback',
      workspaceServeUrl: 'https://mymac.tailnet-name.ts.net/dev/',
    });
    expect(rows.map((r) => r.url)).toEqual(['https://mymac.tailnet-name.ts.net/dev/']);
  });

  it('prefers the MagicDNS name for the plain-HTTP address', () => {
    const rows = tailnetConnectRows(base);
    expect(rows.map((r) => r.url)).toEqual(['http://mymac.tailnet-name.ts.net:5252/dev/']);
  });

  it('falls back to the address when MagicDNS is off', () => {
    const rows = tailnetConnectRows({ ...base, magicDnsName: null });
    expect(rows.map((r) => r.url)).toEqual(['http://100.64.0.7:5252/dev/']);
  });

  it('prints nothing when nothing is listening on the tailnet address', () => {
    expect(tailnetConnectRows({ ...base, bind: 'loopback' })).toEqual([]);
  });

  it('prints nothing when the machine is off a tailnet', () => {
    expect(tailnetConnectRows({ ...base, tailnetIp: null })).toEqual([]);
  });

  it('scopes to the workspace, and to the root on a direct port', () => {
    expect(tailnetConnectRows({ ...base, scope: '/' }).map((r) => r.url))
      .toEqual(['http://mymac.tailnet-name.ts.net:5252/']);
  });

  it('composes no HTTPS address that nothing verified', () => {
    const rows = tailnetConnectRows({
      ...base,
      here: { protocol: 'https:', hostname: 'localhost', port: '5251' },
    });
    expect(rows).toEqual([]);
  });

  it('describes a composed address as plain HTTP', () => {
    const rows = tailnetConnectRows(base);
    expect(rows[0].url.startsWith('http://')).toBe(true);
    expect(rows[0].hint).toContain('plain HTTP');
  });

  it('never contradicts its own URL', () => {
    const cases = [
      base,
      { ...base, here: { protocol: 'https:', hostname: 'localhost', port: '5251' } },
      { ...base, workspaceServeUrl: 'https://mymac.tailnet-name.ts.net/dev/' },
      { ...base, magicDnsName: null },
    ];
    for (const input of cases) {
      for (const row of tailnetConnectRows(input)) {
        const secure = row.url.startsWith('https://');
        expect(row.hint.includes('plain HTTP')).toBe(!secure);
      }
    }
  });
});

describe('tailnetAddressIsServed', () => {
  it('does NOT take a MagicDNS arrival as evidence', () => {
    expect(tailnetAddressIsServed('loopback', '100.64.0.7', 'mymac.tailnet-name.ts.net'))
      .toBe(false);
    expect(tailnetServesThisReader('loopback', '100.64.0.7', 'mymac.tailnet-name.ts.net'))
      .toBe(true);
  });

  it('still believes the address it was served on', () => {
    expect(tailnetAddressIsServed('loopback', '100.64.0.7', '100.64.0.7')).toBe(true);
  });

  it('still accepts a bind that covers the address', () => {
    expect(tailnetAddressIsServed('all', '100.64.0.7', 'localhost')).toBe(true);
    expect(tailnetAddressIsServed('100.64.0.7', '100.64.0.7', 'localhost')).toBe(true);
    expect(tailnetAddressIsServed('loopback', '100.64.0.7', 'localhost')).toBe(false);
  });

  it('is never wider than the row predicate', () => {
    const binds = ['loopback', 'all', '100.64.0.7', '192.168.1.20', ''];
    const hosts = ['localhost', '100.64.0.7', 'mymac.tailnet-name.ts.net', '192.168.1.20'];
    for (const bind of binds) {
      for (const host of hosts) {
        if (tailnetAddressIsServed(bind, '100.64.0.7', host)) {
          expect(tailnetServesThisReader(bind, '100.64.0.7', host)).toBe(true);
        }
      }
    }
  });
});
