/**
 * *Webhook ingress*: whether a delivery can still reach this workspace from
 * outside the machine.
 *
 * The engine probes the public path every 15 minutes and declares an outage on
 * the timeline. This reads that declaration, so the bar and the Webhooks page
 * describe the same one.
 *
 * The two `WebhookIngress*` frames re-read rather than carrying the new state
 * in. One route decides what counts as a live outage. It also drops a
 * declaration once no enabled webhook is left, so disabling the last hook
 * retracts the bar through that same path.
 *
 * See `docs/adr/0143-webhook-ingress-probed-per-address-family.md`.
 */
import { webhookIngress } from '../store';
import { failedIfFresh, setLoadingIfFresh } from '../types';
import { fetchWebhookIngress, type WebhookIngressOutage } from '../../api/client';

export async function loadWebhookIngress(): Promise<void> {
  setLoadingIfFresh(webhookIngress);
  try {
    const ingress = await fetchWebhookIngress();
    webhookIngress.value = { status: 'loaded', data: { ingress, receivedAt: Date.now() } };
  } catch (error) {
    // A failed refresh keeps the standing outage on screen. An engine that
    // cannot answer is itself one way this path breaks. Retracting the bar
    // right then would hide the fault when it matters most.
    webhookIngress.value = failedIfFresh(webhookIngress.value, error);
  }
}

/** The outage on screen right now, or null while the path is healthy.
 *
 *  Both surfaces read it through here, so neither can decide on its own what a
 *  loading or failed read means. Anything but a loaded healthy answer is null:
 *  a bar raised on a read that has not landed would be a claim the engine never
 *  made.
 *
 *  `down_secs` comes back advanced to `now`, because the engine measured it once
 *  and sends no further frame while the outage stands. Pass the shared clock
 *  (`useCoarseClock`) so the bar and the rows always agree.
 *
 *  Called during render, which IS the subscription (ADR 0118): a frame that
 *  reloads the signal repaints the bar and every Webhooks row together. */
export function currentIngressOutage(now: number): WebhookIngressOutage | null {
  const reading = webhookIngress.value;
  if (reading.status !== 'loaded') return null;
  const outage = reading.data.ingress.degraded;
  if (!outage) return null;
  return { ...outage, down_secs: outage.down_secs + secondsSince(reading.data.receivedAt, now) };
}

/** Whole seconds between two readings of the browser clock, never negative.
 *
 *  A clock the user or NTP moved backwards must not make an outage look younger
 *  than the engine measured it. */
function secondsSince(receivedAt: number, now: number): number {
  return Math.max(0, Math.floor((now - receivedAt) / 1000));
}
