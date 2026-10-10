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

import { NOT_READY_COPY, NotReadyStrip, notReadyReason } from '../NotReadyStrip';
import { continueStoppedThread } from '../continueStoppedThread';
import { focusedThreadId, threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import type { CodingAgentChangeState } from '../../../api/threads';
import type { UnproposedReason } from '../../../generated/thread-event-wire';

const REASONS = Object.keys(NOT_READY_COPY) as UnproposedReason[];

type MetaOverrides = NonNullable<NonNullable<Parameters<typeof makeThreadState>[1]>['meta']>;

function codingAgentThread(change: CodingAgentChangeState, meta: MetaOverrides = {}) {
  return makeThreadState('t1', { meta: { channel: 'claude_code', section: 'inbox', codingAgentChangeState: change, ...meta } });
}

function mount(change: CodingAgentChangeState): HTMLElement {
  threadMap.value = new Map([['t1', codingAgentThread(change)]]);
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

  it('is a coding-agent fact only', () => {
    const chat = makeThreadState('t1', { meta: { codingAgentChangeState: { kind: 'unproposed', reason: 'plan_missing' } } });
    expect(notReadyReason(chat)).toBeNull();
  });
});

describe('NotReadyStrip', () => {
  it('shows the label and the hint for each reason', () => {
    for (const reason of REASONS) {
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

  it('never draws a disabled control', () => {
    for (const reason of REASONS) {
      const host = mount({ kind: 'unproposed', reason });
      expect(host.querySelector('[disabled]')).toBeNull();
      render(null, host);
    }
  });

  it('draws nothing for a proposal', () => {
    const host = mount({ kind: 'proposed', requires_restart: false });
    expect(host.querySelector('.not-ready-strip')).toBeNull();
  });
});
