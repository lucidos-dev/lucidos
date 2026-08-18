/** The two surfaces an event wait gets, and the composer line under them.
 *
 *  The transcript ROW is the RECORD and the indicator is the LIVE state; the
 *  split is load-bearing rather than decorative, so the tests pin the properties
 *  that make it true: the park never becomes an exchange divider, and the
 *  indicator disappears exactly when nothing is subscribed. The transcript half
 *  is deliberately the LIGHTER of the two: an event row carries the record,
 *  and the indicator is where the live countdown and the Stop live.
 *
 *  The row is an **event row** and NOT a step, which is the property most of
 *  these guard. It rendered through `.inline-step` until 2026-08-10 and
 *  therefore put a green success check on a subscription that might sleep for
 *  hours, and ellipsized the reason and the subscription, which are the only two
 *  things it has to say. */
import { describe, expect, it, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import type { ComponentChildren, VNode } from 'preact';
import { eventDeliveryBody, eventWaitRowBody, triggerFiredBody } from '../chat-exchange-parts';
import { formatDeliveredPayload } from '../CreateThreadView';
import { eventWaitIndicatorBody } from '../EventWaitPanel';
import * as promptInputHelpers from '../prompt-input-helpers';
import {
  PLACEHOLDER_ANSWERING,
  PLACEHOLDER_FOLLOW_UP,
  promptPlaceholder,
} from '../prompt-input-helpers';
import { isExchangeStartEvent } from '../../../store/thread-events';
import type { EventWaitSummary, Exchange } from '../../../store/thread-events';
import type { ResponseEvent } from '../../../store/types';

type TriggerStarted = Extract<Exchange['userEvent'], { type: 'TriggerStarted' }>;

interface AnyVNode extends VNode<{ children?: ComponentChildren; [k: string]: unknown }> {}

function vnodeText(n: ComponentChildren): string {
  if (n === null || n === undefined || typeof n === 'boolean') return '';
  if (typeof n === 'string' || typeof n === 'number') return String(n);
  if (Array.isArray(n)) return n.map(vnodeText).join('');
  return vnodeText((n as AnyVNode).props?.children);
}

function findByRole(node: ComponentChildren, role: string): AnyVNode | null {
  if (node === null || node === undefined || typeof node === 'boolean') return null;
  if (typeof node === 'string' || typeof node === 'number') return null;
  if (Array.isArray(node)) {
    for (const c of node) {
      const m = findByRole(c, role);
      if (m) return m;
    }
    return null;
  }
  const v = node as AnyVNode;
  if (v.props?.['data-role'] === role) return v;
  return findByRole(v.props?.children, role);
}

function findByClass(node: ComponentChildren, cls: string): AnyVNode | null {
  if (node === null || node === undefined || typeof node === 'boolean') return null;
  if (typeof node === 'string' || typeof node === 'number') return null;
  if (Array.isArray(node)) {
    for (const c of node) {
      const m = findByClass(c, cls);
      if (m) return m;
    }
    return null;
  }
  const v = node as AnyVNode;
  if (String(v.props?.class ?? '').split(/\s+/).includes(cls)) return v;
  return findByClass(v.props?.children, cls);
}

type Wait = Extract<ResponseEvent, { type: 'event_wait' }>;

const wait_ = (over: Partial<Wait> = {}): Wait => ({
  type: 'event_wait',
  wait_id: 'w1',
  subscriptions: ['ChangeProposed'],
  reason: 'the release build to finish',
  expires_at: '2026-08-06T12:00:00Z',
  state: 'waiting',
  ...over,
});

/** The hookless body, which is what the tests drive. The wait row holds no hook
 *  at all now that the jump has moved to the delivery card, but the split stays: the
 *  family's other three bodies keep theirs, and there is no jsdom here. */
const step = (event: Wait) => eventWaitRowBody({ event });

/** Every class a step row can wear to report an outcome. None may reach an
 *  event row: a marker records a fact, it does not return a verdict. */
const STEP_OUTCOME_CLASSES = ['success', 'error', 'pending', 'unfinished'];

describe('EventWaitRow', () => {
  /** The row says what the agent DID, subject first, and names the event types
   *  it is watching. Contained as a card since 2026-08-10, but a lighter one
   *  than `.step-note-card`: a record is not something you can act on. */
  it('names what is being tracked', () => {
    const tree = step(wait_());
    const el = findByRole(tree, 'event-wait-row');
    expect(el?.props['data-state']).toBe('waiting');
    expect(el?.props['data-kind']).toBe('wait');
    expect(vnodeText(tree)).toContain('Set up an event wait: the release build to finish');
    expect(vnodeText(tree)).toContain('ChangeProposed');
    // A card, but never the affordance card's own class.
    expect(String(el?.props.class)).toContain('event-row');
    expect(String(el?.props.class)).not.toContain('step-note-card');
  });

  /** **The bug this row was rebuilt for.** It rendered as `.inline-step` with a
   *  `.step-icon`, and `waiting` mapped to the `success` outcome, so a live
   *  subscription wore the same green check a finished tool call gets. A marker
   *  reports a fact; only the child-thread row shows a verdict, and that verdict
   *  is the child's. */
  it.each(['waiting', 'matched', 'timed_out', 'canceled'] as const)(
    'wears no step costume in the %s state',
    (state) => {
      const tree = step(wait_({ state }));
      const el = findByRole(tree, 'event-wait-row');
      const classes = String(el?.props.class ?? '').split(/\s+/);
      expect(classes).toContain('event-row');
      expect(classes).not.toContain('inline-step');
      for (const outcome of STEP_OUTCOME_CLASSES) expect(classes).not.toContain(outcome);
      expect(findByClass(tree, 'step-icon')).toBeNull();
      expect(findByClass(tree, 'step-description')).toBeNull();
      expect(findByClass(tree, 'step-detail')).toBeNull();
      // A shimmer would run for however long the thread sleeps and claim a turn
      // was running when none is.
      expect(findByClass(tree, 'running-shimmer')).toBeNull();
    },
  );

  /** Every state reads as a WORD, so the tint only groups it and a colourblind
   *  reader gets the same fact. Timeout and stop are told apart by their words
   *  rather than by red: nothing failed either time. */
  it.each([
    ['waiting', 'waiting', 'live'],
    ['matched', 'matched', 'arrived'],
    ['timed_out', 'timed out', 'lapsed'],
    ['canceled', 'stopped', 'halted'],
  ] as const)('reports the %s state as the word "%s"', (state, word, tone) => {
    const tree = step(wait_({ state }));
    const pill = findByClass(tree, 'event-row-state');
    expect(vnodeText(pill)).toBe(word);
    expect(pill?.props['data-tone']).toBe(tone);
  });

  /** A wait that resolved on its own keeps the same subject line, so the eye
   *  reads one row rather than several phrasings. A match swaps the subscription
   *  list for the event that actually matched: one of the types it was watching
   *  is now a specific thing that happened.
   *
   *  **The word is `matched`, not "woke".** A delivery does not require an idle
   *  thread, and the row cannot tell whether this one had been asleep, so it
   *  states the thing that is true of the subscription in both lanes. */
  it.each([
    ['matched', 'ChangeProposed'],
    ['timed_out', 'timed out'],
  ] as const)('keeps the arming subject in the %s state', (state, note) => {
    const tree = step(
      wait_({ state, matched_event_type: state === 'matched' ? 'ChangeProposed' : undefined }),
    );
    expect(findByRole(tree, 'event-wait-row')?.props['data-state']).toBe(state);
    expect(vnodeText(tree)).toContain('Set up an event wait:');
    expect(vnodeText(tree)).toContain(note);
  });

  /** Each watched type gets its own chip, joined by the word the subscription
   *  language itself uses. Two chips and one glue is ONE fact, so the row's
   *  middot separator steps over the "or" rather than fencing it. */
  it('chips every watched event type', () => {
    const tree = step(wait_({ subscriptions: ['ChangeProposed', 'ChangeApplied'] }));
    expect(vnodeText(tree)).toContain('ChangeProposed');
    expect(vnodeText(tree)).toContain('ChangeApplied');
    expect(vnodeText(tree)).toContain('or');
    expect(vnodeText(findByClass(tree, 'event-name'))).toBe('ChangeProposed');
  });

  /** The deadline is a FACT, not a countdown: ADR 0047's amendment puts the
   *  live countdown on the indicator, and a ticking span here would re-render
   *  inside `ChatExchange` once a second for as long as the thread sleeps. */
  it('states an unresolved wait deadline without ticking', () => {
    expect(vnodeText(step(wait_()))).toContain('until ');
    // A resolved wait has no deadline left to state.
    expect(vnodeText(step(wait_({ state: 'matched' })))).not.toContain('until ');
  });

  /** **An `await_event` timeout runs up to 24 hours**, so a deadline is
   *  routinely tomorrow, and a bare "until 09:15" read this afternoon points at
   *  a time that already passed this morning. Today's deadline stays bare,
   *  because naming the day there is noise.
   *
   *  A fixed clock, because "is this today" is the whole assertion and the real
   *  one moves. Noon UTC is never within a minute of local midnight (that would
   *  need an offset of +11:59, which no zone has), so `+1 minute` is same-day in
   *  every timezone the suite could run in; `+36 hours` is a different day in
   *  all of them, including a 25-hour DST fall-back day. */
  it('names the day on a deadline that is not today', () => {
    vi.useFakeTimers();
    try {
      const now = new Date('2026-08-10T12:00:00.000Z');
      vi.setSystemTime(now);
      const at = (ms: number) => new Date(now.getTime() + ms).toISOString();
      const stamp = (e: string) => {
        const text = vnodeText(step(wait_({ expires_at: e })));
        return text.slice(text.indexOf('until '));
      };

      expect(stamp(at(60 * 1000))).toMatch(/^until \d{2}:\d{2}$/);
      expect(stamp(at(36 * 60 * 60 * 1000))).toMatch(/^until \w{3} \d{1,2} \d{2}:\d{2}$/);
    } finally {
      vi.useRealTimers();
    }
  });

  /** An unparseable or absent deadline is omitted rather than rendered as an
   *  "Invalid Date": a row states no fact its event does not carry. */
  it.each([['', 'absent'], ['not-a-date', 'unparseable']])(
    'omits a %s deadline (%s)',
    (expires) => {
      expect(vnodeText(step(wait_({ expires_at: expires })))).not.toContain('until');
    },
  );

  /** A STOP is a different action at a different moment, so it says so. When
   *  the subscription was armed in an earlier turn, this row IS the stop and
   *  sits where the stop happened; "Set up an event wait" there would name the
   *  wrong event entirely.
   *
   *  The word is "stopped", never "discarded": *discarded* already means
   *  throwing a thing away in Lucidos, and one of the causes literally IS a
   *  discarded thread. */
  it('reads a stop as a stop, not as an arming', () => {
    const tree = step(wait_({ state: 'canceled', cause: 'user_stop' }));
    const el = findByRole(tree, 'event-wait-row');
    expect(el?.props['data-state']).toBe('canceled');
    expect(vnodeText(tree)).toContain('Stopped waiting: the release build to finish');
    expect(vnodeText(tree)).not.toContain('Set up an event wait');
    expect(vnodeText(tree)).not.toContain('discard');
    // The subscription is over, so the row names how it ended instead of what
    // it was watching. Naming both would read as a live watch.
    expect(vnodeText(tree)).not.toContain('ChangeProposed');
  });

  /** **A label never says "waiting" twice.** Both subjects here carry the verb,
   *  and `reason` is the model's free text, which reaches for a gerund as often
   *  as a noun phrase. An arm-then-stand-down then printed the same sentence on
   *  two cards, each opening `wait: waiting for`.
   *
   *  Fixed at the label rather than by trusting the guidance, so it holds for
   *  every reason already on disk. `awaitedSubject` carries the rule and its
   *  edges; these two cases pin that both subjects route through it. */
  it.each([
    ['waiting', 'Set up an event wait: the e2e lock to free up'],
    ['canceled', 'Stopped waiting: the e2e lock to free up'],
  ] as const)('drops the duplicated verb on a %s row', (state, subject) => {
    const tree = step(wait_({ state, reason: 'waiting for the e2e lock to free up' }));
    expect(vnodeText(tree)).toContain(subject);
    expect(vnodeText(tree)).not.toContain('waiting for the e2e lock');
  });

  /** Every cause reads as what the person actually did, so a stand-down the
   *  agent performed is not reported as the user pressing a button. */
  it.each([
    ['user_stop', 'stopped from the panel'],
    ['agent_stand_down', 'stood down'],
    ['thread_archived', 'stopped by archiving'],
    ['thread_discarded', 'stopped by discarding the thread'],
    ['thread_canceled', 'stopped by a thread Stop'],
  ] as const)('names %s as "%s"', (cause, note) => {
    expect(vnodeText(step(wait_({ state: 'canceled', cause })))).toContain(note);
  });

  /** A pre-2026-08-07 `EventWaitCanceled` carries neither a cause nor what it
   *  stopped. It still has to render, and it says the one thing it knows rather
   *  than an empty "Set up an event wait: ". */
  it('renders a legacy stop that knows neither cause nor subscription', () => {
    const tree = step(wait_({
      state: 'canceled', reason: '', subscriptions: [], expires_at: '', cause: undefined,
    }));
    expect(vnodeText(tree)).toContain('Stopped waiting for an event');
    expect(vnodeText(tree)).toContain('stopped');
    // Nothing invented to fill the gaps, and no dangling separator either.
    expect(findByClass(tree, 'event-name')).toBeNull();
    expect(findByClass(tree, 'event-row-sep')).toBeNull();
  });

  /** **The arming card carries no jump, by design.** It records the moment the
   *  wait was SET UP, and a link out of it to an event that landed hours later
   *  did not read as belonging to it (reported 2026-08-10). The jump lives on
   *  the delivery card, which IS the arrival. Naming the matched TYPE stays
   *  here, since that says how this wait ended. */
  it('links nowhere, even once it has a matched event', () => {
    const matched = step(wait_({
      state: 'matched',
      matched_event_type: 'ChangeProposed',
      matched_event_id: 'evt-1',
    }));
    expect(findByRole(matched, 'event-wait-jump')).toBeNull();
    // The chip carries the jump on the two rows that HAVE one, so its link
    // variant is what would show up here if one grew back.
    expect(findByClass(matched, 'event-name-link')).toBeNull();
    expect(vnodeText(matched)).not.toContain('Go to event');
    // The matched type is still named.
    expect(vnodeText(findByClass(matched, 'event-name'))).toBe('ChangeProposed');
  });

  /** The park never splits the transcript, and neither does a resolution that
   *  RE-ENTERS the thread: an attached delivery resumes the same exchange, so a
   *  boundary would strand the waiting line above it and break the seamless
   *  resume the whole design exists for.
   *
   *  `EventWaitCanceled` is not in this list because a stop is the one
   *  resolution that re-enters nothing, so there is no resume for a boundary to
   *  break.
   *  A user stop IS a boundary; see `eventWaitStopStartsExchange`. */
  it.each(['EventWaitStarted', 'EventWaitDelivered', 'EventWaitExpired'])(
    'a %s never starts an exchange',
    (type) => {
      expect(isExchangeStartEvent({ type })).toBe(false);
    },
  );

  /** A stop re-enters nothing, so there is no resume for a boundary to break, and the
   *  user's own stop is a thing they did to the thread at a moment. Every other
   *  cause is somebody acting inside a turn and stays a step there. */
  it.each([
    ['user_stop', true],
    ['agent_stand_down', false],
    ['thread_archived', false],
    ['thread_discarded', false],
  ] as const)('a %s cancel starts an exchange: %s', (cause, starts) => {
    expect(isExchangeStartEvent({ type: 'EventWaitCanceled', cause })).toBe(starts);
  });

  /** **Source-scan tripwire for the bug this file's transcript half exists for.**
   *
   *  The row was gated on the "Show steps" toggle, which was off until a user
   *  turned it on, so a parked thread rendered no `[data-role="event-wait-row"]`
   *  at all: the event was in the stream and the class was in the bundle with
   *  nothing on screen. The toggle defaults ON since 2026-08-11, which changes
   *  nothing here: it is the reader's, so it can still be off. There is no jsdom here, so the render gate cannot be
   *  driven; the line itself is what gets pinned. `'step'` is read alongside it
   *  so a scan that stopped matching anything would fail rather than pass. */
  it('renders the row without consulting the Show steps toggle', () => {
    const here: string = dirname(fileURLToPath(import.meta.url));
    const src: string = readFileSync(resolve(here, '../ChatExchange.tsx'), 'utf8');
    const arm = (kind: string) =>
      src
        .split('\n')
        .find((l: string) => l.includes(`evt.type === '${kind}'`) && l.includes('return <'));

    expect(arm('event_wait')).toBeDefined();
    expect(arm('event_wait')).not.toContain('showSteps');
    expect(arm('step')).toContain('showSteps');
  });
});

/** The delivery body, which is what an arrived subscription event looks like in
 *  the transcript. The prose it replaces is still what the MODEL reads; this is
 *  the same delivery addressed to the user, and since 2026-08-10 it is the same
 *  event row the wait above it uses. */
describe('eventDeliveryBody', () => {
  /** No handler by default: a delivery whose event has nowhere to open is the
   *  ordinary case, not the exception. `linked` opts into the jump. */
  const delivery = (over: Partial<Parameters<typeof eventDeliveryBody>[0]> = {}) =>
    eventDeliveryBody({ eventType: 'CodingAgentIdled', opening: false, ...over });
  const linked = (over: Partial<Parameters<typeof eventDeliveryBody>[0]> = {}) =>
    delivery({ onOpenMatched: () => {}, ...over });

  it('leads with the event name and keeps the payload folded', () => {
    const tree = delivery({ payloadJson: '{\n  "has_changes": true\n}' });
    expect(vnodeText(tree)).toContain('CodingAgentIdled');
    // A <details> with no `open` prop: the payload is there, not shown.
    const disclosure = findByClass(tree, 'event-row-fold');
    expect(disclosure).not.toBeNull();
    expect(disclosure?.props.open).toBeUndefined();
    expect(vnodeText(tree)).toContain('Payload');
    expect(vnodeText(tree)).toContain('has_changes');
  });

  it('drops the disclosure when there is nothing to expand', () => {
    const tree = delivery({ eventType: 'ReleaseTagged' });
    expect(vnodeText(tree)).toContain('ReleaseTagged');
    expect(findByClass(tree, 'event-row-fold')).toBeNull();
  });

  /** Same marker as the wait, the child callback and the trigger: that shared
   *  shape is the whole point, so the row's identity is pinned rather than left
   *  to whichever kind gets edited next. The event name goes through the ONE
   *  chip atom, so a subscription and a delivery spell it the same way. */
  it('is an event row, with the name in the shared chip', () => {
    const tree = delivery({ eventType: 'ChangeProposed' });
    const el = findByRole(tree, 'event-delivery');
    expect(String(el?.props.class)).toContain('event-row');
    expect(el?.props['data-kind']).toBe('delivery');
    expect(vnodeText(findByClass(tree, 'event-name'))).toBe('ChangeProposed');
    expect(vnodeText(findByClass(tree, 'event-row-state'))).toBe('delivered');
  });

  /** The arming reason lives on the `EventWaitStarted`, which is routinely
   *  outside the loaded window by the time the delivery lands. A row states no
   *  fact its own event carries, so the card names the event and stops. */
  it('claims no arming reason it cannot see', () => {
    const tree = delivery({ eventType: 'ChangeProposed' });
    expect(vnodeText(tree)).not.toContain('Set up an event wait');
    expect(vnodeText(tree)).not.toContain('waiting');
  });

  /** **The card claims nothing about the thread's prior state**, which is the
   *  same rule one step further: a delivery does not know whether the thread was
   *  asleep. `await_event` does not hold the turn, so a wait routinely resolves
   *  while an unrelated turn is running, and the engine then injects the
   *  delivery into that live loop and tells the MODEL it arrived "while you were
   *  working". The anchor starts an exchange either way, so this card is drawn
   *  and its words printed whichever lane it took.
   *
   *  It read "Woke on <type>" until 2026-08-13, which was that claim. Pinned
   *  over every event-wait surface rather than only this one, because "woke" is
   *  the natural word for the mechanism when you are looking at the engine
   *  instead of at one thread, and it is the arming row's state pill that would
   *  quietly reintroduce it. See
   *  `docs/plans/2026-08-13-a-delivery-does-not-know-the-thread-was-asleep.md`. */
  it('says the event arrived, never that the thread woke', () => {
    expect(vnodeText(delivery({ eventType: 'ChangeProposed' })))
      .toContain('Event arrived: ChangeProposed');
    const surfaces = [
      delivery(),
      linked(),
      ...(['waiting', 'matched', 'timed_out', 'canceled'] as const)
        .map((state) => step(wait_({ state, matched_event_type: 'ChangeProposed' }))),
    ];
    for (const tree of surfaces) expect(vnodeText(tree)).not.toMatch(/\bwok|\bwake/i);
  });

  /** **This card owns the jump**, moved here from the arming card on
   *  2026-08-10: this one IS the arrival, so a link out of it goes to the thing
   *  that arrived, where a link out of "Set up an event wait" pointed at
   *  something that happened hours after the moment that card records.
   *
   *  **And the event's NAME is that jump.** It was a separate "Go to event"
   *  link on the facts line, which asked the card to be read twice for one
   *  destination while the chip right above it already named that destination.
   *  The name is the only text the link could have had, so it is the link. */
  it('makes the event name itself the jump', () => {
    const tree = linked({ eventType: 'ChangeProposed' });
    const jump = findByRole(tree, 'event-delivery-jump');

    expect(jump?.type).toBe('button');
    expect(vnodeText(jump)).toBe('ChangeProposed');
    expect(String(jump?.props.class)).toContain('event-name');
    // Nothing else on the card claims to be the way there.
    expect(vnodeText(tree)).not.toContain('Go to event');
  });

  /** **The bug, as the card renders it** (reported 2026-08-10). A delivery of a
   *  `BackgroundBashCompleted` had nowhere to go, and the link went anyway: it
   *  pulsed the unrelated question card that happened to start the turn the
   *  completion landed in. No target now means no affordance, so the dead tap
   *  is unreachable rather than merely unlikely.
   *
   *  It is also what a delivery with no recorded `event_id` renders, and what
   *  the card shows while the answer is still being resolved. */
  it('leaves the event name inert when there is nowhere to go', () => {
    const tree = delivery({ eventType: 'BackgroundBashCompleted' });

    // Still named: the NAME is the answer to "why did this thread start
    // talking again", whether or not it can be opened.
    expect(vnodeText(findByClass(tree, 'event-name'))).toBe('BackgroundBashCompleted');
    expect(findByRole(tree, 'event-delivery-jump')).toBeNull();
    expect(findByClass(tree, 'event-name-link')).toBeNull();
    expect(vnodeText(tree)).not.toContain('Go to event');
  });

  /** A real `<button>`, not a `<code>` carrying an onClick, so it is reachable
   *  by keyboard and announces itself. Its accessible name says where it goes:
   *  the visible text is a bare event type, which says only what the event is. */
  it('is a keyboard-reachable control that says where it goes', () => {
    const jump = findByRole(linked({ eventType: 'ChangeProposed' }), 'event-delivery-jump');

    expect(jump?.type).toBe('button');
    expect(jump?.props.type).toBe('button');
    expect(jump?.props['aria-label']).toBe('Go to the ChangeProposed event');
  });

  /** Resolving the matched event's owning thread is a network round-trip in
   *  every case but a same-thread match, which on an iOS PWA over Tailscale is
   *  long enough for the tap to read as dead. The chip goes inert so an
   *  impatient second tap cannot start a second navigation.
   *
   *  It keeps its NAME while it works, rather than swapping in an "Opening…"
   *  caption the way the old link did: the name is a fact the row states, not a
   *  button label, and replacing it would delete the answer to "what arrived
   *  here" for as long as the navigation takes. */
  it('reports the jump as pending and refuses a second tap', () => {
    const idle = findByRole(linked(), 'event-delivery-jump');
    expect(idle?.props.disabled).toBe(false);
    expect(idle?.props['aria-busy']).toBeUndefined();

    const pending = findByRole(linked({ opening: true }), 'event-delivery-jump');
    expect(pending?.props.disabled).toBe(true);
    expect(pending?.props['aria-busy']).toBe('true');
    expect(vnodeText(pending)).toBe('CodingAgentIdled');
  });

  /** A marker event carries `{}`, and a disclosure that opens onto an empty
   *  object is a worse affordance than no disclosure. Unserializable payloads
   *  lose the payload only: the NAME still answers what arrived. */
  it.each([
    ['null', null],
    ['undefined', undefined],
    ['an empty object', {}],
  ])('formats %s as nothing worth expanding', (_label, payload) => {
    expect(formatDeliveredPayload(payload)).toBeUndefined();
  });

  it('pretty-prints a real payload', () => {
    expect(formatDeliveredPayload({ has_changes: true })).toBe('{\n  "has_changes": true\n}');
  });

  it('drops a payload it cannot serialize rather than the whole body', () => {
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    expect(formatDeliveredPayload(cyclic)).toBeUndefined();
  });
});

/** The fourth kind on the shared row. A trigger fire is the same thing the other
 *  three report (something outside the thread happened), and it used to render
 *  its whole prompt as markdown above every response it produced. */
describe('triggerFiredBody', () => {
  /** No handler by default. On THIS row that is the everyday case rather than
   *  the exception: a trigger fires on a workspace domain event, which belongs
   *  to no conversation and has no transcript to open. */
  const fired = (over: Record<string, unknown> = {}, opening = false) =>
    triggerFiredBody({
      event: { type: 'TriggerStarted', trigger_id: 't1', ...over } as TriggerStarted,
      opening,
    });
  const firedLinked = (over: Record<string, unknown> = {}, opening = false) =>
    triggerFiredBody({
      event: { type: 'TriggerStarted', trigger_id: 't1', ...over } as TriggerStarted,
      opening,
      onOpenMatched: () => {},
    });

  it('is an event row naming the trigger, with the prompt folded away', () => {
    const tree = fired({ trigger_name: 'Nightly release check', prompt: 'Check the build.' });
    const el = findByRole(tree, 'trigger-fired');
    expect(String(el?.props.class)).toContain('event-row');
    expect(el?.props['data-kind']).toBe('trigger');
    expect(vnodeText(tree)).toContain('Trigger fired: Nightly release check');
    expect(vnodeText(findByClass(tree, 'event-row-state'))).toBe('fired');
    const fold = findByClass(tree, 'event-row-fold');
    expect(fold?.props.open).toBeUndefined();
    expect(vnodeText(fold)).toContain('Prompt');
  });

  /** **`TriggerStarted` carries no cron expression**, only
   *  `invocation: { kind: 'Schedule' }`. So the row says it was scheduled and
   *  stops, rather than inventing a schedule to look complete. */
  it('says a scheduled run is scheduled, and names no schedule', () => {
    const tree = fired({ trigger_name: 'Nightly', invocation: { kind: 'Schedule' } });
    expect(vnodeText(tree)).toContain('scheduled');
    expect(findByClass(tree, 'event-name')).toBeNull();
    expect(findByRole(tree, 'trigger-event-jump')).toBeNull();
  });

  /** An event-driven run DOES carry its type, so it gets the same chip a wait's
   *  subscription and a delivery's matched event get.
   *
   *  **Whether that chip is also the jump is decided upstream**, by whether the
   *  matched event turns out to live in a conversation at all (`eventHasTarget`).
   *  On this row it usually does not: a trigger fires on a workspace domain
   *  event, so the old separate "Go to event" link here was very often a
   *  guaranteed toast. The chip is named either way; only its clickability
   *  moves. */
  const eventFire = { kind: 'Event', event_type: 'ChangeApplied', event_id: 'e1' };

  it('chips the matched event type whether or not it can be opened', () => {
    const plain = fired({ invocation: eventFire });
    expect(vnodeText(findByClass(plain, 'event-name'))).toBe('ChangeApplied');
    expect(findByRole(plain, 'trigger-event-jump')).toBeNull();
    expect(findByClass(plain, 'event-name-link')).toBeNull();

    const linkable = firedLinked({ invocation: eventFire });
    expect(vnodeText(findByClass(linkable, 'event-name'))).toBe('ChangeApplied');
    expect(findByRole(linkable, 'trigger-event-jump')).not.toBeNull();
  });

  /** The same chip atom the delivery card uses, so one event type is spelled one
   *  way wherever it appears, and the jump behaves identically on both rows. */
  it('makes the chip the jump, with no separate link beside it', () => {
    const tree = firedLinked({ invocation: eventFire });
    const jump = findByRole(tree, 'trigger-event-jump');

    expect(jump?.type).toBe('button');
    expect(vnodeText(jump)).toBe('ChangeApplied');
    expect(jump?.props['aria-label']).toBe('Go to the ChangeApplied event');
    expect(vnodeText(tree)).not.toContain('Go to event');
  });

  it('reports the jump as pending and refuses a second tap', () => {
    const pending = findByRole(firedLinked({ invocation: eventFire }, true), 'trigger-event-jump');
    expect(pending?.props.disabled).toBe(true);
    expect(vnodeText(pending)).toBe('ChangeApplied');
  });

  /** A legacy row carries neither a name nor a prompt. It still renders, saying
   *  the one thing it knows, and never falls back to the trigger's uuid: no
   *  screen in Lucidos is labelled with one. */
  it('renders a trigger that knows neither its name nor its prompt', () => {
    const tree = fired();
    expect(vnodeText(tree)).toContain('Trigger fired');
    expect(vnodeText(tree)).not.toContain('t1');
    expect(vnodeText(tree)).not.toContain(':');
    expect(findByClass(tree, 'event-row-fold')).toBeNull();
  });
});

describe('EventWaitIndicator', () => {
  const wait = (over: Partial<EventWaitSummary> = {}): EventWaitSummary => ({
    wait_id: 'w1',
    on: [{ event_type: 'ChangeProposed' }],
    reason: 'waiting for the release build',
    expires_at: '2026-08-06T12:00:00Z',
    ...over,
  });

  it('renders nothing when the thread holds no subscription', () => {
    expect(eventWaitIndicatorBody({ waits: [], onClick: () => {} })).toBeNull();
  });

  /** ONE state. A subscribed thread is idle and there is nothing for the user
   *  to do, so the indicator reports presence rather than a mode. It carried a
   *  `parked` / `watching` split while a wait could hold a turn; ADR 0049
   *  removed that, and a reintroduced `data-state` here would be that
   *  distinction coming back. */
  it('reports presence with no mode of its own', () => {
    const rendered = eventWaitIndicatorBody({ waits: [wait()], onClick: () => {} });
    const button = findByRole(rendered, 'event-wait-indicator');
    expect(button).not.toBeNull();
    expect(button?.props['data-state']).toBeUndefined();
    expect(button?.props['aria-label']).toContain('Watching for an event');
  });

  it('names the single wait, and counts them when there are several', () => {
    const one = eventWaitIndicatorBody({ waits: [wait()], onClick: () => {} });
    expect(findByRole(one, 'event-wait-indicator')?.props['data-tooltip']).toBe(
      'waiting for the release build',
    );

    const many = eventWaitIndicatorBody({
      waits: [wait(), wait({ wait_id: 'w2' })],
      onClick: () => {},
    });
    expect(findByRole(many, 'event-wait-indicator')?.props['data-tooltip']).toBe('2 subscriptions');
  });
});

/** A subscription gets NO placeholder of its own. The thread is idle, the
 *  composer stays fully enabled, and a message runs an ordinary turn while the
 *  subscription keeps watching: that is the ordinary follow-up promise, so the
 *  ordinary follow-up line already says it. The subscription's state lives on
 *  the indicator, which is the surface the user opens for it. These pin that no
 *  third string grows back here. */
describe('the composer while subscribed', () => {
  it('reads exactly like any other focused thread', () => {
    expect(promptPlaceholder(true, false)).toBe(PLACEHOLDER_FOLLOW_UP);
  });

  it('still yields to a pending question', () => {
    expect(promptPlaceholder(true, true)).toBe(PLACEHOLDER_ANSWERING);
  });

  it('exposes no parked variant to reach for', () => {
    expect(Object.keys(promptInputHelpers).filter((k) => k.startsWith('PLACEHOLDER_'))).toEqual(
      expect.not.arrayContaining(['PLACEHOLDER_PARKED']),
    );
  });
});
