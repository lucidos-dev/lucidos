import { webhookRefusals } from '../store';
import { failedIfFresh, setLoadingIfFresh } from '../types';
import { loadSlice, type SliceOf } from '../bootstrapSliceLoads';
import { fetchWebhookRefusals, type WebhookRefusal } from '../../api/client';
import { elapsedSeconds } from '../../utils/elapsed';

export const webhookRefusalsSlice: SliceOf<'webhook_refusals'> = {
  begin: () => setLoadingIfFresh(webhookRefusals),
  apply: (refusals) => {
    webhookRefusals.value = { status: 'loaded', data: { refusals, receivedAt: Date.now() } };
  },
  fail: (error) => {
    webhookRefusals.value = failedIfFresh(webhookRefusals.value, error);
  },
};

export function loadWebhookRefusals(): Promise<void> {
  return loadSlice('webhook_refusals', webhookRefusalsSlice, () => fetchWebhookRefusals());
}

export function currentWebhookRefusals(now: number): WebhookRefusal[] {
  const reading = webhookRefusals.value;
  if (reading.status !== 'loaded') return [];
  return reading.data.refusals.refusing.map((refusal) => ({
    ...refusal,
    refusing_secs: refusal.refusing_secs + elapsedSeconds(reading.data.receivedAt, now),
  }));
}

export function loudestWebhookRefusal(refusals: WebhookRefusal[]): WebhookRefusal | null {
  const ranked = [...refusals].sort((a, b) => {
    if (a.cause !== b.cause) return a.cause === 'disabled' ? -1 : 1;
    return b.refusing_secs - a.refusing_secs;
  });
  return ranked[0] ?? null;
}
