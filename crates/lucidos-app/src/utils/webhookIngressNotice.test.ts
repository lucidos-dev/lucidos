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
    expect(ingressFamiliesPhrase(['ipv6', 'ipv4'])).toBe('IPv4 and IPv6');
  });

  it('falls back to the path rather than claiming a family it was not told', () => {
    expect(ingressFamiliesPhrase([])).toBe('the public path');
  });
});

describe('webhookIngressNotice', () => {
  it('names the family in the title, so one family down never reads as all', () => {
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
    const shifted = outage({ down_since: '1999-01-01T00:00:00Z' });
    expect(webhookIngressNotice(shifted).detail)
      .toBe(webhookIngressNotice(outage()).detail);
  });

  it('claims a lost reply, never a lost delivery', () => {
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
    const line = webhookIngressRowLine(outage());
    expect(line).not.toContain('node.tailnet.ts.net');
    expect(line).not.toContain('8443');
  });
});
