import type { WebhookIngressAddress, WebhookIngressOutage } from '../../api/client';
import { sendSeededPrompt } from './compose';
import { formatDurationPhrase } from '../../utils/formatTime';
import { quoteBlock } from '../../utils/markdownQuote';
import { FAMILY_LABEL, ingressFamiliesPhrase } from '../../utils/webhookIngressNotice';

function addressLine(probe: WebhookIngressAddress): string {
  const answered = probe.status === null ? probe.detail : `HTTP ${probe.status}`;
  const said = answered ? `, ${answered}` : '';
  return `- \`${probe.address}\` (${FAMILY_LABEL[probe.family]}): ${probe.stage}${said}`;
}

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

export async function discussWebhookIngress(outage: WebhookIngressOutage): Promise<void> {
  await sendSeededPrompt(
    webhookIngressDiscussPrompt(outage),
    'start a discussion about the webhook ingress outage',
  );
}
