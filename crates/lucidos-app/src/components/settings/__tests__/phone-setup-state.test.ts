/**
 * The phone-facing half must read the device it is running on.
 *
 * The bug: that half was a constant. A phone reading the page over its own
 * tailnet, from an installed PWA, on cellular, was still shown "Install
 * Tailscale on this device" with a **Get Tailscale** button as the loudest
 * thing on screen, plus three setup steps it had already completed.
 *
 * `tailscaleRowState` has refused to make the equivalent mistake about the Mac
 * since it was written ("telling someone to install Tailscale while they are
 * demonstrably using it would be the worst answer available"). This file holds
 * the same line for the reading device, whose evidence is different: not a
 * `get_connect_info` probe, but the address it was served on.
 */
import { describe, it, expect } from 'vitest';
import { isTailnetHostname, phoneSetupState } from '../MobileAccessPage';

describe('isTailnetHostname', () => {
  it('accepts a MagicDNS name, which only resolves on the tailnet that owns it', () => {
    expect(isTailnetHostname('mymac.tailnet-name.ts.net')).toBe(true);
    expect(isTailnetHostname('MyMac.Tailnet-Name.TS.NET')).toBe(true);
  });

  it('rejects the addresses that prove nothing', () => {
    expect(isTailnetHostname('localhost')).toBe(false);
    expect(isTailnetHostname('192.168.1.42')).toBe(false);
    // A bare CGNAT address is deliberately not proof: an ISP can hand
    // 100.64/10 to a physical interface, and the interface check that settles
    // it on the Mac cannot be run against the phone from here.
    expect(isTailnetHostname('100.64.0.7')).toBe(false);
  });

  it('requires the dot, so a lookalike domain cannot pass', () => {
    expect(isTailnetHostname('ts.net')).toBe(false);
    expect(isTailnetHostname('nots.net')).toBe(false);
    expect(isTailnetHostname('mymac.ts.net.example.com')).toBe(false);
  });
});

describe('phoneSetupState', () => {
  it('offers the install only when nothing proves this device is on a tailnet', () => {
    expect(phoneSetupState('192.168.1.42', false)).toEqual({ kind: 'install' });
    // Installed to the home screen over the LAN: still no tailnet evidence, so
    // the offer stands. Being wrong this way costs a redundant suggestion; the
    // other way costs the user their trust in the page.
    expect(phoneSetupState('192.168.1.42', true)).toEqual({ kind: 'install' });
  });

  it('never offers the install to a device reading over its own tailnet', () => {
    expect(phoneSetupState('mymac.tailnet-name.ts.net', false).kind).not.toBe('install');
    expect(phoneSetupState('mymac.tailnet-name.ts.net', true).kind).not.toBe('install');
  });

  it('leaves one step for a tailnet browser tab', () => {
    expect(phoneSetupState('mymac.tailnet-name.ts.net', false)).toEqual({
      kind: 'add-to-home-screen',
      hostname: 'mymac.tailnet-name.ts.net',
    });
  });

  it('leaves nothing for an installed PWA on the tailnet', () => {
    // The exact state the reported screenshot was taken in.
    expect(phoneSetupState('mymac.tailnet-name.ts.net', true)).toEqual({
      kind: 'ready',
      hostname: 'mymac.tailnet-name.ts.net',
    });
  });
});
