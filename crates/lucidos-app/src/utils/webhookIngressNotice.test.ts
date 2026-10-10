/**
 * The ingress wording table, read by the app bar and by every enabled row on the
 * Webhooks page.
 *
 * One property carries the module: the ADDRESS FAMILY is in every sentence. The
 * outage this feature exists to catch was IPv4 alone, with IPv6 answering
 * correctly all night. Wording that flattened that would repeat the failure at
 * the last layer, after the engine had gone to the trouble of measuring it.
 */
import { describe, it, expect } from 'vitest';

import {
  ingressFamiliesPhrase,
  webhookIngressNotice,
  webhookIngressRowLine,
} from './webhookIngressNotice';
import type { WebhookIngressOutage } from '../api/client';

function outage(over: Partial<WebhookIngressOutage> = {}): WebhookIngressOutage {
  return {
    webhook_name: 'github-ci',
    host: 'node.tailnet.ts.net',
    port: 8443,
    families: ['ipv4'],
    addresses: [],
    down_since: '2026-08-26T22:10:00Z',
    down_secs: 28_800,
    ...over,
  };
}

describe('ingressFamiliesPhrase', () => {
  it('names one family, and both when both are down', () => {
    expect(ingressFamiliesPhrase(['ipv4'])).toBe('IPv4');
    expect(ingressFamiliesPhrase(['ipv6'])).toBe('IPv6');
    expect(ingressFamiliesPhrase(['ipv4', 'ipv6'])).toBe('IPv4 and IPv6');
  });

  it('reads the same however the engine ordered the list', () => {
    // Two surfaces render one outage. If arrival order leaked into the words,
    // the bar and the row could describe the same failure differently.
    expect(ingressFamiliesPhrase(['ipv6', 'ipv4'])).toBe('IPv4 and IPv6');
  });

  it('falls back to the path rather than claiming a family it was not told', () => {
    // Unreachable today: the engine declares an outage only with a family down.
    // A fallback that guessed "every address" would be the wrong kind of wrong.
    expect(ingressFamiliesPhrase([])).toBe('the public path');
  });
});

describe('webhookIngressNotice', () => {
  it('names the family in the title, so one family down never reads as all', () => {
    // The bar is the whole point of the feature. A title saying only "webhook
    // deliveries are not arriving" would have read as false during the real
    // outage. Anyone could see IPv6 working.
    expect(webhookIngressNotice(outage()).title)
      .toBe('Webhook deliveries over IPv4 are not arriving');
    expect(webhookIngressNotice(outage({ families: ['ipv4', 'ipv6'] })).title)
      .toBe('Webhook deliveries over IPv4 and IPv6 are not arriving');
  });

  it('states where it knocked and for how long', () => {
    const detail = webhookIngressNotice(outage()).detail;
    expect(detail).toContain('node.tailnet.ts.net:8443');
    expect(detail).toContain('for 8 hours');
  });

  it('measures the outage from `down_secs`, never from the client clock', () => {
    // The engine sends the span the database measured, beside the instant. A
    // client with a skewed clock therefore cannot report its own skew as outage
    // time (ADR 0053). Moving the instant alone must change nothing.
    const shifted = outage({ down_since: '1999-01-01T00:00:00Z' });
    expect(webhookIngressNotice(shifted).detail)
      .toBe(webhookIngressNotice(outage()).detail);
  });

  it('claims a lost reply, never a lost delivery', () => {
    // The probe knows what it met. It does not know whether a sender tried, so
    // counting deliveries as lost would be an invention.
    const detail = webhookIngressNotice(outage()).detail;
    expect(detail).toContain('gets no reply');
    for (const invented of ['lost', 'dropped', 'missed']) {
      expect(detail, `"${invented}" asserts something no probe can see`)
        .not.toContain(invented);
    }
  });

  it('promises the recheck the probe actually runs', () => {
    expect(webhookIngressNotice(outage()).detail).toContain('Rechecked every 15 minutes.');
  });
});

describe('webhookIngressRowLine', () => {
  it('states the per-family verdict as one clause', () => {
    expect(webhookIngressRowLine(outage())).toBe('Not reachable over IPv4 for 8 hours');
    expect(webhookIngressRowLine(outage({ families: ['ipv4', 'ipv6'], down_secs: 90 })))
      .toBe('Not reachable over IPv4 and IPv6 for 1 minute');
  });

  it('names no webhook, because the ingress sits in front of all of them', () => {
    // Drawn on every enabled row. The probe targets one hook, and what failed
    // is the path they share, so naming the target would be wrong about the
    // rest.
    const line = webhookIngressRowLine(outage());
    expect(line).not.toContain('node.tailnet.ts.net');
    expect(line).not.toContain('8443');
  });
});
