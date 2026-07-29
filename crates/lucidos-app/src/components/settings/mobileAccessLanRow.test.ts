import { describe, it, expect } from 'vitest';
import { lanRowState } from './MobileAccessPage';

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
