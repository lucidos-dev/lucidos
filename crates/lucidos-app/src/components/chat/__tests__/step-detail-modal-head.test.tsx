// @vitest-environment jsdom
/** The step details modal keeps its head to one short line on any width: the
 *  outcome mark, the outcome word and the time (ADR 0290). The step itself opens
 *  the body. It sits over the Conversation pane, not across the pane divider. */
import { afterEach, beforeEach, expect, it } from 'vitest';
import { render } from 'preact';
import { StepDetailModal } from '../StepDetailModal';
import { stepDetailModal } from '../../../store/store';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  stepDetailModal.value = null;
  document.querySelector('.split-layout')?.remove();
});

function openStep(full: string | null = 'cd /home/user/project && make test 2>&1 | tail -4'): HTMLElement {
  stepDetailModal.value = {
    type: 'step',
    description: 'Running: cd /home/user/project && make test',
    outcome: 'success',
    full: full ?? undefined,
    result: 'all tests passed',
  };
  render(<StepDetailModal />, host);
  return document.querySelector<HTMLElement>('[data-role="step-detail-modal"]')!;
}

it('titles the head with the outcome, led by its mark', () => {
  const modal = openStep();
  expect(modal.querySelector('.surface-title')?.textContent).toBe('Completed');
  expect(modal.querySelector('[data-role="step-detail-icon"]')?.classList.contains('success')).toBe(true);
});

it('opens the body on the full command rather than repeating the description', () => {
  const body = openStep().querySelector('.surface-body')!;
  expect(body.textContent).not.toContain('Running:');
  expect(body.firstElementChild?.textContent).toBe('cd /home/user/project && make test 2>&1 | tail -4');
});

it('opens the body on the description when there is no full command', () => {
  const body = openStep(null).querySelector('.surface-body')!;
  expect(body.querySelector('.step-detail-full')).toBeNull();
  expect(body.firstElementChild?.textContent).toBe('Running: cd /home/user/project && make test');
});

it('centres over the Conversation pane when there is one', () => {
  const layout = document.createElement('div');
  layout.className = 'split-layout';
  const pane = document.createElement('div');
  pane.className = 'pane pane-thread';
  pane.getBoundingClientRect = () => ({ left: 400, width: 600 }) as DOMRect;
  layout.appendChild(pane);
  document.body.appendChild(layout);
  expect(openStep().style.getPropertyValue('--pane-centre-x')).toBe('700px');
});

it('leaves the panel centred on the window with no Conversation pane', () => {
  expect(openStep().style.getPropertyValue('--pane-centre-x')).toBe('');
});
