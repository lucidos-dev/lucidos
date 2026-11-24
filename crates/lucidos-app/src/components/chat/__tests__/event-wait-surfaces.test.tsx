// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { render, type ComponentChildren, type VNode } from 'preact';
import { act } from 'preact/test-utils';
import { WAIT_PILL, boundaryCard, eventDeliveryBody, eventWaitRowBody, triggerFiredBody } from '../chat-exchange-parts';
import { formatDeliveredPayload } from '../CreateThreadView';
import { EventConditionPopover } from '../EventConditionPopover';
import { EventRowFoldView } from '../EventRow';
import { waitingIndicatorBody } from '../WaitingPanel';
import * as promptInputHelpers from '../prompt-input-helpers';
import {
  PLACEHOLDER_ANSWERING,
  PLACEHOLDER_FOLLOW_UP,
  promptPlaceholder,
} from '../prompt-input-helpers';
import { eventConditionPopover } from '../../../store/store';
import { isExchangeStartEvent } from '../../../store/thread-events';
import { responseBody } from '../../../store/event-rendering';
import type { EventWaitSummary, Exchange, StoredEvent } from '../../../store/thread-events';
import type { ResponseEvent } from '../../../store/types';
import { formatMessageTimestamp } from '../../../utils/formatTime';

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

function findFold(node: ComponentChildren): AnyVNode | null {
  if (node === null || node === undefined || typeof node === 'boolean') return null;
  if (typeof node === 'string' || typeof node === 'number') return null;
  if (Array.isArray(node)) {
    for (const c of node) {
      const m = findFold(c);
      if (m) return m;
    }
    return null;
  }
  const v = node as AnyVNode;
  if ((v.type as unknown) === EventRowFoldView) return v;
  return findFold(v.props?.children);
}

type Wait = Extract<ResponseEvent, { type: 'event_wait' }>;

const wait_ = (over: Partial<Wait> = {}): Wait => ({
  type: 'event_wait',
  wait_id: 'w1',
  subscriptions: [{ event_type: 'ChangeProposed' }],
  reason: 'the release build to finish',
  created: '2026-08-06T11:00:00Z',
  expires_at: '2026-08-06T12:00:00Z',
  state: 'waiting',
  ...over,
});

const step = (event: Wait) => eventWaitRowBody({ event });

const press = (chip: AnyVNode | null, anchor: HTMLElement) =>
  (chip?.props.onClick as (e: { currentTarget: HTMLElement }) => void)({ currentTarget: anchor });

const STEP_OUTCOME_CLASSES = ['success', 'error', 'pending', 'unfinished'];

