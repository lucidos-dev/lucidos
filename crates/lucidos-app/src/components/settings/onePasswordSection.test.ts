import { describe, it, expect } from 'vitest';
import { describeVaultCheck } from './OnePasswordSection';

describe('describeVaultCheck', () => {
  it('names the vaults a working service account reaches', () => {
    expect(describeVaultCheck({ cli: '/usr/local/bin/op', vaults: ['Engineering', 'Shared'] }))
      .toBe('Reaches 2 vaults: Engineering, Shared.');
    expect(describeVaultCheck({ cli: '/usr/local/bin/op', vaults: ['Engineering'] }))
      .toBe('Reaches 1 vault: Engineering.');
  });

  it('says so when the token works but reaches nothing', () => {
    expect(describeVaultCheck({ cli: '/usr/local/bin/op', vaults: [] }))
      .toBe('/usr/local/bin/op works, but this service account reaches no vaults.');
  });
});
