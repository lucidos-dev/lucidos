import { describe, it, expect } from 'vitest';
import {
  deviceSetupState,
  hostTailnetState,
  isTailnetHostname,
  tailnetHttpUrl,
  tailnetIsServed,
} from '../MobileAccessPage';
import { isLoopbackHostname } from '../../../utils/loopbackHostname';

const HOST_IP = '100.64.0.2';


describe('hostTailnetState', () => {
  it('reports the tailnet address the engine detected for itself', () => {
    expect(
      hostTailnetState({ status: 'loaded', data: { detected_tailscale_ip: HOST_IP } }, null),
    ).toEqual({ kind: 'on-tailnet', ip: HOST_IP });
  });

  it('reports no tailnet, with Tailscale\'s own reason, when the engine has no address', () => {
    const reason = 'Tailscale is installed, but not signed in.';
    expect(
      hostTailnetState({ status: 'loaded', data: { detected_tailscale_ip: null } }, reason),
    ).toEqual({ kind: 'no-tailnet', reason });
    expect(
      hostTailnetState({ status: 'loaded', data: { detected_tailscale_ip: null } }, null),
    ).toEqual({ kind: 'no-tailnet', reason: null });
  });

  it('claims nothing until the fetch has actually landed', () => {
    expect(hostTailnetState({ status: 'not-loaded' }, null)).toEqual({ kind: 'unknown' });
    expect(hostTailnetState({ status: 'loading' }, null)).toEqual({ kind: 'unknown' });
    expect(hostTailnetState({ status: 'failed', error: 'boom' }, null)).toEqual({
      kind: 'unknown',
    });
  });
});

describe('tailnetIsServed', () => {
  it('separates holding a tailnet address from listening on it', () => {
    expect(tailnetIsServed('loopback', HOST_IP)).toBe(false);
    expect(tailnetIsServed('', HOST_IP)).toBe(false);
    expect(tailnetIsServed('192.168.1.10', HOST_IP)).toBe(false);
    expect(tailnetIsServed('all', HOST_IP)).toBe(true);
    expect(tailnetIsServed(HOST_IP, HOST_IP)).toBe(true);
    expect(tailnetIsServed(`  ${HOST_IP}  `, HOST_IP)).toBe(true);
  });

  it('is the same rule that decides whether a URL gets printed', () => {
    for (const bind of ['loopback', '', 'all', HOST_IP, '192.168.1.10']) {
      expect(tailnetIsServed(bind, HOST_IP)).toBe(tailnetHttpUrl(bind, HOST_IP, 5252) !== null);
    }
  });
});


describe('isTailnetHostname', () => {
  it('accepts a MagicDNS name, which only resolves on the tailnet that owns it', () => {
    expect(isTailnetHostname('mymac.tailnet-name.ts.net')).toBe(true);
    expect(isTailnetHostname('MyMac.Tailnet-Name.TS.NET')).toBe(true);
  });

  it('rejects the addresses that prove nothing on their own', () => {
    expect(isTailnetHostname('localhost')).toBe(false);
    expect(isTailnetHostname('192.168.1.42')).toBe(false);
    expect(isTailnetHostname('100.64.0.7')).toBe(false);
  });

  it('requires the dot, so a lookalike domain cannot pass', () => {
    expect(isTailnetHostname('ts.net')).toBe(false);
    expect(isTailnetHostname('nots.net')).toBe(false);
    expect(isTailnetHostname('mymac.ts.net.example.com')).toBe(false);
  });
});

describe('isLoopbackHostname', () => {
  it('recognises every form of "this machine"', () => {
    expect(isLoopbackHostname('localhost')).toBe(true);
    expect(isLoopbackHostname('LOCALHOST')).toBe(true);
    expect(isLoopbackHostname('127.0.0.1')).toBe(true);
    expect(isLoopbackHostname('127.0.0.2')).toBe(true);
    expect(isLoopbackHostname('::1')).toBe(true);
    expect(isLoopbackHostname('[::1]')).toBe(true);
    expect(isLoopbackHostname('tauri.localhost')).toBe(true);
  });

  it('does not mistake a routable address for loopback', () => {
    expect(isLoopbackHostname('192.168.1.42')).toBe(false);
    expect(isLoopbackHostname('100.64.0.7')).toBe(false);
    expect(isLoopbackHostname('mymac.tailnet-name.ts.net')).toBe(false);
    expect(isLoopbackHostname('localhost.example.com')).toBe(false);
    expect(isLoopbackHostname('notlocalhost')).toBe(false);
    expect(isLoopbackHostname('128.0.0.1')).toBe(false);
    expect(isLoopbackHostname('27.0.0.1')).toBe(false);
  });
});

