/**
 * The sentence under the External address field. A save's probe verdict
 * reaches the user only here. So does the news that an empty field means
 * Tailscale.
 */
import { describe, it, expect } from 'vitest';
import { externalAddressNote } from './ExternalAddressSection';
import type { ExternalAddressInfo } from '../../api/client/control';

const TAILNET = 'http://mymac.tailnet-name.ts.net:5252';

const unset: ExternalAddressInfo = {
  external_address: null,
  source: 'unset',
  locked: false,
  env_var: 'LUCIDOS_EXTERNAL_ADDRESS',
  suggestions: [],
  probe: null,
};
const set: ExternalAddressInfo = { ...unset, external_address: 'https://lucidos.example.com', source: 'file' };

describe('externalAddressNote', () => {
  it('says the empty field means Tailscale', () => {
    expect(externalAddressNote(unset, TAILNET)).toMatch(/uses your Tailscale address/);
    expect(externalAddressNote(unset, null)).toMatch(/once this machine is on a tailnet/);
  });

  it('says a stored address leads the pairing QR', () => {
    expect(externalAddressNote(set, TAILNET)).toMatch(/ahead of Tailscale/);
  });

  it('names the variable when the environment locks it', () => {
    const locked = { ...set, source: 'env' as const, locked: true };
    expect(externalAddressNote(locked, TAILNET)).toMatch(/LUCIDOS_EXTERNAL_ADDRESS/);
  });

  it('leads with the verdict of the save just made', () => {
    expect(externalAddressNote({ ...set, probe: { verdict: 'verified' } }, TAILNET)).toMatch(
      /reaches this gateway/,
    );
    const unreachable = externalAddressNote(
      { ...set, probe: { verdict: 'unreachable', detail: 'timed out' } },
      TAILNET,
    );
    expect(unreachable).toMatch(/did not answer from this machine \(timed out\)/);
    expect(unreachable).toMatch(/test it from a phone/);
    expect(
      externalAddressNote({ ...set, probe: { verdict: 'unknown', detail: 'HTTP 401' } }, TAILNET),
    ).toMatch(/could not be checked \(HTTP 401\)/);
  });
});
