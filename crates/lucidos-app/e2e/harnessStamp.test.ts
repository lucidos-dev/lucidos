import { describe, expect, it } from 'vitest';
import { HEADER_GATEWAY_STAMP, signDeviceStamp, withDeviceStamp } from './harnessStamp';

describe('harness device stamps', () => {
  // The vector `a_fixed_key_signs_the_pinned_vector` pins in
  // crates/lucidos-gateway-stamp. Both must change together.
  it('signs the vector the Rust stamp crate pins', () => {
    expect(signDeviceStamp('ab'.repeat(32), 'dev', 'd1', 1000)).toBe(
      'v1.1000.fa40792259c9d99bff467c50b8b976de3da76e0b92c9a69360d5917c49bfa3c4',
    );
  });

  it('leaves headers naming no device untouched', () => {
    const headers = { accept: 'application/json' };
    expect(withDeviceStamp(headers)).toBe(headers);
  });

  it('never replaces a stamp a request already carries', () => {
    const headers = { 'x-lucidos-device-id': 'd1', [HEADER_GATEWAY_STAMP]: 'v1.1.00' };
    expect(withDeviceStamp(headers)).toBe(headers);
  });
});
