/** The two surfaces an event wait gets, and the composer line under them.
 *
 *  The step is the RECORD and the indicator is the LIVE state; the split is
 *  load-bearing rather than decorative, so the tests pin the properties that
 *  make it true: the park never becomes an exchange divider, and the indicator
 *  disappears exactly when nothing is subscribed. The transcript half is
 *  deliberately the LIGHTER of the two now (a step line, not a boxed card),
 *  because the indicator is where the details live. */
import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import type { ComponentChildren, VNode } from 'preact';
import { EventDeliveryBody, eventWaitStepBody } from '../chat-exchange-parts';
import { formatDeliveredPayload } from '../CreateThreadView';
import { eventWaitIndicatorBody } from '../EventWaitPanel';
import * as promptInputHelpers from '../prompt-input-helpers';
import {
  PLACEHOLDER_ANSWERING,
  PLACEHOLDER_FOLLOW_UP,
  promptPlaceholder,
} from '../prompt-input-helpers';
import { isExchangeStartEvent } from '../../../store/thread-events';
import type { EventWaitSummary } from '../../../store/thread-events';
import type { ResponseEvent } from '../../../store/types';

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
  subscription: 'ChangeProposed',
  reason: 'the release build to finish',
  expires_at: '2026-08-06T12:00:00Z',
  state: 'waiting',
  ...over,
});

/** The hookless body, which is what the tests drive. `EventWaitStep` itself owns
 *  one `useSignal` for the jump's pending state, and a component carrying a hook
 *  cannot be invoked as a plain function outside a render. */
const step = (event: Wait, opening = false) =>
  eventWaitStepBody({ event, opening, onOpenMatched: () => {} });

