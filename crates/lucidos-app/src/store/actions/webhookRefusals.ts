/**
 * *Refusal run*: what a webhook does with the deliveries that DO arrive.
 *
 * The sibling of `webhookIngress.ts`, one layer in. That one says whether a
 * sender can reach this workspace. This one says whether the workspace then
 * throws the delivery away, which the ingress probe cannot see: it reads a 401
 * as healthy, because an unsigned probe is exactly what a live verifier turns
 * away.
 *
 * The engine judges every hook every 15 minutes and declares on the timeline.
 * This reads that declaration, so the bar and the Webhooks page describe the
 * same one.
 *
 * See `docs/adr/0235-a-refused-delivery-is-an-outage.md`.
 */
import { webhookRefusals } from '../store';
import { failedIfFresh, setLoadingIfFresh } from '../types';
import { fetchWebhookRefusals, type WebhookRefusal } from '../../api/client';
import { elapsedSeconds } from '../../utils/formatTime';

export async function loadWebhookRefusals(): Promise<void> {
  setLoadingIfFresh(webhookRefusals);
  try {
    const refusals = await fetchWebhookRefusals();
    webhookRefusals.value = { status: 'loaded', data: { refusals, receivedAt: Date.now() } };
  } catch (error) {
    // A failed refresh keeps whatever stands on screen. An engine that cannot
    // answer is itself one way deliveries stop landing. Clearing the bar right
    // then would hide the fault when it matters most.
    webhookRefusals.value = failedIfFresh(webhookRefusals.value, error);
  }
}

/** Every webhook refusing right now, or an empty list.
 *
 *  Both surfaces read it through here, so neither decides on its own what a
 *  loading or failed read means. Anything but a loaded answer is empty: a bar
 *  raised on a read that has not landed would be a claim the engine never made.
 *
 *  `refusing_secs` comes back advanced to `now`, because the engine measured it
 *  once and sends no further frame while the fault stands. Pass the shared
 *  clock (`useCoarseClock`) so the bar and the rows always agree.
 *
 *  Called during render, which IS the subscription (ADR 0118). */
export function currentWebhookRefusals(now: number): WebhookRefusal[] {
  const reading = webhookRefusals.value;
  if (reading.status !== 'loaded') return [];
  return reading.data.refusals.refusing.map((refusal) => ({
    ...refusal,
    refusing_secs: refusal.refusing_secs + elapsedSeconds(reading.data.receivedAt, now),
  }));
}

/** The one refusal a single-line notice speaks for, or null.
 *
 *  A switched-off hook wins, because it is the certain fault: nothing was read,
 *  so every delivery was thrown away. A verification failure is the same
 *  symptom with a cause the user has to go and find.
 *
 *  Ties break on the longest-standing, so the bar does not swap hooks between
 *  two ticks of the clock. */
export function loudestWebhookRefusal(refusals: WebhookRefusal[]): WebhookRefusal | null {
  const ranked = [...refusals].sort((a, b) => {
    if (a.cause !== b.cause) return a.cause === 'disabled' ? -1 : 1;
    return b.refusing_secs - a.refusing_secs;
  });
  return ranked[0] ?? null;
}
