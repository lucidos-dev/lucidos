// @vitest-environment jsdom
/** Opening a dropdown must not take focus from the field being typed in.
 *
 *  On iOS the trigger's mousedown clears focus, so the keyboard slid away just
 *  as the menu opened. The shell grew under a menu placed for the shrunk
 *  viewport, and the menu jumped to its new place: the reported blink. */
import { afterEach, beforeEach, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { Dropdown } from '../Dropdown';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

const options = [{ value: 'a', label: 'Alpha' }, { value: 'b', label: 'Beta' }];

it('cancels the trigger mousedown, so it cannot move focus', () => {
  act(() => { render(<Dropdown options={options} value="a" onChange={() => {}} />, host); });
  const down = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
  host.querySelector('.dropdown-trigger')!.dispatchEvent(down);
  expect(down.defaultPrevented).toBe(true);
});

it('still opens on the click that follows', () => {
  act(() => { render(<Dropdown options={options} value="a" onChange={() => {}} />, host); });
  const trigger = host.querySelector<HTMLButtonElement>('.dropdown-trigger')!;
  act(() => {
    trigger.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true }));
    trigger.click();
  });
  expect(trigger.getAttribute('aria-expanded')).toBe('true');
});