describe('EventWaitStep', () => {
  /** The line says what the agent DID, subject first, the way every other step
   *  in the list does. It is deliberately not a boxed card any more: a park is
   *  one action, and the live details belong on the clock indicator. */
  it('reads as a step naming what is being tracked', () => {
    const tree = step(wait_());
    const el = findByRole(tree, 'event-wait-step');
    expect(el?.props['data-state']).toBe('waiting');
    expect(vnodeText(tree)).toContain('Set up an event wait: the release build to finish');
    expect(vnodeText(tree)).toContain('ChangeProposed');
    // The box is gone, not restyled.
    expect(findByRole(tree, 'event-wait-card')).toBeNull();
    expect(String(el?.props.class)).not.toContain('step-note-card');
  });

  /** The step is the agent SETTING UP the wait, and that finished the moment
   *  the subscription existed. Rendering it in-progress shimmers a row for
   *  however long the thread sleeps (hours, for the release-build case this
   *  fixture is named after) and claims a turn is running when none is. */
  it('reads as a finished action while the wait is still live', () => {
    const el = findByRole(step(wait_()), 'event-wait-step');
    expect(String(el?.props.class)).toContain('success');
    expect(String(el?.props.class)).not.toContain('pending');
    expect(findByClass(step(wait_()), 'running-shimmer')).toBeNull();
  });

  /** A wait that resolved on its own keeps the same subject line and changes
   *  only its outcome and its trailing note, so the eye reads one row rather
   *  than several phrasings. Timeout shares the muted `unfinished` treatment
   *  with a stop (nothing failed, the watch just ended without its event), so
   *  the note is what tells them apart. */
  it.each([
    ['woke', 'success', 'ChangeProposed'],
    ['timed_out', 'unfinished', 'timed out'],
  ] as const)('renders the %s state as an %s step', (state, className, note) => {
    const tree = step(
      wait_({ state, matched_event_type: state === 'woke' ? 'ChangeProposed' : undefined }),
    );
    const el = findByRole(tree, 'event-wait-step');
    expect(el?.props['data-state']).toBe(state);
    expect(String(el?.props.class)).toContain(className);
    expect(vnodeText(tree)).toContain('Set up an event wait:');
    expect(vnodeText(tree)).toContain(note);
  });

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
    const el = findByRole(tree, 'event-wait-step');
    expect(el?.props['data-state']).toBe('canceled');
    expect(String(el?.props.class)).toContain('unfinished');
    expect(vnodeText(tree)).toContain('Stopped waiting: the release build to finish');
    expect(vnodeText(tree)).not.toContain('Set up an event wait');
    expect(vnodeText(tree)).not.toContain('discard');
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
    const tree = step(wait_({ state: 'canceled', reason: '', subscription: '', cause: undefined }));
    expect(vnodeText(tree)).toContain('Stopped waiting for an event');
    expect(vnodeText(tree)).toContain('stopped');
  });

  it('offers a jump to the matched event only when one was recorded', () => {
    const withEvent = step(wait_({
      state: 'woke',
      matched_event_type: 'ChangeProposed',
      matched_event_id: 'evt-1',
    }));
    expect(findByRole(withEvent, 'event-wait-jump')).not.toBeNull();

    const withoutEvent = step(wait_({ state: 'woke', matched_event_type: 'ChangeProposed' }));
    expect(findByRole(withoutEvent, 'event-wait-jump')).toBeNull();
  });

  /** Resolving the matched event's owning thread is a network round-trip in
   *  every case but a same-thread match, which on an iOS PWA over Tailscale is
   *  long enough for the tap to read as dead. The jump says it is working and
   *  goes inert so an impatient second tap cannot start a second navigation. */
  it('reports the jump as pending and refuses a second tap', () => {
    const woke = wait_({
      state: 'woke',
      matched_event_type: 'CodingAgentIdled',
      matched_event_id: 'evt-1',
    });

    const idle = findByRole(step(woke), 'event-wait-jump');
    expect(idle?.props.disabled).toBe(false);
    expect(vnodeText(idle)).toBe('show it');

    const pending = findByRole(step(woke, true), 'event-wait-jump');
    expect(pending?.props.disabled).toBe(true);
    expect(vnodeText(pending)).toBe('opening…');
  });

  /** The park never splits the transcript, and neither does a resolution that
   *  WAKES the thread: an attached delivery resumes the same exchange, so a
   *  boundary would strand the waiting line above it and break the seamless
   *  resume the whole design exists for.
   *
   *  `EventWaitCanceled` is not in this list because a stop is the one
   *  resolution with no wake, so there is no resume for a boundary to break.
   *  A user stop IS a boundary; see `eventWaitStopStartsExchange`. */
  it.each(['EventWaitStarted', 'EventWaitDelivered', 'EventWaitExpired'])(
    'a %s never starts an exchange',
    (type) => {
      expect(isExchangeStartEvent({ type })).toBe(false);
    },
  );

  /** A stop has no wake, so there is no resume for a boundary to break, and the
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

  /** **Source-scan tripwire for the bug this file's step half exists for.**
   *
   *  The row was gated on the "Show steps" toggle, which is off until a user
   *  turns it on, so a parked thread rendered no `[data-role="event-wait-step"]`
   *  at all: the event was in the stream and the class was in the bundle with
   *  nothing on screen. There is no jsdom here, so the render gate cannot be
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

/** The wake body, which is what a detached delivery looks like in the
 *  transcript now. The prose it replaces is still what the MODEL reads; this is
 *  the same delivery addressed to the user. */
describe('EventDeliveryBody', () => {
  it('leads with the event name and keeps the payload folded', () => {
    const tree = EventDeliveryBody({
      eventType: 'CodingAgentIdled',
      payloadJson: '{\n  "has_changes": true\n}',
    });
    expect(vnodeText(tree)).toContain('CodingAgentIdled');
    // A <details> with no `open` prop: the payload is there, not shown.
    const disclosure = findByClass(tree, 'event-delivery-payload');
    expect(disclosure).not.toBeNull();
    expect(disclosure?.props.open).toBeUndefined();
    expect(vnodeText(tree)).toContain('has_changes');
  });

  it('drops the disclosure when there is nothing to expand', () => {
    const tree = EventDeliveryBody({ eventType: 'ReleaseTagged' });
    expect(vnodeText(tree)).toContain('ReleaseTagged');
    expect(findByClass(tree, 'event-delivery-payload')).toBeNull();
  });

  /** A marker event carries `{}`, and a disclosure that opens onto an empty
   *  object is a worse affordance than no disclosure. Unserializable payloads
   *  lose the payload only: the NAME still answers why the thread woke. */
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
