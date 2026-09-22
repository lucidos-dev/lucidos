/**
 * Discuss a refusing webhook with the Lucidos Agent.
 *
 * The bar names a symptom. "No readable signature" leaves three questions
 * open. Does the secret on this side differ from the sender's hook config?
 * Does the sender emit the header at all? Does something in between strip it?
 *
 * So the button quotes the declaration itself and hands the agent the whole of
 * it: which hook, which cause, how many deliveries, how long, and what the run
 * was made of.
 *
 * The engine reports a refusal run and never repairs one, so the recovery step
 * lives with whoever reads this.
 *
 * See `docs/adr/0235-a-refused-delivery-is-an-outage.md`.
 */
import type { WebhookRefusal } from '../../api/client';
import { sendSeededPrompt } from './compose';
import { quoteBlock } from '../../utils/markdownQuote';
import {
  reportedRefusalCause,
  webhookRefusalNotice,
} from '../../utils/webhookRefusalNotice';

/** The message the Discuss button sends: a lead-in, then the refusal quoted.
 *
 *  A complete message rather than a lead-in the user finishes, because Discuss
 *  sends it. Quoting the declaration is what carries it to the agent, which
 *  cannot read the bar.
 *
 *  The bullets carry only what the notice does not say: the wire cause, and
 *  when the run started. The notice already names the hook, the count, the age
 *  and every reason with its tally. A second voice on the cause is the one
 *  thing this must not grow, and the wrong one has already cost one long
 *  investigation.
 *
 *  `others` is the clause the bar draws after the notice, so the quote matches
 *  the bar. Several hooks failing at once is the evidence that one rotated
 *  secret broke them all.
 *
 *  Nothing here reads a clock, so it subtracts no server instant from a local
 *  one (ADR 0053). Pure, so the shape is testable without a store. */
export function webhookRefusalDiscussPrompt(
  refusal: WebhookRefusal,
  others: string | null,
): string {
  const notice = webhookRefusalNotice(refusal);
  const facts = [
    `**${notice.title}**`,
    '',
    // The cause the notice below speaks from, never the stored one. A record
    // saying `verification` for a hook that is off would name one cause in the
    // bullet and the other in the sentence under it.
    `- Cause: \`${reportedRefusalCause(refusal)}\``,
    // "Refusal run" in full, the canonical term. A bare "Run started" reads as
    // the sender's own run on a hook named for one, and the fixture hook is
    // called "GitHub workflow runs".
    `- Refusal run started: ${refusal.refusing_since}`,
    '',
    others ? `${notice.detail} ${others}` : notice.detail,
  ];
  return `Let's discuss these refused webhook deliveries:\n\n${quoteBlock(facts.join('\n'))}`;
}

/** Start a conversation about the standing refusal.
 *
 *  `sendSeededPrompt` owns the whole gesture. It takes a draft of its own
 *  rather than the one being typed in. It forces the Lucidos Agent
 *  destination, reveals the thread pane, sends, and toasts on failure.
 *
 *  The bar stays up behind the conversation. It retracts on an acceptance, a
 *  reconfigured hook, a fortnight of silence or a deletion, so hiding it here
 *  would claim a recovery nothing measured. */
export async function discussWebhookRefusal(
  refusal: WebhookRefusal,
  others: string | null,
): Promise<void> {
  await sendSeededPrompt(
    webhookRefusalDiscussPrompt(refusal, others),
    'start a discussion about the refused webhook deliveries',
  );
}
