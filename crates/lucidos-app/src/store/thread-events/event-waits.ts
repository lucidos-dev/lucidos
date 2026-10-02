import type {
  EngineReason,
  EventSubscription,
  EventWaitSummary,
  StoredEvent,
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
  ContinuationStarted: 'resumed',
  TriggerStarted: 'trigger fired',
  ChildThreadCompleted: 'child thread returned',
  ChildThreadStopped: 'child thread stopped',
  ChildThreadDetached: 'child thread moved to top level',
  MessageHeld: 'message held',
  MissingHardeningDetected: 'hardening needed',
  MergeConflictDetected: 'merge conflict',
  UserPromptInjected: 'message to the agent',
  CodingAgentPromptSent: 'instructions to the agent',
  ResponseAborted: 'response interrupted',
  ResponseCanceled: 'response canceled',
  EventWaitCanceled: 'stopped waiting',
  McpConsentRequested: 'tool consent requested',
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
 *  capitals. The chip's tooltip says what it means (`plainEventMeaning`). */
export function plainEventName(eventType: string): string {
  const known = PLAIN_EVENT_NAMES[eventType];
  if (known) return known;
  const words = eventType.match(TYPE_WORD);
  if (!words) return eventType;
  return words.map((w) => (w.length > 1 && w === w.toUpperCase() ? w : w.toLowerCase())).join(' ');
}

/** What an event means, as a sentence, for the chip's tooltip. The chip already
 *  shows the plain name, so this says the next thing a reader asks. */
const PLAIN_EVENT_MEANINGS: Record<string, string> = {
  BackgroundBashCompleted: 'A command the agent started in the background has finished.',
  CodingAgentIdled: 'A coding agent finished its turn and is waiting.',
  ChangeProposed: 'An agent offered a change for you to apply.',
  ChangeApplied: 'A change was merged into the project.',
  ContinuationStarted: 'A reply that had stopped was picked back up.',
  TriggerStarted: 'A trigger ran, on its schedule or on an event.',
  ChildThreadCompleted: 'A child thread finished and reported back.',
  ChildThreadStopped: 'A child thread was stopped before it finished.',
  ChildThreadDetached: 'A child thread was moved to top level, so it no longer reports back here.',
  MessageHeld: 'A message waited for your reply before the agent read it.',
  MissingHardeningDetected: 'The change had not passed hardening, its automatic review and test run, yet.',
  MergeConflictDetected: 'The change conflicts with newer changes in the project: a merge conflict.',
  UserPromptInjected: 'Words handed to the agent while it works, from you, another thread or Lucidos.',
  CodingAgentPromptSent: 'Words Lucidos handed to the coding agent.',
  ResponseAborted: 'Lucidos stopped a reply before it finished.',
  ResponseCanceled: 'You stopped a reply before it finished.',
  EventWaitCanceled: 'You told the agent to stop waiting.',
  McpConsentRequested: 'A tool asked for your permission before its first use.',
  E2ELockReleased: 'The end-to-end test run finished, so another can start.',
};

/** The chip tooltip for an event type, or `undefined` for one with no written
 *  meaning. A raw type would only repeat the chip in machine words. */
export function plainEventMeaning(eventType: string): string | undefined {
  return PLAIN_EVENT_MEANINGS[eventType];
}

/** The engine's reason on an *event wait* re-entry. */
export type EventWaitReason = Extract<EngineReason, { kind: 'event_wait' }>;

/** The opening sentences the engine writes into a re-entry's prose
 *  (`event_wait/mod.rs`). Matched only on rows written before the engine
 *  stamped an origin. Those rows are frozen, so the words cannot drift. */
export const LEGACY_REENTRY_OPENINGS: [string, EventWaitReason['outcome']][] = [
  ['A subscription you registered has timed out.', 'expired'],
  ['An event you subscribed to has arrived', 'delivered'],
];

/** Why a `UserPromptInjected` exists, when it is an *event wait* re-entry.
 *
 *  The engine's origin says so on every row written now. An older row recorded
 *  no origin, so its shape is read instead: a delivery link, or one of the two
 *  opening sentences. Such a row knows only the outcome, so `watched` and
 *  `wait_reason` come back empty and every reader drops those clauses. */
export function waitReentryReason(event: StoredEvent): EventWaitReason | undefined {
  if (event.type !== 'UserPromptInjected') return undefined;
  if (event.origin?.kind === 'engine') {
    return event.origin.reason.kind === 'event_wait' ? event.origin.reason : undefined;
  }
  if (event.origin) return undefined;
  const legacy = (outcome: EventWaitReason['outcome']): EventWaitReason =>
    ({ kind: 'event_wait', outcome, watched: [], wait_reason: '' });
  if (event.delivered_event_id) return legacy('delivered');
  const opening = LEGACY_REENTRY_OPENINGS.find(([words]) => event.text.startsWith(words));
  return opening ? legacy(opening[1]) : undefined;
}

/** Why the engine acted, for a starter whose row predates its `origin`. */
const ENGINE_REASON_BY_TYPE: Partial<Record<string, EngineReason>> = {
  MissingHardeningDetected: { kind: 'missing_hardening' },
  MergeConflictDetected: { kind: 'merge_conflict' },
};

/** Why the engine wrote a turn's starter: the recorded engine reason, or the
 *  one its shape implies on a row too old to carry one. */
export function starterEngineReason(event: StoredEvent): EngineReason | undefined {
  const origin = 'origin' in event ? event.origin : undefined;
  if (origin?.kind === 'engine') return origin.reason;
  return waitReentryReason(event) ?? ENGINE_REASON_BY_TYPE[event.type];
}

/** How a group is narrowed, in words, or `undefined` when it is not.
 *
 *  A condition is summarised rather than dumped: the raw operator JSON is
 *  developer-facing, and this is read by whoever is waiting. The two PRESSABLE
 *  surfaces open the conditions themselves, through `eventConditionDoor`. */
export function subscriptionFilterNote(g: SubscriptionGroup): string | undefined {
  const n = g.conditions.length;
  if (n === 0) return undefined;
  return n === 1 ? 'with a condition' : `${n} conditions`;
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
