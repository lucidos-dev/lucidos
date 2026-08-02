import { describe, it, expect } from 'vitest';
import { lanRowState, tailnetHttpUrl, directAccessRows } from './MobileAccessPage';

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

describe('directAccessRows', () => {
  it('never prints the same address as both Local network and Tailscale', () => {
    // The user's own configuration: the gateway pinned to the tailnet address.
    // lanRowState shows whatever specific address is bound, so derived
    // separately this rendered the tailnet URL twice, once mislabelled "Local
    // network / same Wi-Fi".
    const rows = directAccessRows('100.64.0.7', '192.168.1.20', '100.64.0.7', 5252);
    expect(rows.tailnetUrl).toBe('http://100.64.0.7:5252');
    expect(rows.lan).toEqual({ kind: 'disabled' });
  });

  it('shows both when the gateway is on all interfaces', () => {
    const rows = directAccessRows('all', '192.168.1.20', '100.64.0.7', 5252);
    expect(rows.lan).toEqual({ kind: 'url', url: 'http://192.168.1.20:5252' });
    expect(rows.tailnetUrl).toBe('http://100.64.0.7:5252');
  });

  it('shows neither under the loopback default', () => {
    const rows = directAccessRows('loopback', '192.168.1.20', '100.64.0.7', 5252);
    expect(rows.lan).toEqual({ kind: 'disabled' });
    expect(rows.tailnetUrl).toBeNull();
  });

  it('leaves a genuine LAN bind alone', () => {
    const rows = directAccessRows('192.168.1.20', '192.168.1.20', '100.64.0.7', 5252);
    expect(rows.lan).toEqual({ kind: 'url', url: 'http://192.168.1.20:5252' });
    expect(rows.tailnetUrl).toBeNull();
  });
});
