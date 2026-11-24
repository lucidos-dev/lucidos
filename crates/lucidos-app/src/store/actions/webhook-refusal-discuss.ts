import type { WebhookRefusal } from '../../api/client';
import { sendSeededPrompt } from './compose';
import { quoteBlock } from '../../utils/markdownQuote';
import {
  reportedRefusalCause,
  webhookRefusalNotice,
} from '../../utils/webhookRefusalNotice';

export function webhookRefusalDiscussPrompt(
  refusal: WebhookRefusal,
  others: string | null,
): string {
  const notice = webhookRefusalNotice(refusal);
  const facts = [
    `**${notice.title}**`,
    '',
    `- Cause: \`${reportedRefusalCause(refusal)}\``,
    `- Refusal run started: ${refusal.refusing_since}`,
    '',
    others ? `${notice.detail} ${others}` : notice.detail,
  ];
  return `Let's discuss these refused webhook deliveries:\n\n${quoteBlock(facts.join('\n'))}`;
}

export async function discussWebhookRefusal(
  refusal: WebhookRefusal,
  others: string | null,
): Promise<void> {
  await sendSeededPrompt(
    webhookRefusalDiscussPrompt(refusal, others),
    'start a discussion about the refused webhook deliveries',
  );
}
