// @vitest-environment jsdom
/** The frontend preview section names no state it has not read. Unknown, its
 *  button waits dimmed; failed, it says so and still offers Start; loaded, it
 *  offers the action. */
import { afterEach, beforeEach, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { FrontendPreviewSection } from '../FrontendPreviewSection';
import { frontendPreview } from '../../../store/actions/frontend-preview';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  frontendPreview.value = { status: 'not-loaded' };
});

function show() {
  act(() => { render(<FrontendPreviewSection threadId="t1" />, host); });
  return {
    row: host.querySelector('.control-preview-row')!,
    button: host.querySelector<HTMLButtonElement>('.control-preview-row button')!,
    hint: host.querySelector('.control-preview-hint')!.textContent,
  };
}

it('holds the button inert while the status is unknown', () => {
  frontendPreview.value = { status: 'loading' };
  const { row, button } = show();
  expect(row.getAttribute('data-state')).toBe('loading');
  expect(button.disabled).toBe(true);
});

it('says a failed read failed, and still lets Start report the real fault', () => {
  frontendPreview.value = { status: 'failed', error: 'engine unreachable' };
  const { button, hint } = show();
  expect(hint).toBe('Could not read the preview status: engine unreachable');
  expect(button.disabled).toBe(false);
});

it('offers Start once the slot reads as free', () => {
  frontendPreview.value = { status: 'loaded', data: { running: false } };
  const { row, button } = show();
  expect(row.getAttribute('data-state')).toBe('loaded');
  expect(button.disabled).toBe(false);
  expect(button.textContent).toBe('Start');
});
