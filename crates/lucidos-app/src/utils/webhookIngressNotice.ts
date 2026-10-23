/**
 * The words for a webhook ingress outage: what the app bar states while a family
 * is down, and what every enabled row on the Webhooks page says under its name.
 *
 * A leaf module because those two surfaces sit in different trees. The bar lives
 * in `components/layout/` and the row in `components/settings/`, so either
 * importing the other would be importing a view to get a sentence. Nothing here
 * has a view of its own, so the table is unit tested directly.
 *
 * Every sentence names the ADDRESS FAMILY, which is the whole lesson of the
 * outage this exists to catch. The funnel refused IPv4 for eight hours while
 * IPv6 answered correctly. A single boolean would have called that healthy.
 *
 * Nothing here claims a delivery was lost. The engine knows only what its own
 * probe met. What it can say is that a sender using those addresses gets no
 * reply, which is the fact the user has to act on.
 *
 * See `docs/adr/0143-webhook-ingress-probed-per-address-family.md`.
 */

import type { WebhookIngressFamily, WebhookIngressOutage } from '../api/client';
import { formatDurationPhrase } from './formatTime';

/** How each family is spelled for a reader. Exported because the Discuss prompt
 *  names them too, and two spellings of one family would describe one outage
 *  two ways. */
export const FAMILY_LABEL: Record<WebhookIngressFamily, string> = { ipv4: 'IPv4', ipv6: 'IPv6' };

/** How often the engine looks again. Matches `WEBHOOK_INGRESS_CRON` in
 *  `scheduler/webhook_ingress/`, and a number that drifts from it promises a
 *  recovery the probe does not keep. */
const PROBE_INTERVAL = 'every 15 minutes';

/** The families as a reader says them: `IPv4`, `IPv6`, or `IPv4 and IPv6`.
 *
 *  Named in a fixed order rather than the order they arrived in, so two surfaces
 *  reading one outage cannot describe it two ways.
 *
 *  An empty list names the path instead of a family. The engine declares an
 *  outage only with at least one family down, so this is unreachable today. A
 *  fallback that invented "every address" would be a claim, not a fallback. */
export function ingressFamiliesPhrase(families: WebhookIngressFamily[]): string {
  const named = (['ipv4', 'ipv6'] as const)
    .filter((family) => families.includes(family))
    .map((family) => FAMILY_LABEL[family]);
  return named.length > 0 ? named.join(' and ') : 'the public path';
}

/** Where the probe knocked, written the way a sender would address it. */
function origin(outage: WebhookIngressOutage): string {
  return `${outage.host}:${outage.port}`;
}

/** How long it has been down, rooted in the span the DATABASE measured.
 *
 *  The engine sends `down_secs` beside `down_since` for exactly this reason. A
 *  client that subtracted the two instants itself would report its own clock
 *  skew as outage time (ADR 0053).
 *
 *  `currentIngressOutage` has already advanced that span to now, by adding a gap
 *  between two readings of the browser clock. So the label keeps counting while
 *  the outage stands, and still subtracts no server instant from a local one. */
function outageAge(outage: WebhookIngressOutage): string {
  return formatDurationPhrase(outage.down_secs);
}

/** What the app bar states while the path is down.
 *
 *  The title is the fact and the detail is the consequence, matching the
 *  connection bar beside it. The detail says "sent that way" rather than naming
 *  the family a second time, so one wording serves one family and both. */
export function webhookIngressNotice(
  outage: WebhookIngressOutage,
): { title: string; detail: string } {
  const families = ingressFamiliesPhrase(outage.families);
  return {
    title: `Webhook deliveries over ${families} are not arriving`,
    detail:
      `${origin(outage)} has not answered over ${families} for ${outageAge(outage)}. ` +
      'A delivery sent that way gets no reply and never reaches this workspace. ' +
      `Rechecked ${PROBE_INTERVAL}.`,
  };
}

/** The one clause a Webhooks row carries while the path is down.
 *
 *  Drawn on every enabled hook, never on the one the probe happened to target:
 *  the ingress sits in front of all of them, so naming one would be wrong about
 *  the rest. The row already says it is a webhook, so this states the reach
 *  alone. */
export function webhookIngressRowLine(outage: WebhookIngressOutage): string {
  return `Not reachable over ${ingressFamiliesPhrase(outage.families)} for ${outageAge(outage)}`;
}
