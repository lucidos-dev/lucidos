import { describe, it, expect } from 'vitest';
import {
  lanRowState,
  tailnetHttpUrl,
  lanRowAvoidingTailnet,
  workspaceUrlAt,
  originAtHost,
  servingBind,
  tailnetServesThisReader,
  tailnetAddressIsServed,
  tailnetConnectRows,
} from './MobileAccessPage';

describe('lanRowState', () => {
  it('hides the URL under the loopback bind (the packaged default)', () => {
    // A loopback-bound gateway cannot serve a LAN address — advertising the
    // URL anyway was the original Mobile Access bug (a dead, copyable URL).
    expect(lanRowState('loopback', '192.168.1.20', 5252)).toEqual({ kind: 'disabled' });
    expect(lanRowState('', '192.168.1.20', 5252)).toEqual({ kind: 'disabled' });
    expect(lanRowState('  loopback  ', '192.168.1.20', 5252)).toEqual({ kind: 'disabled' });
  });

  it('shows the detected LAN URL when bound to all interfaces', () => {
    expect(lanRowState('all', '192.168.1.20', 5252)).toEqual({
      kind: 'url',
      url: 'http://192.168.1.20:5252',
    });
  });

  it('reports no address when bound to all but no LAN IP was detected', () => {
    expect(lanRowState('all', null, 5252)).toEqual({ kind: 'none' });
  });

  it('shows the bound IP for a specific-IP bind, ignoring the detected LAN IP', () => {
    // A specific-IP bind serves exactly that IP (plus loopback); the detected
    // en0 address may be a different, unreachable interface.
    expect(lanRowState('100.64.0.7', '192.168.1.20', 5252)).toEqual({
      kind: 'url',
      url: 'http://100.64.0.7:5252',
    });
  });
});

