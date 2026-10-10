// @vitest-environment jsdom
/** An `Agent` row folds its sub-agent's steps under it. Folded, a running agent
 *  shows its latest step on one line beneath it. Plan:
 *  `docs/plans/2026-10-06-fold-sub-agent-steps-under-agent-row.md`. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { withScrollAnchor } from '../CreateThreadView';
import { SubAgentStepGroup } from '../chat-exchange-parts';

vi.mock('../CreateThreadView', () => ({
  withScrollAnchor: vi.fn((_anchor: Element | null, fn: () => void) => fn()),
}));
import { openSubAgentGroups } from '../../../store/store';
import type { ResponseEvent, StepOutcome } from '../../../store/types';

type StepEvent = Extract<ResponseEvent, { type: 'step' }>;

const child = (id: string, outcome: StepOutcome): StepEvent => ({
  type: 'step',
  description: `Run ${id}`,
  outcome,
  tool_use_id: id,
  parent_tool_use_id: 'agent-a',
});

const agent = (outcome: StepOutcome, children: StepEvent[]): StepEvent => ({
  type: 'step',
  description: 'Correctness review',
  tool_name: 'Agent',
  outcome,
  tool_use_id: 'agent-a',
  children,
});

let host: HTMLElement;
beforeEach(() => {
  openSubAgentGroups.value = new Set();
  vi.mocked(withScrollAnchor).mockClear();
  host = document.createElement('div');
  document.body.appendChild(host);
});
afterEach(() => {
  render(null, host);
  host.remove();
});

const draw = (event: StepEvent) => act(() => { render(<SubAgentStepGroup event={event} />, host); });
const tail = () => host.querySelector('[data-role="sub-agent-tail"]')?.textContent ?? null;
const listed = () => [...host.querySelectorAll('[data-role="sub-agent-steps"] .step-description')].map(e => e.textContent);
const toggle = () => host.querySelector<HTMLButtonElement>('[data-role="step-fold"]')!;

describe('SubAgentStepGroup', () => {
  it('folded and running: shows the step count and the latest step as the tail', () => {
    draw(agent('pending', [child('a1', 'success'), child('a2', 'pending')]));

    expect(host.querySelector('.step-fold-count')?.textContent).toBe('2 steps');
    expect(tail()).toContain('Run a2');
    expect(listed()).toEqual([]);
    expect(toggle().getAttribute('aria-expanded')).toBe('false');
  });

  it('ended: the tail says how the agent ended, with its outcome mark', () => {
    const ending = () => host.querySelector('[data-role="sub-agent-tail"] .inline-step')!;

    draw(agent('success', [child('a1', 'success')]));
    expect(tail()).toBe('Done');
    expect(ending().classList.contains('success')).toBe(true);
    expect(ending().querySelector('.step-outcome-icon-success')).not.toBeNull();
    expect(host.querySelector('.step-fold-count')?.textContent).toBe('1 step');

    draw(agent('error', [child('a1', 'success')]));
    expect(tail()).toBe('Failed');
    expect(ending().classList.contains('error')).toBe(true);

    draw(agent('unfinished', [child('a1', 'unfinished')]));
    expect(tail()).toBe('Did not finish');
  });

  it('ended and unfolded: the list shows the steps, so there is no tail', () => {
    draw(agent('success', [child('a1', 'success')]));

    act(() => { toggle().click(); });

    expect(tail()).toBeNull();
    expect(listed()).toEqual(['Run a1']);
  });

  it('unfolded: lists every step in order and drops the tail', () => {
    draw(agent('pending', [child('a1', 'success'), child('a2', 'pending')]));

    act(() => { toggle().click(); });

    expect(listed()).toEqual(['Run a1', 'Run a2']);
    expect(toggle().getAttribute('aria-expanded')).toBe('true');
    expect(openSubAgentGroups.value.has('agent-a')).toBe(true);
  });

  // A reader following a running thread must not be scrolled by the growth.
  it('both fold targets hold the pressed control still', () => {
    draw(agent('pending', [child('a1', 'success')]));
    const count = host.querySelector<HTMLButtonElement>('.step-fold-count')!;

    act(() => { toggle().click(); });
    act(() => { count.click(); });

    expect(vi.mocked(withScrollAnchor).mock.calls.map(([anchor]) => anchor)).toEqual([toggle(), count]);
  });

  it('the agent row keeps its own description and opens no outcome mark', () => {
    draw(agent('success', [child('a1', 'success')]));

    const row = host.querySelector('.inline-step.has-fold')!;
    expect(row.querySelector('.step-description')?.textContent).toBe('Correctness review');
    expect(row.querySelector('.step-icon')).toBeNull();
  });
});
