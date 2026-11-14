import { describe, expect, it } from 'vitest';
import { pairingRequestDetail } from './PairingRequestsSection';

describe('pairingRequestDetail', () => {
  it('says what waits and how long it has, rounding up', () => {
    expect(pairingRequestDetail({ id: 'a', kind: 'device', label: 'New phone', expires_in_secs: 241 }))
      .toBe('A device, with a code made on the machine running Lucidos · 5 min left');
    expect(pairingRequestDetail({ id: 'b', kind: 'gateway', label: 'Lucidos on studio', expires_in_secs: 20 }))
      .toBe('Another gateway, with a secret made on the machine running Lucidos · 1 min left');
  });
});
