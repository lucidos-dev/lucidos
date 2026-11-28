import { webhookIngress } from '../store';
import { failedIfFresh, setLoadingIfFresh } from '../types';
import { loadSlice, type SliceOf } from '../bootstrapSliceLoads';
import { fetchWebhookIngress, type WebhookIngressOutage } from '../../api/client';
import { elapsedSeconds } from '../../utils/elapsed';

export const webhookIngressSlice: SliceOf<'webhook_ingress'> = {
  begin: () => setLoadingIfFresh(webhookIngress),
  apply: (ingress) => {
    webhookIngress.value = { status: 'loaded', data: { ingress, receivedAt: Date.now() } };
  },
  fail: (error) => {
    webhookIngress.value = failedIfFresh(webhookIngress.value, error);
  },
};

export function loadWebhookIngress(): Promise<void> {
  return loadSlice('webhook_ingress', webhookIngressSlice, () => fetchWebhookIngress());
}

export function ingressIsUnverified(): boolean {
  const reading = webhookIngress.value;
  return reading.status === 'loaded' && reading.data.ingress.unverified === true;
}

export function currentIngressOutage(now: number): WebhookIngressOutage | null {
  const reading = webhookIngress.value;
  if (reading.status !== 'loaded') return null;
  const outage = reading.data.ingress.degraded;
  if (!outage) return null;
  return {
    ...outage,
    down_secs: outage.down_secs + elapsedSeconds(reading.data.receivedAt, now),
  };
}
