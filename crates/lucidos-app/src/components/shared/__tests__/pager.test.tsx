// @vitest-environment jsdom
/** The pager names the rows shown and steps a page either way, never below
 *  the first row. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { Pager } from '../Pager';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

function button(name: string): HTMLButtonElement {
  return [...host.querySelectorAll('button')].find((b) => b.textContent === name)!;
}

it('names the rows shown and steps a page either way', () => {
  const onChange = vi.fn();
  act(() => { render(<Pager offset={50} pageSize={50} total={120} hasMore onChange={onChange} />, host); });
  expect(host.querySelector('.pager-info')?.textContent).toBe('51–100 of 120');
  act(() => { button('Next').click(); });
  act(() => { button('Prev').click(); });
  expect(onChange.mock.calls).toEqual([[100], [0]]);
});

it('stops at both ends', () => {
  act(() => { render(<Pager offset={0} pageSize={50} total={30} hasMore={false} onChange={() => {}} />, host); });
  expect(host.querySelector('.pager-info')?.textContent).toBe('1–30 of 30');
  expect(button('Prev').disabled).toBe(true);
  expect(button('Next').disabled).toBe(true);
});