describe('tailnetHttpUrl', () => {
  it('stays silent when the gateway does not listen on the tailnet address', () => {
    // Being on a tailnet says nothing about the gateway's bind. Loopback is the
    // packaged DEFAULT, so this is the common case, and printing the URL anyway
    // would repeat the dead-URL bug lanRowState exists to prevent.
    expect(tailnetHttpUrl('loopback', '100.64.0.7', 5252)).toBeNull();
    expect(tailnetHttpUrl('', '100.64.0.7', 5252)).toBeNull();
    // Bound to some OTHER specific address: that one is served, not this one.
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

describe('lanRowAvoidingTailnet', () => {
  it('never prints the same address as both Local network and Tailscale', () => {
    // The user's own configuration: the gateway pinned to the tailnet address.
    // lanRowState shows whatever specific address is bound, so derived
    // separately this rendered the tailnet URL twice, once mislabelled "Local
    // network / same Wi-Fi".
    expect(lanRowAvoidingTailnet('100.64.0.7', '192.168.1.20', '100.64.0.7', 5252))
      .toEqual({ kind: 'disabled' });
  });

  it('shows the LAN row when the gateway is on all interfaces', () => {
    expect(lanRowAvoidingTailnet('all', '192.168.1.20', '100.64.0.7', 5252))
      .toEqual({ kind: 'url', url: 'http://192.168.1.20:5252' });
  });

  it('shows nothing under the loopback default', () => {
    expect(lanRowAvoidingTailnet('loopback', '192.168.1.20', '100.64.0.7', 5252))
      .toEqual({ kind: 'disabled' });
  });

  it('leaves a genuine LAN bind alone', () => {
    expect(lanRowAvoidingTailnet('192.168.1.20', '192.168.1.20', '100.64.0.7', 5252))
      .toEqual({ kind: 'url', url: 'http://192.168.1.20:5252' });
  });
});

describe('workspaceUrlAt', () => {
  it('lands on the workspace, not the gateway root', () => {
    // The root 307-redirects to the sole workspace or to the picker, so on a
    // multi-workspace install it is the wrong address to hand out.
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
    // The dev gateway speaks TLS on 5251. A composed `http://` would be dead.
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
    // The bug this replaces: reading `gateway_bind` here reports a direct-port
    // page's reachability from a bind that governs a different process.
    expect(servingBind(config, false)).toBe('100.64.0.7');
  });

  it('follows the gateway bind on a direct port while inherit is on', () => {
    expect(servingBind({ ...config, inherit: true }, false)).toBe('loopback');
  });
});

describe('tailnetServesThisReader', () => {
  it('believes the address it was served on over the stored bind', () => {
    // A bind change takes effect only on restart, so the config can disagree
    // with the live socket. Arriving here IS the proof.
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
    bind: 'all',
  };

  it('prints the verified HTTPS URL verbatim, and nothing else', () => {
    // The engine sets it only after a request to it came back from that same
    // engine, so rebuilding it here would let the two disagree.
    const rows = tailnetConnectRows({
      ...base,
      workspaceServeUrl: 'https://mymac.tailnet-name.ts.net/dev/',
    });
    expect(rows).toHaveLength(1);
    expect(rows[0].url).toBe('https://mymac.tailnet-name.ts.net/dev/');
    expect(rows[0].hint).toContain('HTTPS');
  });

  it('publishes the HTTPS URL under the loopback default', () => {
    // `serve` proxies from the machine to `127.0.0.1`, so it is unaffected by
    // the bind that kills every other row here. Bind-gating it would suppress
    // the best address on the page over an unrelated setting.
    const rows = tailnetConnectRows({
      ...base,
      bind: 'loopback',
      workspaceServeUrl: 'https://mymac.tailnet-name.ts.net/dev/',
    });
    expect(rows.map((r) => r.url)).toEqual(['https://mymac.tailnet-name.ts.net/dev/']);
  });

  it('prefers the MagicDNS name for the plain-HTTP address', () => {
    // It resolves to the same tailnet address for any device on the tailnet,
    // and unlike `100.64.0.7` a person can retype it.
    const rows = tailnetConnectRows(base);
    expect(rows.map((r) => r.url)).toEqual(['http://mymac.tailnet-name.ts.net:5252/dev/']);
  });

  it('falls back to the address when MagicDNS is off', () => {
    const rows = tailnetConnectRows({ ...base, magicDnsName: null });
    expect(rows.map((r) => r.url)).toEqual(['http://100.64.0.7:5252/dev/']);
  });

  it('prints nothing when nothing is listening on the tailnet address', () => {
    // The original bug this page was built around: a dead, copyable URL.
    expect(tailnetConnectRows({ ...base, bind: 'loopback' })).toEqual([]);
  });

  it('prints nothing when the machine is off a tailnet', () => {
    expect(tailnetConnectRows({ ...base, tailnetIp: null })).toEqual([]);
  });

  it('scopes to the workspace, and to the root on a direct port', () => {
    expect(tailnetConnectRows({ ...base, scope: '/' }).map((r) => r.url))
      .toEqual(['http://mymac.tailnet-name.ts.net:5252/']);
  });

  it('keeps the reader on TLS when that is how they got here', () => {
    const rows = tailnetConnectRows({
      ...base,
      here: { protocol: 'https:', hostname: 'localhost', port: '5251' },
    });
    expect(rows.map((r) => r.url)).toEqual(['https://mymac.tailnet-name.ts.net:5251/dev/']);
  });

  it('describes the address by its own scheme, not by which branch built it', () => {
    // The row keeps the reader's scheme, so this branch can produce an HTTPS
    // URL. A hardcoded "plain HTTP" hint then denied push at an origin that
    // offers it, under an `https://` address saying otherwise. Two ordinary
    // setups reach it: the dev gateway's own TLS cert, and a serve-fronted
    // reader whose verification timed out.
    const overTls = tailnetConnectRows({
      ...base,
      here: { protocol: 'https:', hostname: 'localhost', port: '5251' },
    });
    expect(overTls[0].hint).toContain('HTTPS');
    expect(overTls[0].hint).not.toContain('plain HTTP');

    const overPlain = tailnetConnectRows(base);
    expect(overPlain[0].url.startsWith('http://')).toBe(true);
    expect(overPlain[0].hint).toContain('plain HTTP');
  });

  it('never contradicts its own URL', () => {
    // The invariant behind the case above, over every shape this returns.
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
    // The narrower question, and the regression this split exists for. Arriving
    // over `serve` proves 443 is fronted, and nothing about the gateway port on
    // the tailnet address. Borrowing the row's predicate told a phone on the
    // recommended setup that the plain-HTTP address was live.
    expect(tailnetAddressIsServed('loopback', '100.64.0.7', 'mymac.tailnet-name.ts.net'))
      .toBe(false);
    // The wider question answers the same reader yes, because the URL it prints
    // is the address that reader is already on.
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
