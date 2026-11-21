// @vitest-environment jsdom
/**
 * The sentence under the External address field. A save's probe verdict
 * reaches the user only here. So does the news that an empty field means
 * Tailscale.
 */
import { describe, it, expect } from 'vitest';
import { h, render } from 'preact';
import { ExternalAddressSection, externalAddressNote, publicListenerNote } from './ExternalAddressSection';
import type { ExternalAddressInfo } from '../../api/client/control';

const TAILNET = 'http://mymac.tailnet-name.ts.net:5252';

const unset: ExternalAddressInfo = {
  external_address: null,
  source: 'unset',
  locked: false,
  env_var: 'LUCIDOS_EXTERNAL_ADDRESS',
  suggestions: [],
  probe: null,
  public_port: 5282,
};
const set: ExternalAddressInfo = { ...unset, external_address: 'https://lucidos.example.com', source: 'file' };

describe('ExternalAddressSection', () => {
  it.each([true, false])('shows the gateway scope when gateway controls are available: %s', (available) => {
    const host = document.createElement('div');
    try {
      render(h(ExternalAddressSection, {
        available,
        info: { status: 'loaded', data: unset },
        tailnetDefault: TAILNET,
        onSaved: () => {},
      }), host);
      expect(host.querySelector('.settings-section-desc')?.textContent?.trim()).toBe(
        'This address belongs to the gateway and applies to every workspace on it.',
      );
      expect(host.textContent?.includes('lucidos external-address')).toBe(!available);
    } finally {
      render(null, host);
    }
  });
});

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

describe('publicListenerNote', () => {
  it('names the loopback port a tunnel should point at', () => {
    expect(publicListenerNote(unset)).toContain('http://127.0.0.1:5282');
  });

  it('says nothing when the public listener is off', () => {
    expect(publicListenerNote({ ...unset, public_port: null })).toBeNull();
  });
});
