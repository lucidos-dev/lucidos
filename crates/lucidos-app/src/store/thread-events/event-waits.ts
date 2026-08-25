import type {
  EventSubscription,
  EventWaitSummary,
  ThreadEvent,
  TransientEvent,
} from './thread-event-types';
import type { ThreadMeta } from './thread-meta';

/** The `await_event` tool's name, as it appears on `ToolCalled` / `ToolResult`.
 *  Mirrors `llm::tool_names::AWAIT_EVENT`. */
export const AWAIT_EVENT_TOOL = 'await_event';

/** Fold one event into `meta.liveEventWaits`. Returns true when the list
 *  changed, so `handleEvent` knows to flag a meta change.
 *
 *  Two events feed it: `EventWaitStarted` appends a wait, and the three
 *  resolutions remove it by `wait_id`. Nothing in between changes it, because
 *  nothing in between can: a subscription does not hold its thread's turn, so
 *  a message, a new turn or a restart all leave it exactly as it was (ADR
 *  0049). The `await_event` `ToolResult` that used to *detach* a wait here is
 *  now just the call's own result and carries no meaning for this list.
 *
 *  **The fold is for immediacy, not for truth.** The server carries the same
 *  list on every thread summary and every per-event aggregate, and both
 *  overwrite `meta.liveEventWaits` wholesale. This is what applies an arm or a
 *  resolution the instant its event lands.
 *
 *  Idempotent by `wait_id`, so it converges with those snapshots rather than
 *  fighting them: an append replaces a same-id entry in place, and a
 *  resolution filters, which is a no-op once the snapshot already dropped it.
 *  Pure and total over the event stream too, so replay reconstructs exactly
 *  the same set that live SSE built incrementally. */
export function eventWaitProjection(
  meta: ThreadMeta,
  event: ThreadEvent | TransientEvent,
): boolean {
  switch (event.type) {
    case 'EventWaitStarted': {
      const next: EventWaitSummary = {
        wait_id: event.wait_id,
        on: event.on,
        reason: event.reason,
        expires_at: event.expires_at,
      };
      // Replay can re-deliver an event; keep the list a set by wait_id.
      const existing = meta.liveEventWaits.findIndex((w) => w.wait_id === event.wait_id);
      if (existing !== -1) {
        meta.liveEventWaits = meta.liveEventWaits.map((w, i) => (i === existing ? next : w));
      } else {
        meta.liveEventWaits = [...meta.liveEventWaits, next];
      }
      return true;
    }
    case 'EventWaitDelivered':
    case 'EventWaitExpired':
    case 'EventWaitCanceled': {
      const before = meta.liveEventWaits.length;
      meta.liveEventWaits = meta.liveEventWaits.filter((w) => w.wait_id !== event.wait_id);
      return meta.liveEventWaits.length !== before;
    }
    default:
      return false;
  }
}

/** Seconds left until `expires_at`, floored at 0. The indicator ticks this in
 *  component-local state; it is exported so the formatting is testable without
 *  a clock in the component. */
export function secondsRemaining(expiresAt: string, now: number): number {
  const deadline = Date.parse(expiresAt);
  if (Number.isNaN(deadline)) return 0;
  return Math.max(0, Math.round((deadline - now) / 1000));
}

/** A countdown a person can read at a glance: `2h 5m`, `4m 12s`, `18s`.
 *  Deliberately coarse above an hour, since nobody watching a release land
 *  cares about the seconds. */
export function formatRemaining(seconds: number): string {
  if (seconds <= 0) return 'due now';
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

/** One subscription as a label, using the same words the agent used in `on:`.
 *
 *  A condition is summarised as "filtered" rather than dumped: the raw operator
 *  JSON is developer-facing, and this is read by whoever is waiting.
 *
 *  The summary is not the whole answer though. The two PRESSABLE surfaces open
 *  the condition itself, through `eventConditionDoor`, which is what makes
 *  "filtered how" answerable from the UI. The third consumer is the archive
 *  confirmation below, whose dialog takes plain strings and offers no door. */
export function waitSubscriptionLabel(s: EventSubscription): string {
  return s.condition ? `${s.event_type} (filtered)` : s.event_type;
}

/** The whole subscription as one plain string, for a surface that can hold no
 *  markup at all: the archive confirmation's detail list, which is `string[]`.
 *
 *  Neither pressable surface comes through here. Both label each entry on its
 *  own, because both make a filtered entry pressable, and a button cannot
 *  survive a joined string.
 *
 *  Takes the `on:` list rather than a whole wait, because that is all it reads.
 *  Its caller holds whole waits and hands over `w.on`. */
export function describeWaitSubscription(on: EventSubscription[]): string {
  return on.map(waitSubscriptionLabel).join(' or ');
}