const MAGIC_DNS = 'mymac.tailnet-name.ts.net';

function reader(over: Partial<Parameters<typeof deviceSetupState>[0]> = {}) {
  return deviceSetupState({
    hostname: MAGIC_DNS,
    standalone: false,
    secureContext: true,
    hostTailnetIp: HOST_IP,
    hostMagicDnsName: MAGIC_DNS,
    ...over,
  });
}

describe('deviceSetupState', () => {
  it('defers to concern 1 when the reader IS the machine', () => {
    expect(reader({ hostname: 'localhost' })).toEqual({ kind: 'same-machine' });
    expect(reader({ hostname: '127.0.0.1', standalone: true, hostTailnetIp: null })).toEqual({
      kind: 'same-machine',
    });
    expect(reader({ hostname: 'localhost', hostTailnetIp: null })).toEqual({
      kind: 'same-machine',
    });
  });

  it('accepts the engine-reported tailnet address as proof', () => {
    expect(reader({ hostname: HOST_IP }).kind).not.toBe('join-tailnet');
    expect(reader({ hostname: HOST_IP, standalone: true })).toEqual({
      kind: 'ready',
      hostname: HOST_IP,
    });
  });

  it('calls an installed app ready, even one opened over plain HTTP', () => {
    expect(reader({ hostname: HOST_IP, standalone: true, secureContext: false })).toEqual({
      kind: 'ready',
      hostname: HOST_IP,
    });
  });

  it('accepts the host\'s reported MagicDNS name whatever its suffix', () => {
    const headscale = 'mymac.example-tailnet.test';
    expect(reader({ hostname: headscale, hostMagicDnsName: headscale })).toEqual({
      kind: 'install-app',
      hostname: headscale,
    });
    expect(
      reader({ hostname: 'other.example-tailnet.test', hostMagicDnsName: headscale }),
    ).toEqual({ kind: 'join-tailnet' });
  });

  it('never treats the CGNAT range itself as proof', () => {
    expect(reader({ hostname: '100.64.0.7' })).toEqual({ kind: 'join-tailnet' });
    expect(reader({ hostname: '100.64.0.7', hostTailnetIp: null })).toEqual({
      kind: 'join-tailnet',
    });
    expect(reader({ hostname: HOST_IP, hostTailnetIp: null })).toEqual({ kind: 'join-tailnet' });
  });

  it('offers the install only when nothing proves this device is on a tailnet', () => {
    expect(reader({ hostname: '192.168.1.42' })).toEqual({ kind: 'join-tailnet' });
    expect(reader({ hostname: '192.168.1.42', standalone: true })).toEqual({
      kind: 'join-tailnet',
    });
  });

  it('never offers the install to a device reading over its own tailnet', () => {
    expect(reader({ hostTailnetIp: null }).kind).not.toBe('join-tailnet');
    expect(reader({ standalone: true, hostTailnetIp: null }).kind).not.toBe('join-tailnet');
  });

  it('leaves one step for a tailnet browser tab on a secure origin', () => {
    expect(reader()).toEqual({ kind: 'install-app', hostname: MAGIC_DNS });
  });

  it('does not ask a plain-HTTP reader to install what its browser will refuse', () => {
    expect(reader({ hostname: HOST_IP, secureContext: false })).toEqual({
      kind: 'needs-https',
      hostname: HOST_IP,
    });
    expect(reader({ secureContext: false })).toEqual({
      kind: 'needs-https',
      hostname: MAGIC_DNS,
    });
    expect(reader({ secureContext: false }).kind).not.toBe('join-tailnet');
  });

  it('leaves nothing for an installed PWA on the tailnet', () => {
    expect(reader({ standalone: true })).toEqual({ kind: 'ready', hostname: MAGIC_DNS });
  });

  it('survives a host fetch that never landed, rather than blanking', () => {
    expect(reader({ hostTailnetIp: null }).kind).toBe('install-app');
    expect(reader({ hostname: '192.168.1.42', hostTailnetIp: null })).toEqual({
      kind: 'join-tailnet',
    });
  });
});