describe('EventWaitRow', () => {
  afterEach(() => { eventConditionPopover.value = null; });

  it('names what is being tracked', () => {
    const tree = step(wait_());
    const el = findByRole(tree, 'event-wait-row');
    expect(el?.props['data-state']).toBe('waiting');
    expect(el?.props['data-kind']).toBe('wait');
    expect(vnodeText(findByClass(tree, 'event-row-subject'))).toBe(WAIT_PILL);
    expect(vnodeText(findByClass(tree, 'event-row-detail'))).toBe('The release build to finish');
    expect(vnodeText(findByClass(tree, 'event-row-meta'))).toBe('watching forchange proposed');
    expect(String(el?.props.class)).toContain('event-row');
    expect(String(el?.props.class)).not.toContain('step-note-card');
  });

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
      expect(findByClass(tree, 'running-shimmer')).toBeNull();
    },
  );

  it.each([
    ['waiting', /^Until /, 'live'],
    ['matched', /^Arrived$/, 'arrived'],
    ['timed_out', /^Gave up at /, 'lapsed'],
    ['canceled', /^Stopped$/, 'halted'],
  ] as const)('reports the %s state as %s', (state, words, tone) => {
    const tree = step(wait_({ state }));
    const pill = findByClass(tree, 'event-row-state');
    expect(vnodeText(pill)).toMatch(words);
    expect(pill?.props['data-tone']).toBe(tone);
  });

  it.each([
    ['waiting', 'Waiting'],
    ['timed_out', 'Gave up'],
  ] as const)('falls back to "%s" words with no deadline', (state, word) => {
    const pill = findByClass(step(wait_({ state, expires_at: '' })), 'event-row-state');
    expect(vnodeText(pill)).toBe(word);
  });

  it('stamps a delivered wait with when it was done', () => {
    vi.useFakeTimers();
    try {
      const now = new Date('2026-08-10T12:00:00.000Z');
      vi.setSystemTime(now);
      const at = (ms: number) => new Date(now.getTime() + ms).toISOString();
      const stamp = (e: string) =>
        vnodeText(findByClass(step(wait_({ state: 'matched', matched_at: e })), 'event-row-state'));

      expect(stamp(at(-60 * 1000))).toMatch(/^Arrived \d{2}:\d{2}$/);
      expect(stamp(at(-36 * 60 * 60 * 1000))).toMatch(/^Arrived \w{3} \d{1,2} \d{2}:\d{2}$/);
    } finally {
      vi.useRealTimers();
    }
  });

  it('stamps the row with when the wait started', () => {
    const time = findByClass(step(wait_()), 'event-row-time');
    expect(time?.props.dateTime).toBe('2026-08-06T11:00:00Z');
    expect(vnodeText(time)).toBe(formatMessageTimestamp('2026-08-06T11:00:00Z'));
  });

  it('puts the start time above the card, not inside it', () => {
    const tree = step(wait_());
    const card = findByRole(tree, 'event-wait-row');
    expect(findByClass(card, 'event-row-time')).toBeNull();
    const siblings = (tree as AnyVNode).props.children as AnyVNode[];
    expect(siblings.map((c) => c.props['data-role'] ?? c.props.class)).toEqual([
      'event-row-time',
      'event-wait-row',
    ]);
  });

  it('omits the start time when it cannot be read', () => {
    expect(findByClass(step(wait_({ created: '' })), 'event-row-time')).toBeNull();
  });

  it('states the deadline in the pill and nowhere else', () => {
    const tree = step(wait_());
    expect(vnodeText(findByClass(tree, 'event-row-state'))).toMatch(/^Until /);
    expect(vnodeText(findByClass(tree, 'event-row-meta'))).not.toMatch(/until/i);
  });

  it.each(['matched', 'timed_out', 'canceled'] as const)('keeps one layout in the %s state', (state) => {
    const tree = step(wait_({ state }));
    expect(findByRole(tree, 'event-wait-row')?.props['data-state']).toBe(state);
    expect(vnodeText(findByClass(tree, 'event-row-subject'))).toBe(WAIT_PILL);
    expect(vnodeText(findByClass(tree, 'event-row-detail'))).toBe('The release build to finish');
    expect(vnodeText(tree)).not.toContain('change proposed');
    expect(findByClass(tree, 'event-row-meta')).toBeNull();
  });

  it('heads the card with a Waiting for pill', () => {
    const pill = findByClass(findByClass(step(wait_()), 'event-row-subject'), 'event-name');
    expect(pill?.type).toBe('code');
    expect(vnodeText(pill)).toBe(WAIT_PILL);
    expect(pill?.props['data-tooltip']).toBe('The agent watches for an event and carries on when it arrives.');
  });

  it.each(['waiting', 'matched', 'timed_out', 'canceled'] as const)(
    'says neither "event wait" nor "subscription" in the %s state',
    (state) => {
      const text = vnodeText(step(wait_({ state, cause: 'user_stop' })));
      expect(text).not.toMatch(/event wait|subscription/i);
    },
  );

  it('keeps a gerund reason as written', () => {
    const detail = findByClass(step(wait_({ reason: 'watching the deploy' })), 'event-row-detail');
    expect(vnodeText(detail)).toBe('Watching the deploy');
  });

  it('chips every watched event type', () => {
    const tree = step(
      wait_({ subscriptions: [{ event_type: 'ChangeProposed' }, { event_type: 'ChangeApplied' }] }),
    );
    expect(vnodeText(findByClass(tree, 'event-row-meta'))).toBe('watching forchange proposedorchange applied');
    expect(findByClass(tree, 'event-row-sep')).toBeNull();
  });

  it('shows the plain name with its meaning on the tooltip', () => {
    const chip = findByClass(findByClass(step(wait_()), 'event-row-meta'), 'event-name');
    expect(vnodeText(chip)).toBe('change proposed');
    expect(chip?.props['data-tooltip']).toBe('An agent offered a change for you to apply.');
  });

  it('opens the condition at a filtered chip', () => {
    const condition = { 'workflow_run.event': 'completed' };
    const tree = step(wait_({ subscriptions: [{ event_type: 'GithubWorkflowRunStateChanged', condition }] }));
    const chip = findByClass(tree, 'event-name-link');
    expect(vnodeText(chip)).toBe('github workflow run state changed with a condition');
    expect(vnodeText(findByClass(chip, 'event-name-note'))).toBe('with a condition');
    expect(chip?.props['aria-label']).toBe('github workflow run state changed · show the condition');
    const anchor = document.createElement('button');
    press(chip, anchor);
    expect(eventConditionPopover.value).toEqual({
      eventType: 'GithubWorkflowRunStateChanged',
      conditions: [condition],
      anchor,
      waitId: 'w1',
    });
  });

  it('closes the condition on a second press of the same chip', () => {
    const wait = wait_({ subscriptions: [{ event_type: 'ChangeProposed', condition: { repo: 'r' } }] });
    press(findByClass(step(wait), 'event-name-link'), document.createElement('button'));
    press(findByClass(step(wait), 'event-name-link'), document.createElement('button'));
    expect(eventConditionPopover.value).toBeNull();
  });

  it('marks the chip as the toggle of its popover', () => {
    const wait = wait_({ subscriptions: [{ event_type: 'ChangeProposed', condition: { repo: 'r' } }] });
    const shut = findByClass(step(wait), 'event-name-link');
    expect(shut?.props['aria-haspopup']).toBe('dialog');
    expect(shut?.props['aria-expanded']).toBe(false);
    press(shut, document.createElement('button'));
    expect(findByClass(step(wait), 'event-name-link')?.props['aria-expanded']).toBe(true);
    const other = findByClass(step({ ...wait, wait_id: 'w2' }), 'event-name-link');
    expect(other?.props['aria-expanded']).toBe(false);
  });

  it('folds repeats of one event type into a single chip', () => {
    const conditions = [1, 2, 3, 4, 5, 6].map((n) => ({ thread_id: `t${n}` }));
    const tree = step(wait_({
      subscriptions: conditions.map((condition) => ({ event_type: 'CodingAgentIdled', condition })),
    }));
    const chip = findByClass(tree, 'event-name-link');
    expect(vnodeText(chip)).toBe('coding agent stopped working 6 conditions');
    expect(vnodeText(findByClass(tree, 'event-row-meta'))).toBe('watching forcoding agent stopped working 6 conditions');
    expect(chip?.props['aria-label']).toBe('coding agent stopped working · show the 6 conditions');
    const anchor = document.createElement('button');
    press(chip, anchor);
    expect(eventConditionPopover.value).toEqual({ eventType: 'CodingAgentIdled', conditions, anchor, waitId: 'w1' });
  });

  it('shows the condition as a type and payload block, in a popover at the chip', () => {
    const anchor = document.createElement('button');
    document.body.appendChild(anchor);
    const host = document.createElement('div');
    document.body.appendChild(host);
    eventConditionPopover.value = { eventType: 'CodingAgentIdled', conditions: [{ thread_id: 't1' }], anchor, waitId: 'w1' };
    act(() => { render(<EventConditionPopover />, host); });
    try {
      const panel = document.querySelector<HTMLElement>('[data-role="event-condition-popover"]');
      expect(panel?.classList.contains('anchored-popover')).toBe(true);
      const subject = panel?.querySelector<HTMLElement>('[data-role="event-condition-type"]');
      expect(subject?.textContent).toBe('CodingAgentIdled');
      expect(subject?.dataset.tooltip).toBe('coding agent stopped working');
      expect(panel?.querySelector('[data-role="event-condition-json"]')?.textContent).toBe(
        'type: CodingAgentIdled\npayload: {\n  "thread_id": "t1"\n}',
      );
    } finally {
      act(() => { render(null, host); });
      host.remove();
      anchor.remove();
    }
  });

  it('leaves an unfiltered chip inert', () => {
    const tree = step(wait_({ subscriptions: [{ event_type: 'ChangeProposed' }] }));
    expect(findByClass(tree, 'event-name-link')).toBeNull();
    expect(findByClass(findByClass(tree, 'event-row-meta'), 'event-name')?.type).toBe('code');
  });

  it('states an unresolved wait deadline without ticking', () => {
    expect(vnodeText(step(wait_()))).toContain('Until ');
    expect(vnodeText(step(wait_({ state: 'matched' })))).not.toMatch(/until /i);
  });

  it('names the day on a deadline that is not today', () => {
    vi.useFakeTimers();
    try {
      const now = new Date('2026-08-10T12:00:00.000Z');
      vi.setSystemTime(now);
      const at = (ms: number) => new Date(now.getTime() + ms).toISOString();
      const stamp = (e: string) =>
        vnodeText(findByClass(step(wait_({ expires_at: e })), 'event-row-state'));

      expect(stamp(at(60 * 1000))).toMatch(/^Until \d{2}:\d{2}$/);
      expect(stamp(at(36 * 60 * 60 * 1000))).toMatch(/^Until \w{3} \d{1,2} \d{2}:\d{2}$/);
    } finally {
      vi.useRealTimers();
    }
  });

  it.each([['', 'absent'], ['not-a-date', 'unparseable']])(
    'omits a %s deadline (%s)',
    (expires) => {
      expect(vnodeText(step(wait_({ expires_at: expires })))).not.toMatch(/until/i);
    },
  );

  it('reads a stop as a stop', () => {
    const tree = step(wait_({ state: 'canceled', cause: 'user_stop' }));
    const el = findByRole(tree, 'event-wait-row');
    expect(el?.props['data-state']).toBe('canceled');
    expect(vnodeText(findByClass(tree, 'event-row-state'))).toBe('Stopped');
    expect(vnodeText(findByClass(tree, 'event-row-meta'))).toBe('you stopped it');
    expect(vnodeText(tree)).not.toContain('discard');
  });

  it.each(['waiting', 'canceled'] as const)('drops the duplicated verb on a %s row', (state) => {
    const tree = step(wait_({ state, reason: 'waiting for the e2e lock to free up' }));
    expect(vnodeText(findByClass(tree, 'event-row-detail'))).toBe('The e2e lock to free up');
  });

  it.each([
    ['user_stop', 'you stopped it'],
    ['agent_stand_down', 'the agent stopped it'],
    ['thread_archived', 'stopped when the thread was archived'],
    ['thread_discarded', 'stopped when the thread was discarded'],
    ['thread_canceled', 'stopped by Stop'],
    ['snapshot_restored', 'stopped by a workspace restore'],
  ] as const)('names %s as "%s"', (cause, note) => {
    expect(vnodeText(step(wait_({ state: 'canceled', cause })))).toContain(note);
  });

  it('adds no note for an unknown cause', () => {
    const tree = step(wait_({ state: 'canceled', cause: 'unknown' }));
    expect(findByClass(tree, 'event-row-meta')).toBeNull();
  });

  it('renders a legacy stop that knows neither cause nor subscription', () => {
    const tree = step(wait_({
      state: 'canceled', reason: '', subscriptions: [], expires_at: '', cause: undefined,
    }));
    expect(vnodeText(findByClass(tree, 'event-row-detail'))).toBe('An event');
    expect(vnodeText(findByClass(tree, 'event-row-state'))).toBe('Stopped');
    expect(findByClass(tree, 'event-row-meta')).toBeNull();
    expect(findByClass(tree, 'event-row-sep')).toBeNull();
  });

  it('links nowhere, even once it has a matched event', () => {
    const matched = step(wait_({ state: 'matched' }));
    expect(findByRole(matched, 'event-wait-jump')).toBeNull();
    expect(findByClass(matched, 'event-name-link')).toBeNull();
    expect(vnodeText(matched)).not.toContain('Go to event');
    expect(vnodeText(matched)).not.toContain('change proposed');
  });

  it.each(['EventWaitStarted', 'EventWaitDelivered', 'EventWaitExpired'])(
    'a %s never starts an exchange',
    (type) => {
      expect(isExchangeStartEvent({ type })).toBe(false);
    },
  );

  it.each([
    ['user_stop', true],
    ['agent_stand_down', false],
    ['thread_archived', false],
    ['thread_discarded', false],
  ] as const)('a %s cancel starts an exchange: %s', (cause, starts) => {
    expect(isExchangeStartEvent({ type: 'EventWaitCanceled', cause })).toBe(starts);
  });

  it('renders the row without consulting the Show steps toggle', () => {
    const wait: ResponseEvent = {
      type: 'event_wait',
      wait_id: 'w1',
      subscriptions: [{ event_type: 'ChangeApplied' }],
      reason: 'waiting for the apply',
      created: '2026-08-10T11:00:00Z',
      expires_at: '2026-08-10T12:00:00Z',
      state: 'waiting',
    };
    const events: ResponseEvent[] = [{ type: 'step', description: 'Read file', outcome: 'success' }, wait];
    for (const showSteps of [true, false]) {
      for (const showDetails of [true, false]) {
        const rows = responseBody(events, { showSteps, showDetails, rowsHidden: 0 }).flatMap(s => s.rows);
        expect(rows.find(r => r.kind === 'marker'), `${showSteps}/${showDetails}`).toMatchObject({ event: wait });
      }
    }
    const [steps] = responseBody(events, { showSteps: false, showDetails: true, rowsHidden: 0 })[0].rows;
    expect(steps).toMatchObject({ kind: 'steps', open: false });
  });
});

