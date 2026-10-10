// @vitest-environment jsdom
/** Opening a dropdown must not change the trigger's width.
 *
 *  `▾` and `▴` can differ in width, as they do in the iOS fallback font. So the
 *  chevron keeps one glyph and CSS turns it over.
 *  `dropdown-chevron-single-glyph.test.ts` keeps every other trigger on it. */
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

it('keeps the same chevron glyph open and closed, and marks it open', () => {
  const options = [{ value: 'a', label: 'Alpha' }, { value: 'b', label: 'Beta' }];
  act(() => { render(<Dropdown options={options} value="a" onChange={() => {}} />, host); });
  const chevron = () => host.querySelector('.dropdown-chevron')!;
  const closed = chevron().textContent;
  expect(chevron().classList.contains('open')).toBe(false);

  act(() => { host.querySelector<HTMLButtonElement>('.dropdown-trigger')!.click(); });

  expect(chevron().textContent).toBe(closed);
  expect(chevron().classList.contains('open')).toBe(true);
});
