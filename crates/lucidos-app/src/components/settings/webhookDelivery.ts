/**
 * What a webhook's last delivery left behind, in words.
 *
 * The page exists so that silence can be read. "Arrived and was refused" and
 * "never arrived" produce the same symptom, no events, and have completely
 * different causes. A rotated secret looks exactly like a dead ingress, and a
 * refusal is the only thing that tells the two apart.
 *
 * Both lines take `now`, so what they say is testable.
 */

import { formatAgoPhrase } from '../../utils/formatTime';
import type { Webhook } from '../../api/client';

/** The timestamp, or `null` if there is nothing readable to say.
 *
 *  An unparseable stamp is a bug in whatever wrote it. Rendering it as "never"
 *  would bury that, so the caller drops the clause instead. */
function stampedAt(iso: string | null): Date | null {
  if (!iso) return null;
  const at = new Date(iso);
  return Number.isNaN(at.getTime()) ? null : at;
}

/** When a delivery last verified and emitted.
 *
 *  `null` only for a stamp that will not parse. A hook that has never accepted
 *  anything says so, which is the reading the outage needed. */
export function lastDeliveryLine(
  hook: Pick<Webhook, 'last_accepted_at'>,
  now: Date,
): string | null {
  if (!hook.last_accepted_at) return 'No delivery has verified yet';
  const at = stampedAt(hook.last_accepted_at);
  return at ? `Last delivery ${formatAgoPhrase(at, now)}` : null;
}

/** When a delivery last arrived and was turned away, and why.
 *
 *  `null` when none ever has, because an absent refusal is not news. Shown
 *  alongside the line above rather than instead of it: a hook can be accepting
 *  one sender and refusing another. */
export function lastRefusalLine(
  hook: Pick<Webhook, 'last_refused_at' | 'last_refusal_reason'>,
  now: Date,
): string | null {
  const at = stampedAt(hook.last_refused_at);
  if (!at) return null;
  const when = `Last refused ${formatAgoPhrase(at, now)}`;
  return hook.last_refusal_reason ? `${when}: ${hook.last_refusal_reason}` : when;
}
