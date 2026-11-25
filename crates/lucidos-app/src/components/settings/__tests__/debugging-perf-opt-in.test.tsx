// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { DebuggingSection } from '../DebuggingSection';
import { enginePackaged } from '../../../store/store';
import { PERF_FLAG_KEY, perfRecordingOn } from '../../../utils/perfQueue';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  localStorage.removeItem(PERF_FLAG_KEY);
  enginePackaged.value = null;
});

function perfRow() {
  return host.querySelector('[data-search-anchor="debugging:perf"]') as HTMLElement;
}

function clearButton() {
  return perfRow().querySelector('button[aria-label="Clear stored perf opt-in"]') as HTMLButtonElement | null;
}

it('offers to clear an old opt-in from source and keeps recording on afterwards', () => {
  enginePackaged.value = false;
  localStorage.setItem(PERF_FLAG_KEY, '1');
  act(() => { render(<DebuggingSection />, host); });
  expect(perfRow().textContent).toContain('Always on');
  expect(clearButton()).not.toBeNull();

  act(() => { clearButton()!.click(); });

  expect(localStorage.getItem(PERF_FLAG_KEY)).toBeNull();
  expect(clearButton()).toBeNull();
  expect(perfRow().textContent).toContain('Always on');
  expect(perfRecordingOn()).toBe(true);
});

it('shows no clear control from source when nothing is stored', () => {
  enginePackaged.value = false;
  act(() => { render(<DebuggingSection />, host); });
  expect(perfRow().textContent).toContain('Always on');
  expect(clearButton()).toBeNull();
});

it('keeps the switch, and no clear control, on a packaged install', () => {
  enginePackaged.value = true;
  localStorage.setItem(PERF_FLAG_KEY, '1');
  act(() => { render(<DebuggingSection />, host); });
  expect(clearButton()).toBeNull();
  const toggle = perfRow().querySelector('input[type="checkbox"]') as HTMLInputElement;
  expect(toggle.checked).toBe(true);
});
