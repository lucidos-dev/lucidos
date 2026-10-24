/**
 * What the two refusal surfaces are allowed to claim, pinned away from either.
 *
 * Both read one selector, so the rules live here. A failed refresh keeps what
 * stood, and the age keeps counting while it stands. The bar picks one hook to
 * speak for by a rule neither surface decides on its own.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.mock('../../api/client', () => ({ fetchWebhookRefusals: vi.fn() }));

import {
  currentWebhookRefusals,
  loadWebhookRefusals,
  loudestWebhookRefusal,
} from './webhookRefusals';
import { fetchWebhookRefusals } from '../../api/client';
import type { WebhookRefusal } from '../../api/client';
import { webhookRefusals } from '../store';

const mockFetch = vi.mocked(fetchWebhookRefusals);

const LANDED = new Date('2026-09-20T06:00:00Z');

function refusal(over: Partial<WebhookRefusal> = {}): WebhookRefusal {
  return {
    webhook_id: '6f1c0f3e-0000-4000-8000-000000000001',
    webhook_name: 'GitHub workflow runs',
    enabled: false,
    cause: 'disabled',
    refusals: 42,
    reasons: { disabled: 42 },
    refusing_since: '2026-09-02T04:41:06Z',
    refusing_secs: 120,
    ...over,
  };
}

describe('webhook refusal readings', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    webhookRefusals.value = { status: 'not-loaded' };
    vi.useFakeTimers();
    vi.setSystemTime(LANDED);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('claims nothing while every hook is landing what it gets', async () => {
    mockFetch.mockResolvedValueOnce({ refusing: [] });
    await loadWebhookRefusals();
    expect(currentWebhookRefusals(Date.now())).toEqual([]);
  });

  it('claims nothing from a read that never landed', async () => {
    mockFetch.mockRejectedValueOnce(new Error('offline'));
    await loadWebhookRefusals();
    expect(webhookRefusals.value.status).toBe('failed');
    expect(currentWebhookRefusals(Date.now())).toEqual([]);
  });

  it('keeps a standing refusal through a failed refresh', async () => {
    mockFetch.mockResolvedValueOnce({ refusing: [refusal()] });
    await loadWebhookRefusals();

    // An engine that cannot answer is itself one way deliveries stop landing.
    mockFetch.mockRejectedValueOnce(new Error('engine restarting'));
    await loadWebhookRefusals();

    expect(currentWebhookRefusals(Date.now())).toHaveLength(1);
  });

  it('retracts on an empty answer', async () => {
    mockFetch.mockResolvedValueOnce({ refusing: [refusal()] });
    await loadWebhookRefusals();
    mockFetch.mockResolvedValueOnce({ refusing: [] });
    await loadWebhookRefusals();
    expect(currentWebhookRefusals(Date.now())).toEqual([]);
  });

  it('ages the fault forward while it stands', async () => {
    mockFetch.mockResolvedValueOnce({ refusing: [refusal({ refusing_secs: 120 })] });
    await loadWebhookRefusals();

    // The engine measures the span once and sends no frame while it stands. A
    // bar reading "for 2 minutes" after eighteen days is the exact failure
    // this whole feature exists to report.
    const eighteenDays = 18 * 24 * 60 * 60;
    const later = LANDED.getTime() + eighteenDays * 1000;
    expect(currentWebhookRefusals(later)[0].refusing_secs).toBe(120 + eighteenDays);
  });

  it('never ages the fault backwards when the clock moves back', async () => {
    mockFetch.mockResolvedValueOnce({ refusing: [refusal({ refusing_secs: 120 })] });
    await loadWebhookRefusals();
    expect(currentWebhookRefusals(LANDED.getTime() - 60_000)[0].refusing_secs).toBe(120);
  });
});

describe('loudestWebhookRefusal', () => {
  it('speaks for the switched-off hook first', () => {
    // The certain fault: nothing was read, so every delivery was thrown away.
    // A verification failure is the same symptom with a cause to go and find.
    const picked = loudestWebhookRefusal([
      refusal({ webhook_id: 'a', cause: 'verification', enabled: true, refusing_secs: 900_000 }),
      refusal({ webhook_id: 'b', cause: 'disabled', refusing_secs: 1800 }),
    ]);
    expect(picked?.webhook_id).toBe('b');
  });

  it('breaks a tie on the longest-standing, so the bar holds still', () => {
    const picked = loudestWebhookRefusal([
      refusal({ webhook_id: 'a', refusing_secs: 1800 }),
      refusal({ webhook_id: 'b', refusing_secs: 90_000 }),
    ]);
    expect(picked?.webhook_id).toBe('b');
  });

  it('leaves the list it was given alone', () => {
    // It is called during render off a signal-derived array, so a sort in
    // place would reorder what the Webhooks page is drawing.
    const given = [
      refusal({ webhook_id: 'a', cause: 'verification', enabled: true }),
      refusal({ webhook_id: 'b' }),
    ];
    loudestWebhookRefusal(given);
    expect(given.map((r) => r.webhook_id)).toEqual(['a', 'b']);
  });

  it('is null when nothing is refusing', () => {
    expect(loudestWebhookRefusal([])).toBeNull();
  });
});
