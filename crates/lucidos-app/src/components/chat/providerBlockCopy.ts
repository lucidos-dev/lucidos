import type { ProviderError } from '../../generated/thread-event-wire';

/** The reason OpenAI gave, in the words the card uses, by the provider's code.
 *  Only these three codes are policy blocks. */
const BLOCK_REASON: Record<string, string> = {
  misalignment_policy_violation: 'potentially unintended activity',
  cyber_policy: 'its cybersecurity policy',
  bio_policy: 'its biosecurity policy',
};

/** What the failure card says about a turn OpenAI blocked, or null for any
 *  other failure. OpenAI's own words are shown apart, in
 *  `.exchange-error-output`. */
export function providerBlockCopy(error: ProviderError): { heading: string; reason: string } | null {
  const reason = BLOCK_REASON[error.code];
  if (!reason) return null;
  return {
    heading: 'OpenAI blocked this turn',
    reason:
      `OpenAI's safety check flagged this turn for ${reason}. OpenAI does not say which part triggered it. ` +
      'The branch keeps all work. Your next message starts a new session.',
  };
}

/** The line that names a provider code Lucidos has no copy for, so the user
 *  still sees what the provider said. Null for a policy block, which has its
 *  own card. */
export function providerCodeLine(error: ProviderError): string | null {
  return providerBlockCopy(error) ? null : `Provider error code: ${error.code}`;
}
