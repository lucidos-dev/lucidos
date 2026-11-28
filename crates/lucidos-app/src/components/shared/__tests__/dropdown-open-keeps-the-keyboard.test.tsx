// @vitest-environment jsdom
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
