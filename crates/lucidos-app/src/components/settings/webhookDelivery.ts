
import { formatAgoPhrase } from '../../utils/formatTime';
import type { Webhook } from '../../api/client';

function stampedAt(iso: string | null): Date | null {
  if (!iso) return null;
  const at = new Date(iso);
  return Number.isNaN(at.getTime()) ? null : at;
}

export function lastDeliveryLine(
  hook: Pick<Webhook, 'last_accepted_at'>,
  now: Date,
): string | null {
  if (!hook.last_accepted_at) return 'No delivery has verified yet';
  const at = stampedAt(hook.last_accepted_at);
  return at ? `Last delivery ${formatAgoPhrase(at, now)}` : null;
}

export function lastRefusalLine(
  hook: Pick<Webhook, 'last_refused_at' | 'last_refusal_reason'>,
  now: Date,
): string | null {
  const at = stampedAt(hook.last_refused_at);
  if (!at) return null;
  const when = `Last refused ${formatAgoPhrase(at, now)}`;
  return hook.last_refusal_reason ? `${when}: ${hook.last_refusal_reason}` : when;
}