describe('eventDeliveryBody', () => {
  const delivery = (over: Partial<Parameters<typeof eventDeliveryBody>[0]> = {}) =>
    eventDeliveryBody({ eventType: 'CodingAgentIdled', opening: false, ...over });
  const linked = (over: Partial<Parameters<typeof eventDeliveryBody>[0]> = {}) =>
    delivery({ onOpenMatched: () => {}, ...over });

  it('leads with the event name and keeps the payload folded', () => {
    const tree = delivery({ payloadJson: '{\n  "has_changes": true\n}' });
    expect(vnodeText(tree)).toContain('Coding agent stopped working');
    const fold = findFold(tree);
    expect(fold).not.toBeNull();
    expect(fold?.props.open).toBeUndefined();
    expect(fold?.props.label).toBe('Details');
    expect(vnodeText(fold?.props.body as ComponentChildren)).toContain('has_changes');
  });

  it('drops the disclosure when there is nothing to expand', () => {
    const tree = delivery({ eventType: 'ReleaseTagged' });
    expect(vnodeText(tree)).toContain('Release tagged');
    expect(findFold(tree)).toBeNull();
  });

  it('is an event row, with the name in the shared chip', () => {
    const tree = delivery({ eventType: 'ChangeProposed' });
    const el = findByRole(tree, 'event-delivery');
    expect(String(el?.props.class)).toContain('event-row');
    expect(el?.props['data-kind']).toBe('delivery');
    expect(vnodeText(findByClass(tree, 'event-name'))).toBe('Change proposed');
    expect(findByClass(tree, 'event-name')?.props['data-tooltip']).toBe('An agent offered a change for you to apply.');
    expect(vnodeText(findByClass(tree, 'event-row-state'))).toBe('Arrived');
  });

  it('claims no arming reason it cannot see', () => {
    const tree = delivery({ eventType: 'ChangeProposed' });
    expect(vnodeText(tree)).not.toContain('Waiting for');
    expect(vnodeText(tree)).not.toContain('waiting');
  });

  it('says the event arrived, never that the thread woke', () => {
    expect(vnodeText(delivery({ eventType: 'ChangeProposed' }))).toBe('Change proposedArrived');
    const surfaces = [
      delivery(),
      linked(),
      ...(['waiting', 'matched', 'timed_out', 'canceled'] as const)
        .map((state) => step(wait_({ state }))),
    ];
    for (const tree of surfaces) expect(vnodeText(tree)).not.toMatch(/wok|wake/i);
    for (const outcome of ['delivered', 'expired'] as const) {
      const anchor = {
        type: 'PromptInjected', text: 'x', mode: 'agent',
        origin: { kind: 'engine', reason: { kind: 'event_wait', outcome, watched: ['ChangeProposed'], wait_reason: 'r' } },
      } as StoredEvent;
      expect(JSON.stringify(boundaryCard(anchor, '', { turn: 'done', carried: 'x' }))).not.toMatch(/wok|wake/i);
    }
  });

  it('makes the event name itself the jump', () => {
    const tree = linked({ eventType: 'ChangeProposed' });
    const jump = findByRole(tree, 'event-delivery-jump');

    expect(jump?.type).toBe('button');
    expect(vnodeText(jump)).toBe('Change proposed');
    expect(String(jump?.props.class)).toContain('event-name');
    expect(vnodeText(tree)).not.toContain('Go to event');
  });

  it('leaves the event name inert when there is nowhere to go', () => {
    const tree = delivery({ eventType: 'BackgroundBashCompleted' });

    expect(vnodeText(findByClass(tree, 'event-name'))).toBe('Background job finished');
    expect(findByRole(tree, 'event-delivery-jump')).toBeNull();
    expect(findByClass(tree, 'event-name-link')).toBeNull();
    expect(vnodeText(tree)).not.toContain('Go to event');
  });

  it('is a keyboard-reachable control that says where it goes', () => {
    const jump = findByRole(linked({ eventType: 'ChangeProposed' }), 'event-delivery-jump');

    expect(jump?.type).toBe('button');
    expect(jump?.props.type).toBe('button');
    expect(jump?.props['aria-label']).toBe('Change proposed · go to the event');
  });

  it('reports the jump as pending and refuses a second tap', () => {
    const idle = findByRole(linked(), 'event-delivery-jump');
    expect(idle?.props.disabled).toBe(false);
    expect(idle?.props['aria-busy']).toBeUndefined();

    const pending = findByRole(linked({ opening: true }), 'event-delivery-jump');
    expect(pending?.props.disabled).toBe(true);
    expect(pending?.props['aria-busy']).toBe('true');
    expect(vnodeText(pending)).toBe('Coding agent stopped working');
  });

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

describe('triggerFiredBody', () => {
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
    expect(vnodeText(findByClass(tree, 'event-row-subject'))).toBe('Trigger fired');
    expect(vnodeText(findByClass(tree, 'event-row-meta'))).toContain('Nightly release check');
    expect(vnodeText(findByClass(tree, 'event-row-state'))).toBe('Fired');
    const fold = findFold(tree);
    expect(fold?.props.open).toBeUndefined();
    expect(fold?.props.label).toBe('Details');
  });

  it('says a scheduled run is scheduled, and names no schedule', () => {
    const tree = fired({ trigger_name: 'Nightly', invocation: { kind: 'Schedule' } });
    expect(vnodeText(findByClass(tree, 'event-row-state'))).toBe('Scheduled');
    expect(findByClass(findByClass(tree, 'event-row-meta'), 'event-name')).toBeNull();
    expect(findByRole(tree, 'trigger-event-jump')).toBeNull();
  });

  const eventFire = { kind: 'Event', event_type: 'ChangeApplied', event_id: 'e1' };

  it('chips the matched event type whether or not it can be opened', () => {
    const plain = fired({ invocation: eventFire });
    expect(vnodeText(findByClass(findByClass(plain, 'event-row-meta'), 'event-name'))).toBe('change applied');
    expect(findByRole(plain, 'trigger-event-jump')).toBeNull();
    expect(findByClass(plain, 'event-name-link')).toBeNull();

    const linkable = firedLinked({ invocation: eventFire });
    expect(vnodeText(findByClass(findByClass(linkable, 'event-row-meta'), 'event-name'))).toBe('change applied');
    expect(findByRole(linkable, 'trigger-event-jump')).not.toBeNull();
  });

  it('makes the chip the jump, with no separate link beside it', () => {
    const tree = firedLinked({ invocation: eventFire });
    const jump = findByRole(tree, 'trigger-event-jump');

    expect(jump?.type).toBe('button');
    expect(vnodeText(jump)).toBe('change applied');
    expect(jump?.props['aria-label']).toBe('change applied · go to the event');
    expect(vnodeText(tree)).not.toContain('Go to event');
  });

  it('reports the jump as pending and refuses a second tap', () => {
    const pending = findByRole(firedLinked({ invocation: eventFire }, true), 'trigger-event-jump');
    expect(pending?.props.disabled).toBe(true);
    expect(vnodeText(pending)).toBe('change applied');
  });

  it('renders a trigger that knows neither its name nor its prompt', () => {
    const tree = fired();
    expect(vnodeText(tree)).toContain('Trigger fired');
    expect(vnodeText(tree)).not.toContain('t1');
    expect(vnodeText(tree)).not.toContain(':');
    expect(findFold(tree)).toBeNull();
  });
});

