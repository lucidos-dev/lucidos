/**
 * Quoting a block of text into a message the user sends.
 *
 * A leaf module because three Discuss buttons need it and none owns it. They
 * quote a notification, a webhook ingress outage, and a refusing webhook.
 * Sharing the quoting is the whole of what they have in common. What they
 * quote differs completely, so each builds its own block and only the wrapper
 * is here.
 */

/** Prefix every line with a markdown blockquote marker. A blank line takes a
 *  bare `>` so the quote stays one block instead of splitting in two. */
export function quoteBlock(text: string): string {
  return text
    .split('\n')
    .map((line) => (line.trim() ? `> ${line}` : '>'))
    .join('\n');
}
