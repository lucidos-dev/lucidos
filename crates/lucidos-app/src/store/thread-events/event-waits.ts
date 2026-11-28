import type {
  EngineReason,
  EventSubscription,
  EventWaitSummary,
  StoredEvent,
  ThreadEvent,
  TransientEvent,
} from './thread-event-types';
import type { ThreadMeta } from './thread-meta';

export const AWAIT_EVENT_TOOL = 'await_event';

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
        background_timing: event.background_timing,
      };
      const existing = meta.liveEventWaits.findIndex((w) => w.wait_id === event.wait_id);
      if (existing !== -1) {
        meta.liveEventWaits = meta.liveEventWaits.map((w, i) => (i === existing ? next : w));
      } else {
        meta.liveEventWaits = [...meta.liveEventWaits, next];
      }
      return true;
    }
    case 'EventWaitTimingUpdated': {
      const index = meta.liveEventWaits.findIndex((wait) => wait.wait_id === event.wait_id);
      if (index < 0) return false;
      meta.liveEventWaits = meta.liveEventWaits.map((wait, i) => i === index ? {
        ...wait,
        reason: event.reason,
        expires_at: event.expires_at,
        background_timing: event.background_timing,
      } : wait);
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

export function displayedWaitDeadline(wait: Pick<EventWaitSummary, 'expires_at' | 'background_timing'>): string {
  const timing = wait.background_timing;
  if (!timing) return wait.expires_at;
  return timing.state === 'running' ? timing.deadline : '';
}

export function secondsRemaining(expiresAt: string, now: number): number {
  const deadline = Date.parse(expiresAt);
  if (Number.isNaN(deadline)) return 0;
  return Math.max(0, Math.round((deadline - now) / 1000));
}

export function formatRemaining(seconds: number): string {
  if (seconds <= 0) return 'due now';
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

export interface SubscriptionGroup {
  event_type: string;
  conditions: Record<string, unknown>[];
}

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

const PLAIN_EVENT_NAMES: Record<string, string> = {
  BackgroundBashCompleted: 'background job finished',
  CodingAgentIdled: 'coding agent stopped working',
  ChangeProposed: 'change proposed',
  ChangeApplied: 'change applied',
  ContinuationStarted: 'resumed',
  TriggerStarted: 'trigger fired',
  ChildThreadCompleted: 'child thread returned',
  ChildThreadStopped: 'child thread stopped',
  ChildReportFiled: 'child thread report filed',
  ChildThreadDetached: 'child thread moved to top level',
  MessageHeld: 'message held',
  MissingHardeningDetected: 'hardening needed',
  MergeConflictDetected: 'merge conflict',
  PromptInjected: 'message to the agent',
  CodingAgentPromptSent: 'instructions to the agent',
  ResponseAborted: 'response interrupted',
  ResponseCanceled: 'response canceled',
  EventWaitCanceled: 'stopped waiting',
  McpConsentRequested: 'tool consent requested',
  E2ELockReleased: 'test lock released',
};

const TYPE_WORD = /[A-Z0-9]+(?=[A-Z][a-z])|[A-Z]?[a-z]+|[A-Z0-9]+/g;

export function plainEventName(eventType: string): string {
  const known = PLAIN_EVENT_NAMES[eventType];
  if (known) return known;
  const words = eventType.match(TYPE_WORD);
  if (!words) return eventType;
  return words.map((w) => (w.length > 1 && w === w.toUpperCase() ? w : w.toLowerCase())).join(' ');
}

const PLAIN_EVENT_MEANINGS: Record<string, string> = {
  BackgroundBashCompleted: 'A command the agent started in the background has finished.',
  CodingAgentIdled: 'A coding agent finished its turn and is waiting.',
  ChangeProposed: 'An agent offered a change for you to apply.',
  ChangeApplied: 'A change was merged into the project.',
  ChangeDiscarded: 'A change was thrown away without being merged.',
  ChangeReverted: 'A merged change was undone.',
  ChangeApplyFailed: 'A change could not be merged into the project.',
  ContinuationStarted: 'A reply that had stopped was picked back up.',
  TriggerStarted: 'A trigger ran, on its schedule or on an event.',
  ChildThreadCompleted: 'A child thread finished and reported back.',
  ChildThreadStopped: 'A child thread was stopped before it finished.',
  ChildReportFiled: 'A child thread finished, and its report was filed here without waking this thread.',
  ChildThreadDetached: 'A child thread was moved to top level, so it no longer reports back here.',
  MessageHeld: 'A message waited for your reply before the agent read it.',
  MissingHardeningDetected: 'The change had not passed hardening, its automatic review and test run, yet.',
  MergeConflictDetected: 'The change conflicts with newer changes in the project: a merge conflict.',
  PromptInjected: 'Words handed to the agent while it works, from you, another thread or Lucidos.',
  CodingAgentPromptSent: 'Words Lucidos handed to the coding agent.',
  ResponseAborted: 'Lucidos stopped a reply before it finished.',
  ResponseCanceled: 'You stopped a reply before it finished.',
  EventWaitStarted: 'The agent watches for an event and carries on when it arrives.',
  EventWaitCanceled: 'You told the agent to stop waiting.',
  McpConsentRequested: 'A tool asked for your permission before its first use.',
  E2ELockReleased: 'The end-to-end test run finished, so another can start.',
};

export function plainEventMeaning(eventType: string): string | undefined {
  return PLAIN_EVENT_MEANINGS[eventType];
}

export type EventWaitReason = Extract<EngineReason, { kind: 'event_wait' }>;

export const LEGACY_REENTRY_OPENINGS: [string, EventWaitReason['outcome']][] = [
  ['A subscription you registered has timed out.', 'expired'],
  ['An event you subscribed to has arrived', 'delivered'],
];

export function waitReentryReason(event: StoredEvent): EventWaitReason | undefined {
  if (event.type !== 'PromptInjected') return undefined;
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

const ENGINE_REASON_BY_TYPE: Partial<Record<string, EngineReason>> = {
  MissingHardeningDetected: { kind: 'missing_hardening' },
  MergeConflictDetected: { kind: 'merge_conflict' },
};

export function starterEngineReason(event: StoredEvent): EngineReason | undefined {
  const origin = 'origin' in event ? event.origin : undefined;
  if (origin?.kind === 'engine') return origin.reason;
  return waitReentryReason(event) ?? ENGINE_REASON_BY_TYPE[event.type];
}

export function subscriptionFilterNote(g: SubscriptionGroup): string | undefined {
  const n = g.conditions.length;
  if (n === 0) return undefined;
  return n === 1 ? 'with a condition' : `${n} conditions`;
}

export function waitSubscriptionLabel(g: SubscriptionGroup): string {
  const name = plainEventName(g.event_type);
  const note = subscriptionFilterNote(g);
  return note ? `${name} (${note})` : name;
}

export function describeWaitSubscription(on: EventSubscription[]): string {
  return groupSubscriptions(on).map(waitSubscriptionLabel).join(' or ');
}