describe('the waiting indicator, on a subscription', () => {
  const wait = (over: Partial<EventWaitSummary> = {}): EventWaitSummary => ({
    wait_id: 'w1',
    on: [{ event_type: 'ChangeProposed' }],
    reason: 'waiting for the release build',
    expires_at: '2026-08-06T12:00:00Z',
    ...over,
  });
  const noSubThreads = { threads: [], unresolved: 0 };
  const body = (waits: EventWaitSummary[]) =>
    waitingIndicatorBody({ waits, subThreads: noSubThreads, onClick: () => {} });

  it('renders nothing when the thread holds no subscription', () => {
    expect(body([])).toBeNull();
  });

  it('reports presence with no mode of its own', () => {
    const button = findByRole(body([wait()]), 'waiting-indicator');
    expect(button).not.toBeNull();
    expect(button?.props['data-state']).toBeUndefined();
    expect(button?.props['aria-label']).toContain('Waiting for');
  });

  it('names the single wait, and counts them when there are several', () => {
    expect(findByRole(body([wait()]), 'waiting-indicator')?.props['data-tooltip']).toBe(
      'waiting for the release build',
    );

    const many = body([wait(), wait({ wait_id: 'w2' })]);
    expect(findByRole(many, 'waiting-indicator')?.props['data-tooltip']).toBe('2 events');
  });
});

describe('the composer while subscribed', () => {
  it('reads exactly like any other focused thread', () => {
    expect(promptPlaceholder(true, false, false)).toBe(PLACEHOLDER_FOLLOW_UP);
  });

  it('still yields to a pending question', () => {
    expect(promptPlaceholder(true, true, false)).toBe(PLACEHOLDER_ANSWERING);
  });

  it('exposes no parked variant to reach for', () => {
    expect(Object.keys(promptInputHelpers).filter((k) => k.startsWith('PLACEHOLDER_'))).toEqual(
      expect.not.arrayContaining(['PLACEHOLDER_PARKED']),
    );
  });
});
