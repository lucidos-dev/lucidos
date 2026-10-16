// @vitest-environment jsdom
/**
 * The strip above the prompt box that says why a thread's work is not ready
 * (ADR 0400). It shows only for unproposed work a turn end withheld, on a
 * resting coding-agent thread, and only a turn that did not finish gets
 * Continue. Nothing in it is a disabled control (ADR 0168).
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render } from 'preact';

vi.mock('../continueStoppedThread', () => ({ continueStoppedThread: vi.fn() }));
vi.mock('../../../api/client', () => ({ hardenThread: vi.fn(() => Promise.resolve()) }));
vi.mock('../../../store/actions/threads', () => ({ focusThreadOrBootstrap: vi.fn() }));

import { NOT_READY_COPY, NotReadyStrip, notReadyLabel } from '../NotReadyStrip';
import { continueStoppedThread } from '../continueStoppedThread';
import { hardenThread } from '../../../api/client';
import { focusThreadOrBootstrap } from '../../../store/actions/threads';
import { focusedThreadId, notReadyReason, threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import type { CodingAgentChangeState } from '../../../api/threads';
import type { UnproposedReason } from '../../../generated/thread-event-wire';
import type { StoredEvent } from '../../../store/thread-events';

const STRIP_REASONS = Object.keys(NOT_READY_COPY) as (keyof typeof NOT_READY_COPY)[];
const REASONS: UnproposedReason[] = [...STRIP_REASONS, 'carried_by_successor'];

type MetaOverrides = NonNullable<NonNullable<Parameters<typeof makeThreadState>[1]>['meta']>;

function codingAgentThread(change: CodingAgentChangeState, meta: MetaOverrides = {}) {
  return makeThreadState('t1', { meta: { channel: 'claude_code', section: 'inbox', codingAgentChangeState: change, ...meta } });
}

function mount(change: CodingAgentChangeState, meta: MetaOverrides = {}): HTMLElement {
  threadMap.value = new Map([['t1', codingAgentThread(change, meta)]]);
  focusedThreadId.value = 't1';
  const host = document.createElement('div');
  document.body.appendChild(host);
  render(<NotReadyStrip />, host);
  return host;
}

afterEach(() => {
  document.body.innerHTML = '';
  threadMap.value = new Map();
  focusedThreadId.value = null;
});

describe('notReadyReason', () => {
  it('names the reason a turn end withheld the work', () => {
    for (const reason of REASONS) {
      expect(notReadyReason(codingAgentThread({ kind: 'unproposed', reason }))).toBe(reason);
    }
  });

  it('says nothing for work no turn end withheld, a proposal, or no work', () => {
    expect(notReadyReason(codingAgentThread({ kind: 'unproposed', reason: null }))).toBeNull();
    expect(notReadyReason(codingAgentThread({ kind: 'proposed', requires_restart: false }))).toBeNull();
    expect(notReadyReason(codingAgentThread({ kind: 'none' }))).toBeNull();
  });

  it('waits while the thread runs or watches an event', () => {
    const withheld: CodingAgentChangeState = { kind: 'unproposed', reason: 'plan_missing' };
    expect(notReadyReason(codingAgentThread(withheld, { status: 'running' }))).toBeNull();
    expect(notReadyReason(codingAgentThread(withheld, { liveEventWaitCount: 1 }))).toBeNull();
  });

  // The reader typed while a question was open, then canceled it. The turn
  // ends unfinished, and the agent starts the queued message a moment later.
  describe('while the agent holds a message it has not read', () => {
    const withheld: CodingAgentChangeState = { kind: 'unproposed', reason: 'turn_incomplete' };
    const message = (text: string, id: string) =>
      ({ type: 'MessageReceived', text, channel: 'claude_code', _eventId: id }) as StoredEvent;
    const read = (id: string) => ({ type: 'CodingAgentInputRead', input_event_id: id }) as StoredEvent;
    const canceled = { type: 'ResponseCanceled', cause: 'user_stop', channel: 'claude_code' } as StoredEvent;
    const idled = { type: 'CodingAgentIdled', channel: 'claude_code' } as StoredEvent;
    const withEvents = (events: StoredEvent[], meta: MetaOverrides = {}) => {
      const thread = codingAgentThread(withheld, meta);
      events.forEach((event, i) => thread.events.set(i + 1, event));
      return thread;
    };
    const queued = [message('start', 'm1'), read('m1'), message('tap on phone', 'm2'), canceled, idled];

    it('says nothing, since the next turn is on its way', () => {
      expect(notReadyReason(withEvents(queued))).toBeNull();
    });

    it('says nothing when an older message was read after it arrived', () => {
      const late = [message('start', 'm1'), message('tap on phone', 'm2'), read('m1'), canceled, idled];
      expect(notReadyReason(withEvents(late))).toBeNull();
    });

    it('names the reason again once the agent has read it', () => {
      expect(notReadyReason(withEvents([...queued, read('m2')]))).toBe('turn_incomplete');
    });

    it('names the reason when the message was taken back', () => {
      const removed = { type: 'QueuedMessageRemoved', removed_message_id: 'm2' } as StoredEvent;
      expect(notReadyReason(withEvents([...queued, removed]))).toBe('turn_incomplete');
    });

    it('says nothing while an older message waits behind a newer one taken back', () => {
      const removed = { type: 'QueuedMessageRemoved', removed_message_id: 'm3' } as StoredEvent;
      expect(notReadyReason(withEvents([...queued, message('and the dates', 'm3'), removed]))).toBeNull();
    });

    it('names the reason when the session failed, since nothing will read it', () => {
      expect(notReadyReason(withEvents(queued, { status: 'failed' }))).toBe('turn_incomplete');
    });

    it('names the reason on history from before read events', () => {
      expect(notReadyReason(withEvents([message('start', 'm1'), canceled, idled]))).toBe('turn_incomplete');
    });

    it('names the reason when a later continuation was read after that history', () => {
      const continued = [message('start', 'm1'), canceled, idled, { type: 'ContinuationRequested', _eventId: 'c1' } as StoredEvent, read('c1'), canceled, idled];
      expect(notReadyReason(withEvents(continued))).toBe('turn_incomplete');
    });
  });

  it('is a coding-agent fact only', () => {
    const chat = makeThreadState('t1', { meta: { codingAgentChangeState: { kind: 'unproposed', reason: 'plan_missing' } } });
    expect(notReadyReason(chat)).toBeNull();
  });
});

describe('NotReadyStrip', () => {
  it('shows the label and the hint for each reason', () => {
    for (const reason of STRIP_REASONS) {
      const host = mount({ kind: 'unproposed', reason });
      const text = host.querySelector('.not-ready-strip')?.textContent ?? '';
      expect(text).toContain(NOT_READY_COPY[reason].label);
      expect(text).toContain(NOT_READY_COPY[reason].hint);
      render(null, host);
    }
  });

  it('offers Continue only for a turn that did not finish', () => {
    for (const reason of REASONS) {
      const host = mount({ kind: 'unproposed', reason });
      const labels = [...host.querySelectorAll('button')].map((b) => b.textContent?.trim());
      expect(labels.includes('Continue')).toBe(reason === 'turn_incomplete');
      render(null, host);
    }
  });

  it('continues the focused thread', () => {
    const host = mount({ kind: 'unproposed', reason: 'turn_incomplete' });
    const cont = [...host.querySelectorAll('button')].find((b) => b.textContent?.trim() === 'Continue');
    cont?.click();
    expect(continueStoppedThread).toHaveBeenCalledWith('t1');
  });

  it('offers Harden only for work that was not hardened', () => {
    for (const reason of REASONS) {
      const host = mount({ kind: 'unproposed', reason });
      const labels = [...host.querySelectorAll('button')].map((b) => b.textContent?.trim());
      expect(labels.includes('Harden')).toBe(reason === 'hardening_missing');
      render(null, host);
    }
  });

  it('hardens the focused thread', () => {
    const host = mount({ kind: 'unproposed', reason: 'hardening_missing' });
    const harden = [...host.querySelectorAll('button')].find((b) => b.textContent?.trim() === 'Harden');
    harden?.click();
    expect(hardenThread).toHaveBeenCalledWith('t1');
  });

  it('never draws a disabled control', () => {
    for (const reason of REASONS) {
      const host = mount({ kind: 'unproposed', reason });
      expect(host.querySelector('[disabled]')).toBeNull();
      render(null, host);
    }
  });

  it('names the parent an integrated sub-thread folds into, and offers nothing to press', () => {
    const host = mount({ kind: 'unproposed', reason: 'integrated_into_parent' }, { parentThreadTitle: 'Fix the drawer' });
    const text = host.querySelector('.not-ready-strip')?.textContent ?? '';
    expect(text).toContain('Integrated into Fix the drawer');
    expect(text).toContain(NOT_READY_COPY.integrated_into_parent.hint);
    const labels = [...host.querySelectorAll('button')].map((b) => b.textContent?.trim());
    expect(labels).not.toContain('Harden');
    expect(labels).not.toContain('Continue');
    expect(labels).not.toContain('Apply');
  });

  it('falls back to its parent in general before the title is known', () => {
    expect(notReadyLabel('integrated_into_parent', undefined)).toBe('Integrated into its parent');
    expect(notReadyLabel('hardening_missing', { parentThreadTitle: 'Fix the drawer' })).toBe(NOT_READY_COPY.hardening_missing.label);
  });

  describe('for a handed-off thread', () => {
    const successor = { thread_id: 's1', title: 'Fix the drawer (continued)' };
    const handedOff = { kind: 'unproposed', reason: 'carried_by_successor' } as const;

    it('draws a framed note with an arrow, not the amber strip', () => {
      const host = mount(handedOff, { successor });
      expect(host.querySelector('.not-ready-strip')).toBeNull();
      const note = host.querySelector('[data-role="continued-here"]');
      expect(note?.textContent).toBe('Continued here');
      expect(note?.querySelector('.continued-here-icon')).not.toBeNull();
    });

    it('opens the successor thread from "here"', () => {
      const host = mount(handedOff, { successor });
      host.querySelector<HTMLButtonElement>('.continued-here-link')?.click();
      expect(focusThreadOrBootstrap).toHaveBeenCalledWith('s1');
    });

    it('says where the work went without a link before the successor is known', () => {
      const host = mount(handedOff);
      expect(host.querySelector('[data-role="continued-here"]')?.textContent).toBe('Continued in a new thread');
      expect(host.querySelector('button')).toBeNull();
    });
  });

  it('draws nothing for a proposal', () => {
    const host = mount({ kind: 'proposed', requires_restart: false });
    expect(host.querySelector('.not-ready-strip')).toBeNull();
  });
});
