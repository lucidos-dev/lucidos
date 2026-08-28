/**
 * What the two ingress surfaces are allowed to claim, pinned away from either.
 *
 * Both read one selector, so the rules live here: a failed refresh keeps the
 * outage it already had, and the age keeps counting while the outage stands.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.mock('../../api/client', () => ({ fetchWebhookIngress: vi.fn() }));

import { currentIngressOutage, loadWebhookIngress } from './webhookIngress';
import { fetchWebhookIngress } from '../../api/client';
import type { WebhookIngressOutage } from '../../api/client';
import { webhookIngress } from '../store';

const mockFetch = vi.mocked(fetchWebhookIngress);

const LANDED = new Date('2026-08-27T06:00:00Z');

function outage(over: Partial<WebhookIngressOutage> = {}): WebhookIngressOutage {
  return {
    host: 'hooks.example.ts.net',
    port: 8443,
    families: ['ipv4'],
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
    mockFetch.mockResolvedValueOnce({ degraded: null });
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
    mockFetch.mockResolvedValueOnce({ degraded: outage() });
    await loadWebhookIngress();

    // An engine that cannot answer is itself one way this path breaks.
    mockFetch.mockRejectedValueOnce(new Error('engine restarting'));
    await loadWebhookIngress();

    expect(currentIngressOutage(Date.now())?.families).toEqual(['ipv4']);
  });

  it('retracts the outage on a healthy answer', async () => {
    mockFetch.mockResolvedValueOnce({ degraded: outage() });
    await loadWebhookIngress();
    mockFetch.mockResolvedValueOnce({ degraded: null });
    await loadWebhookIngress();
    expect(currentIngressOutage(Date.now())).toBeNull();
  });

  it('ages the outage forward while it stands', async () => {
    mockFetch.mockResolvedValueOnce({ degraded: outage({ down_secs: 120 }) });
    await loadWebhookIngress();

    // The whole outage this feature exists to catch, read eight hours in.
    const eightHours = 8 * 60 * 60;
    const later = LANDED.getTime() + eightHours * 1000;
    expect(currentIngressOutage(later)?.down_secs).toBe(120 + eightHours);
  });

  it('never ages the outage backwards when the clock moves back', async () => {
    mockFetch.mockResolvedValueOnce({ degraded: outage({ down_secs: 120 }) });
    await loadWebhookIngress();
    expect(currentIngressOutage(LANDED.getTime() - 60_000)?.down_secs).toBe(120);
  });
});
