import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.mock('../../api/client', () => ({ fetchWebhookIngress: vi.fn() }));

import { currentIngressOutage, ingressIsUnverified, loadWebhookIngress } from './webhookIngress';
import { fetchWebhookIngress } from '../../api/client';
import type { WebhookIngressOutage } from '../../api/client';
import { webhookIngress } from '../store';

const mockFetch = vi.mocked(fetchWebhookIngress);

const LANDED = new Date('2026-08-27T06:00:00Z');

function outage(over: Partial<WebhookIngressOutage> = {}): WebhookIngressOutage {
  return {
    webhook_name: 'github-ci',
    host: 'hooks.example.ts.net',
    port: 8443,
    families: ['ipv4'],
    addresses: [],
    down_since: '2026-08-26T22:00:00Z',
    down_secs: 120,
    ...over,
  };
}

describe('webhook ingress readings', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    webhookIngress.value = { status: 'not-loaded' };
    vi.useFakeTimers();
    vi.setSystemTime(LANDED);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('claims no outage while the path is healthy', async () => {
    mockFetch.mockResolvedValueOnce({ unverified: false, degraded: null });
    await loadWebhookIngress();
    expect(currentIngressOutage(Date.now())).toBeNull();
  });

  it('claims no outage from a read that never landed', async () => {
    mockFetch.mockRejectedValueOnce(new Error('offline'));
    await loadWebhookIngress();
    expect(webhookIngress.value.status).toBe('failed');
    expect(currentIngressOutage(Date.now())).toBeNull();
  });

  it('keeps a standing outage through a failed refresh', async () => {
    mockFetch.mockResolvedValueOnce({ unverified: false, degraded: outage() });
    await loadWebhookIngress();

    mockFetch.mockRejectedValueOnce(new Error('engine restarting'));
    await loadWebhookIngress();

    expect(currentIngressOutage(Date.now())?.families).toEqual(['ipv4']);
  });

  it('retracts the outage on a healthy answer', async () => {
    mockFetch.mockResolvedValueOnce({ unverified: false, degraded: outage() });
    await loadWebhookIngress();
    mockFetch.mockResolvedValueOnce({ unverified: false, degraded: null });
    await loadWebhookIngress();
    expect(currentIngressOutage(Date.now())).toBeNull();
  });

  it('updates the unverified notice independently of outage details', async () => {
    mockFetch.mockResolvedValueOnce({ degraded: null, unverified: true });
    await loadWebhookIngress();
    expect(ingressIsUnverified()).toBe(true);
    expect(currentIngressOutage(Date.now())).toBeNull();

    mockFetch.mockResolvedValueOnce({ degraded: null, unverified: false });
    await loadWebhookIngress();
    expect(ingressIsUnverified()).toBe(false);
  });

  it('ages the outage forward while it stands', async () => {
    mockFetch.mockResolvedValueOnce({ unverified: false, degraded: outage({ down_secs: 120 }) });
    await loadWebhookIngress();

    const eightHours = 8 * 60 * 60;
    const later = LANDED.getTime() + eightHours * 1000;
    expect(currentIngressOutage(later)?.down_secs).toBe(120 + eightHours);
  });

  it('never ages the outage backwards when the clock moves back', async () => {
    mockFetch.mockResolvedValueOnce({ unverified: false, degraded: outage({ down_secs: 120 }) });
    await loadWebhookIngress();
    expect(currentIngressOutage(LANDED.getTime() - 60_000)?.down_secs).toBe(120);
  });
});
