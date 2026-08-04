/**
 * Mobile Access answers TWO independent questions, and must not muddle them:
 *
 *   1. Is the machine running the engine on a tailnet?
 *   2. Has the device reading this page joined that tailnet?
 *
 * This file pins both derivations, in that order, deliberately in separate
 * `describe` blocks that share no inputs: the host's state is read from the
 * engine over HTTP, the device's from the address it was served on.
 *
 * The bug that produced the split. The page used to answer only one of them,
 * chosen by platform (`isTauri() && enginePackaged`), and the device half's sole
 * proof of tailnet membership was a `.ts.net` hostname. So a Mac whose gateway
 * is bound to its own tailnet address, reading `http://100.x.y.z:5173` in
 * desktop Chrome, was shown "Install Tailscale on this device" with a **Get
 * Tailscale** button, under the heading "Getting Lucidos onto this phone" -- to
 * a working Tailscale user, on a machine that is not a phone, over a connection
 * that only exists because Tailscale is working.
 *
 * `tailscaleRowState` has refused to make the equivalent mistake about the host
 * since it was written ("telling someone to install Tailscale while they are
 * demonstrably using it would be the worst answer available"). This file holds
 * the same line for the reading device, whose evidence is different: not a
 * `get_connect_info` probe, but the address it was served on, checked against
 * the tailnet address the engine reports for itself.
 */
import { describe, it, expect } from 'vitest';
import {
  deviceSetupState,
  hostTailnetState,
  isLoopbackHostname,
  isTailnetHostname,
  tailnetHttpUrl,
  tailnetIsServed,
} from '../MobileAccessPage';

/** The engine's reported tailnet address for the machine it runs on. */
const HOST_IP = '100.101.71.7';

// --- Concern 1: the machine running the engine -----------------------------

describe('hostTailnetState', () => {
  it('reports the tailnet address the engine detected for itself', () => {
    expect(
      hostTailnetState({ status: 'loaded', data: { detected_tailscale_ip: HOST_IP } }),
    ).toEqual({ kind: 'on-tailnet', ip: HOST_IP });
  });

  it('reports no tailnet when the engine detected no address', () => {
    expect(
      hostTailnetState({ status: 'loaded', data: { detected_tailscale_ip: null } }),
    ).toEqual({ kind: 'no-tailnet' });
  });

  it('claims nothing until the fetch has actually landed', () => {
    // Three not-loaded states, one answer. Rendering "not on a tailnet" while
    // the request is in flight would be a claim we cannot support yet, and the
    // failed case must stay distinct from the empty one (Loadable contract).
    expect(hostTailnetState({ status: 'not-loaded' })).toEqual({ kind: 'unknown' });
    expect(hostTailnetState({ status: 'loading' })).toEqual({ kind: 'unknown' });
    expect(hostTailnetState({ status: 'failed', error: 'boom' })).toEqual({ kind: 'unknown' });
  });
});

describe('tailnetIsServed', () => {
  it('separates holding a tailnet address from listening on it', () => {
    // The distinction the host row's prose depends on. A machine can be
    // perfectly healthy on its tailnet while nothing off it can connect,
    // which is the packaged default.
    expect(tailnetIsServed('loopback', HOST_IP)).toBe(false);
    expect(tailnetIsServed('', HOST_IP)).toBe(false);
    // Bound to some OTHER specific address: serves that one, not this one.
    expect(tailnetIsServed('192.168.1.10', HOST_IP)).toBe(false);
    expect(tailnetIsServed('all', HOST_IP)).toBe(true);
    expect(tailnetIsServed(HOST_IP, HOST_IP)).toBe(true);
    expect(tailnetIsServed(`  ${HOST_IP}  `, HOST_IP)).toBe(true);
  });

  it('is the same rule that decides whether a URL gets printed', () => {
    // Extracted from `tailnetHttpUrl` precisely so the prose and the URL cannot
    // drift apart. If these two ever disagree, one of the surfaces is lying.
    for (const bind of ['loopback', '', 'all', HOST_IP, '192.168.1.10']) {
      expect(tailnetIsServed(bind, HOST_IP)).toBe(tailnetHttpUrl(bind, HOST_IP, 5252) !== null);
    }
  });
});

// --- Concern 2: the device reading the page --------------------------------

describe('isTailnetHostname', () => {
  it('accepts a MagicDNS name, which only resolves on the tailnet that owns it', () => {
    expect(isTailnetHostname('mymac.tailnet-name.ts.net')).toBe(true);
    expect(isTailnetHostname('MyMac.Tailnet-Name.TS.NET')).toBe(true);
  });

  it('rejects the addresses that prove nothing on their own', () => {
    expect(isTailnetHostname('localhost')).toBe(false);
    expect(isTailnetHostname('192.168.1.42')).toBe(false);
    // A bare CGNAT address is still not proof BY ITSELF: an ISP can hand
    // 100.64/10 to a physical interface. What settles it is a match against the
    // interface-checked address the engine reports, which is `deviceSetupState`'s
    // job, not this predicate's.
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
    // The whole 127/8 is loopback, not just .0.1.
    expect(isLoopbackHostname('127.0.0.2')).toBe(true);
    // `location.hostname` keeps the brackets on an IPv6 host.
    expect(isLoopbackHostname('::1')).toBe(true);
    expect(isLoopbackHostname('[::1]')).toBe(true);
    // RFC 6761 reserves the whole .localhost tree; Tauri serves the bundled
    // assets from `tauri.localhost` on non-macOS.
    expect(isLoopbackHostname('tauri.localhost')).toBe(true);
  });

  it('does not mistake a routable address for loopback', () => {
    expect(isLoopbackHostname('192.168.1.42')).toBe(false);
    expect(isLoopbackHostname('100.64.0.7')).toBe(false);
    expect(isLoopbackHostname('mymac.tailnet-name.ts.net')).toBe(false);
    // A lookalike that merely contains the word.
    expect(isLoopbackHostname('localhost.example.com')).toBe(false);
    expect(isLoopbackHostname('notlocalhost')).toBe(false);
    // Outside 127/8.
    expect(isLoopbackHostname('128.0.0.1')).toBe(false);
    expect(isLoopbackHostname('27.0.0.1')).toBe(false);
  });
});

