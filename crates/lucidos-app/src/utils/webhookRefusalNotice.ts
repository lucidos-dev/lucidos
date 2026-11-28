
import type { WebhookRefusal, WebhookRefusalCause } from '../api/client';
import { formatDurationPhrase } from './formatTime';

const CHECK_INTERVAL = 'every 15 minutes';

const REASON_LABEL: Record<string, string> = {
  'disabled': 'the webhook is switched off',
  'body-not-utf8': 'the body was not text',
  'token': 'the bearer token did not match',
  'signature-missing': 'no readable signature',
  'signature-mismatch': 'the signature did not match',
  'timestamp-outside-tolerance': 'the signed timestamp was too old',
  'credential-missing': 'the signing secret is gone',
};

function age(refusal: WebhookRefusal): string {
  return formatDurationPhrase(refusal.refusing_secs);
}

function arrived(refusal: WebhookRefusal): string {
  return refusal.refusals === 1
    ? '1 delivery has arrived'
    : `${refusal.refusals} deliveries have arrived`;
}

function lost(refusal: WebhookRefusal): string {
  return refusal.refusals === 1 ? '1 delivery' : `${refusal.refusals} deliveries`;
}

export function refusalReasonsPhrase(refusal: WebhookRefusal): string | null {
  const named = Object.entries(refusal.reasons)
    .filter(([, count]) => count > 0)
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .map(([key, count]) => `${REASON_LABEL[key] ?? key} (${count})`);
  if (named.length === 0) return null;
  if (refusal.cause === 'disabled' && named.length === 1) return null;
  return named.join(', ');
}

export function reportedRefusalCause(refusal: WebhookRefusal): WebhookRefusalCause {
  return refusal.enabled ? refusal.cause : 'disabled';
}

function switchedOff(refusal: WebhookRefusal): boolean {
  return reportedRefusalCause(refusal) === 'disabled';
}

function unsignedOnly(refusal: WebhookRefusal): boolean {
  const counted = Object.entries(refusal.reasons).filter(([, count]) => count > 0);
  return counted.length > 0 && counted.every(([key]) => key === 'signature-missing');
}

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
  if (unsignedOnly(refusal)) {
    return {
      title: `"${refusal.webhook_name}" is refusing unsigned requests`,
      detail:
        `${arrived(refusal)} over ${age(refusal)}, and none of them carried a readable ` +
        'signature. A sender with a secret set signs every delivery, so these may not ' +
        'be from your sender. That is no sign the secret is wrong, so do not rotate it. ' +
        'If they are your sender\'s, check that it has a secret set and signs the way ' +
        `this webhook expects. Rechecked ${CHECK_INTERVAL}.`,
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

export function webhookRefusalRowLine(refusal: WebhookRefusal): string {
  return `${lost(refusal)} ${rowReason(refusal)}, over ${age(refusal)}`;
}

function rowReason(refusal: WebhookRefusal): string {
  if (switchedOff(refusal)) return 'thrown away, because this webhook is switched off';
  if (unsignedOnly(refusal)) return 'refused, because none of them carried a signature';
  return 'refused, because none of them verified';
}
