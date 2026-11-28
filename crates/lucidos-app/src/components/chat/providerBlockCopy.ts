import type { ProviderError } from '../../generated/thread-event-wire';

const BLOCK_REASON: Record<string, string> = {
  misalignment_policy_violation: 'potentially unintended activity',
  cyber_policy: 'its cybersecurity policy',
  bio_policy: 'its biosecurity policy',
};

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

export function providerCodeLine(error: ProviderError): string | null {
  return providerBlockCopy(error) ? null : `Provider error code: ${error.code}`;
}