const MAGIC_DNS = 'mymac.tailnet-name.ts.net';

/** A reader on an https MagicDNS address, which is the fully-working setup.
 *  Each test overrides only the field it is about. */
function reader(over: Partial<Parameters<typeof deviceSetupState>[0]> = {}) {
  return deviceSetupState({
    hostname: MAGIC_DNS,
    standalone: false,
    secureContext: true,
    hostTailnetIp: HOST_IP,
    ...over,
  });
}

describe('deviceSetupState', () => {
  it('defers to concern 1 when the reader IS the machine', () => {
    // Reading over loopback means there is no tailnet to join HERE: this device
    // is the engine host, so its tailnet state is the host's. Offering a client
    // install would be answering the wrong question.
    expect(reader({ hostname: 'localhost' })).toEqual({ kind: 'same-machine' });
    expect(reader({ hostname: '127.0.0.1', standalone: true, hostTailnetIp: null })).toEqual({
      kind: 'same-machine',
    });
    // True even when the host has no tailnet address at all: what to do about
    // that belongs to the host section, which is where the offer now lives.
    expect(reader({ hostname: 'localhost', hostTailnetIp: null })).toEqual({
      kind: 'same-machine',
    });
  });

  it('accepts the engine-reported tailnet address as proof', () => {
    // THE REPORTED BUG. The gateway is bound to the machine's tailnet address,
    // so the page is served at it. Reaching an address that the engine read off
    // a Tailscale interface means this device is on that tailnet: it is how the
    // request arrived. Nothing here infers membership from the range.
    expect(reader({ hostname: HOST_IP }).kind).not.toBe('join-tailnet');
    expect(reader({ hostname: HOST_IP, standalone: true })).toEqual({
      kind: 'ready',
      hostname: HOST_IP,
    });
  });

  it('never treats the CGNAT range itself as proof', () => {
    // The negative that keeps the proof sound. A 100.x host that is NOT the
    // address the engine reported proves nothing: that range is real CGNAT
    // space an ISP can hand to a physical interface, and the interface check
    // that settles it for the host cannot be run against this device from here.
    expect(reader({ hostname: '100.64.0.7' })).toEqual({ kind: 'join-tailnet' });
    // And with nothing reported, no 100.x host can pass at all.
    expect(reader({ hostname: '100.64.0.7', hostTailnetIp: null })).toEqual({
      kind: 'join-tailnet',
    });
    expect(reader({ hostname: HOST_IP, hostTailnetIp: null })).toEqual({ kind: 'join-tailnet' });
  });

  it('offers the install only when nothing proves this device is on a tailnet', () => {
    expect(reader({ hostname: '192.168.1.42' })).toEqual({ kind: 'join-tailnet' });
    // Installed to the home screen over the LAN: still no tailnet evidence, so
    // the offer stands. Being wrong this way costs a redundant suggestion; the
    // other way costs the user their trust in the page.
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
    // THE REPORTED SETUP, one step on. Tailscale is working and the address is
    // right, but `http://100.x` is not a secure origin, so the browser offers
    // no install control and registers no service worker: naming one would send
    // the user hunting for a button that does not exist. What is actually left
    // is `tailscale serve` on the machine, which is concern 1's business.
    expect(reader({ hostname: HOST_IP, secureContext: false })).toEqual({
      kind: 'needs-https',
      hostname: HOST_IP,
    });
    // Plain HTTP to a MagicDNS name is the same story: the name is not what
    // makes an origin secure.
    expect(reader({ secureContext: false })).toEqual({
      kind: 'needs-https',
      hostname: MAGIC_DNS,
    });
    // Still not an install offer. Tailscale is not the thing missing here.
    expect(reader({ secureContext: false }).kind).not.toBe('join-tailnet');
  });

  it('leaves nothing for an installed PWA on the tailnet', () => {
    // The exact state the first reported screenshot was taken in.
    expect(reader({ standalone: true })).toEqual({ kind: 'ready', hostname: MAGIC_DNS });
  });

  it('survives a host fetch that never landed, rather than blanking', () => {
    // A failed `network-config` costs one of the three proofs, not the whole
    // derivation: the MagicDNS proof needs nothing from the engine.
    expect(reader({ hostTailnetIp: null }).kind).toBe('install-app');
    expect(reader({ hostname: '192.168.1.42', hostTailnetIp: null })).toEqual({
      kind: 'join-tailnet',
    });
  });
});
