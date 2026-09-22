/**
 * The words for a webhook that is throwing its deliveries away: what the app
 * bar states, and what its row on the Webhooks page says under its name.
 *
 * A leaf module for the reason `webhookIngressNotice.ts` is one: the bar lives
 * in `components/layout/` and the row in `components/settings/`, so either
 * importing the other would be importing a view to get a sentence. Nothing here
 * has a view, so the table is unit tested directly.
 *
 * Every sentence says plainly that deliveries are being LOST, which is the
 * whole lesson of the outage this catches. A hook somebody switched off
 * answered 401 to every delivery for 18 days, with a green panel row and a
 * green ingress probe throughout.
 *
 * See `docs/adr/0235-a-refused-delivery-is-an-outage.md`.
 */

import type { WebhookRefusal, WebhookRefusalCause } from '../api/client';
import { formatDurationPhrase } from './formatTime';

/** How often the engine looks again. Matches `WEBHOOK_REFUSAL_CRON` in
 *  `scheduler/webhook_refusal.rs`, and a number that drifts from it promises a
 *  recovery the check does not keep. */
const CHECK_INTERVAL = 'every 15 minutes';

/** How each reason the engine records reads for a person.
 *
 *  Keyed by `DeliveryRefusal::key`, which is a frozen wire value. A key this
 *  build does not know is printed as it stands rather than dropped: an engine
 *  newer than the page still has something true to say. */
const REASON_LABEL: Record<string, string> = {
  'disabled': 'the webhook is switched off',
  'body-not-utf8': 'the body was not text',
  'token': 'the bearer token did not match',
  'signature-missing': 'no readable signature',
  'signature-mismatch': 'the signature did not match',
  'timestamp-outside-tolerance': 'the signed timestamp was too old',
  'credential-missing': 'the signing secret is gone',
};

/** How long it has been going, rooted in the span the ENGINE measured.
 *
 *  `currentWebhookRefusals` has already advanced that span to now, by adding a
 *  gap between two readings of the browser clock. So the label keeps counting
 *  while the fault stands, and still subtracts no server instant from a local
 *  one (ADR 0053). */
function age(refusal: WebhookRefusal): string {
  return formatDurationPhrase(refusal.refusing_secs);
}

/** How many deliveries arrived, said as a person would.
 *
 *  The count is the evidence, so it is never rounded away. One is a real case,
 *  not an edge: a switched-off hook declares on its first refusal, because
 *  nothing was read and the loss is certain. The verb comes along, or every
 *  such bar reads "1 delivery have arrived". */
function arrived(refusal: WebhookRefusal): string {
  return refusal.refusals === 1
    ? '1 delivery has arrived'
    : `${refusal.refusals} deliveries have arrived`;
}

/** How many deliveries, as a bare count for a clause that supplies its own
 *  verb. */
function lost(refusal: WebhookRefusal): string {
  return refusal.refusals === 1 ? '1 delivery' : `${refusal.refusals} deliveries`;
}

/** What the run was made of, as a clause, or null when it says nothing new.
 *
 *  Omitted for a switched-off hook: the cause IS the reason, and repeating it
 *  would pad the sentence. Omitted for a single reason the headline already
 *  names, and for a tally that did not survive its round trip. */
export function refusalReasonsPhrase(refusal: WebhookRefusal): string | null {
  const named = Object.entries(refusal.reasons)
    .filter(([, count]) => count > 0)
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .map(([key, count]) => `${REASON_LABEL[key] ?? key} (${count})`);
  if (named.length === 0) return null;
  if (refusal.cause === 'disabled' && named.length === 1) return null;
  return named.join(', ');
}

/** The cause every surface speaks from, which is not always the stored one.
 *
 *  The live flag wins, the rule `judge` applies in the engine. A hook that is
 *  off threw the delivery away before reading it, so no run on it can support
 *  "none of them verified".
 *
 *  Stated again here on purpose. The verification words send the reader at the
 *  secret, and re-pointing a hook replaces its whole config and drops the
 *  secret with it. So a stale record must not reach those words for a hook
 *  this page can see is off.
 *
 *  Every surface reads it HERE rather than off the record, or the bar and the
 *  Discuss message name different causes for one hook. */
export function reportedRefusalCause(refusal: WebhookRefusal): WebhookRefusalCause {
  return refusal.enabled ? refusal.cause : 'disabled';
}

function switchedOff(refusal: WebhookRefusal): boolean {
  return reportedRefusalCause(refusal) === 'disabled';
}

/** What the app bar states while a hook is throwing deliveries away.
 *
 *  The title is the fact and the detail is the consequence, matching the two
 *  bars beside it.
 *
 *  The two causes never share a sentence. A switched-off hook verified nothing,
 *  so telling its owner to check the signature sends them somewhere there is
 *  nothing to find. That wrong turn has already cost a long investigation once.
 *  So the `disabled` title says SWITCHED OFF in as many words, and its fix is
 *  one click on the screen the other button opens. */
export function webhookRefusalNotice(
  refusal: WebhookRefusal,
): { title: string; detail: string } {
  if (switchedOff(refusal)) {
    return {
      title: `"${refusal.webhook_name}" is switched off and deliveries are being thrown away`,
      detail:
        `${arrived(refusal)} over ${age(refusal)}, and every one was refused ` +
        'before it was read. Nothing is wrong with the signature or the secret: ' +
        'the webhook is off. Switch it back on, or delete it and repoint the sender. ' +
        `Rechecked ${CHECK_INTERVAL}.`,
    };
  }
  const because = refusalReasonsPhrase(refusal);
  return {
    title: `"${refusal.webhook_name}" is refusing every delivery it gets`,
    detail:
      `${arrived(refusal)} over ${age(refusal)}, and none of them verified` +
      `${because ? `: ${because}` : ''}. ` +
      'The public path is fine, so this is the secret or the signature config. ' +
      `Rechecked ${CHECK_INTERVAL}.`,
  };
}

/** The one clause a Webhooks row carries while it is refusing.
 *
 *  The row already names the hook, so this states what is happening to its
 *  deliveries and nothing else. */
export function webhookRefusalRowLine(refusal: WebhookRefusal): string {
  const what = switchedOff(refusal)
    ? 'thrown away, because this webhook is switched off'
    : 'refused, because none of them verified';
  return `${lost(refusal)} ${what}, over ${age(refusal)}`;
}
