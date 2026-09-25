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

/** Every entry of an `on:` list that names one event type, shown as one.
 *
 *  An agent often watches one type several times with different filters (one
 *  `CodingAgentIdled` per session). Listing each entry printed the same name six
 *  times, joined by five "or"s, and said nothing the grouped form does not. */
export interface SubscriptionGroup {
  event_type: string;
  /** Empty when any entry is unfiltered: that entry matches every event of the
   *  type, so the group does too, whatever its siblings filter on. */
  conditions: Record<string, unknown>[];
}

/** Groups in first-seen order, so the list still reads the way the agent wrote it. */
export function groupSubscriptions(on: EventSubscription[]): SubscriptionGroup[] {
  const groups = new Map<string, { conditions: Record<string, unknown>[]; unfiltered: boolean }>();
  for (const s of on) {
    const g = groups.get(s.event_type) ?? { conditions: [], unfiltered: false };
    if (s.condition) g.conditions.push(s.condition);
    else g.unfiltered = true;
    groups.set(s.event_type, g);
  }
  return [...groups].map(([event_type, g]) => ({
    event_type,
    conditions: g.unfiltered ? [] : g.conditions,
  }));
}

/** Everyday words for the event types a thread most often waits on. Any type
 *  not listed here is split into words by `plainEventName`. */
const PLAIN_EVENT_NAMES: Record<string, string> = {
  BackgroundBashCompleted: 'background job finished',
  CodingAgentIdled: 'coding agent stopped working',
  ChangeProposed: 'change proposed',
  ChangeApplied: 'change applied',
  E2ELockReleased: 'test lock released',
};

/** One word of a PascalCase type: an acronym run, a capitalised word, or digits
 *  glued to an acronym (`E2E`). */
const TYPE_WORD = /[A-Z0-9]+(?=[A-Z][a-z])|[A-Z]?[a-z]+|[A-Z0-9]+/g;

/** An event type as a reader says it: `BackgroundBashCompleted` reads
 *  "background job finished". Event types are named in the past tense, so the
 *  phrase fits a waiting row and an arrival card alike.
 *
 *  Lower case, because it usually sits mid-sentence. An acronym keeps its
 *  capitals. The raw type stays on the chip's tooltip (`eventNameChip`). */
export function plainEventName(eventType: string): string {
  const known = PLAIN_EVENT_NAMES[eventType];
  if (known) return known;
  const words = eventType.match(TYPE_WORD);
  if (!words) return eventType;
  return words.map((w) => (w.length > 1 && w === w.toUpperCase() ? w : w.toLowerCase())).join(' ');
}

/** How a group is narrowed, in words, or `undefined` when it is not.
 *
 *  A condition is summarised rather than dumped: the raw operator JSON is
 *  developer-facing, and this is read by whoever is waiting. The two PRESSABLE
 *  surfaces open the conditions themselves, through `eventConditionDoor`. */
export function subscriptionFilterNote(g: SubscriptionGroup): string | undefined {
  const n = g.conditions.length;
  if (n === 0) return undefined;
  return n === 1 ? 'matching only' : `${n} conditions`;
}

/** One group as a plain label, for a surface that holds no markup. */
export function waitSubscriptionLabel(g: SubscriptionGroup): string {
  const name = plainEventName(g.event_type);
  const note = subscriptionFilterNote(g);
  return note ? `${name} (${note})` : name;
}

/** The whole subscription as one plain string, for a surface that can hold no
 *  markup at all: the archive confirmation's detail list, which is `string[]`.
 *
 *  Neither pressable surface comes through here. Both label each group on its
 *  own, because both make a filtered group pressable, and a button cannot
 *  survive a joined string. */
export function describeWaitSubscription(on: EventSubscription[]): string {
  return groupSubscriptions(on).map(waitSubscriptionLabel).join(' or ');
}
