/**
 * Discuss a webhook ingress outage with the Lucidos Agent.
 *
 * The bar states the outage in one sentence, which is right for a bar and too
 * little to reason from. So the button quotes the declaration itself: which
 * hook, which public path, which families, how long, and what every probed
 * address answered.
 *
 * The engine reports an ingress outage and never repairs one, so the recovery
 * step lives with whoever reads this. Handing the agent the diagnosis is what
 * lets it be useful rather than asking for the detail back.
 *
 * See `docs/adr/0143-webhook-ingress-probed-per-address-family.md`.
 */
import type { WebhookIngressAddress, WebhookIngressOutage } from '../../api/client';
import { sendSeededPrompt } from './compose';
import { formatDurationPhrase } from '../../utils/formatTime';
import { quoteBlock } from '../../utils/markdownQuote';
import { FAMILY_LABEL, ingressFamiliesPhrase } from '../../utils/webhookIngressNotice';

/** One address as a bullet: what was dialled, and how far the request got.
 *
 *  The stage keeps its wire spelling, which is the vocabulary the ADR pins.
 *  A workspace trigger already codes against those words, so renaming them for
 *  prose would put two names on one concept. */
function addressLine(probe: WebhookIngressAddress): string {
  const answered = probe.status === null ? probe.detail : `HTTP ${probe.status}`;
  const said = answered ? `, ${answered}` : '';
  return `- \`${probe.address}\` (${FAMILY_LABEL[probe.family]}): ${probe.stage}${said}`;
}

/** The message the Discuss button sends: a lead-in, then the outage quoted.
 *
 *  A complete message rather than a lead-in the user finishes, because Discuss
 *  sends it. Quoting the declaration is what carries it to the agent, which
 *  cannot read the bar.
 *
 *  Pure, so the shape is testable without a store. */
export function webhookIngressDiscussPrompt(outage: WebhookIngressOutage): string {
  const facts = [
    `**Webhook ingress is degraded over ${ingressFamiliesPhrase(outage.families)}**`,
    '',
    `- Public path: \`${outage.host}:${outage.port}\``,
    `- Probed webhook: ${outage.webhook_name}`,
    `- Down for: ${formatDurationPhrase(outage.down_secs)}, since ${outage.down_since}`,
  ];
  if (outage.addresses.length > 0) {
    facts.push('', 'What each address answered:', ...outage.addresses.map(addressLine));
  }
  return `Let's discuss this webhook ingress outage:\n\n${quoteBlock(facts.join('\n'))}`;
}

/** Start a conversation about the standing ingress outage.
 *
 *  `sendSeededPrompt` owns the whole gesture: it confirms before replacing a
 *  draft in progress, forces the Lucidos Agent destination, reveals the thread
 *  pane, sends, and toasts on failure.
 *
 *  The bar stays up behind the conversation. Only a good probe retracts it, so
 *  hiding it here would claim a recovery nothing measured. */
export async function discussWebhookIngress(outage: WebhookIngressOutage): Promise<void> {
  await sendSeededPrompt(
    webhookIngressDiscussPrompt(outage),
    'start a discussion about the webhook ingress outage',
  );
}
