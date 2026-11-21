// @vitest-environment jsdom
/** Settings › Coding agents › Binaries draws one row per agent while the read
 *  is in flight: the names are known, and only what the read decides shimmers. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  getCodingAgentBinaries: () => new Promise(() => {}),
}));

import { CodingAgentBinariesSection } from '../CodingAgentBinariesSection';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';

let host: HTMLDivElement;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => { render(<CodingAgentBinariesSection />, host); });
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.useRealTimers();
});

it('draws nothing in the list before the delay gate opens', () => {
  expect(host.querySelector('.sk-bar')).toBeNull();
  expect(host.querySelector('.list-row')).toBeNull();
});

it('draws a row per agent, names real, once the gate opens', () => {
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  const rows = host.querySelectorAll('.loading-fade-skeleton .list-row');
  expect([...rows].map((r) => r.querySelector('.title')?.textContent)).toEqual(['Claude Code', 'Codex']);
  expect(rows[0]!.querySelector('.list-row-details .sk-bar')).not.toBeNull();
  expect(rows[0]!.querySelector('input')).toBeNull();
  expect(host.textContent).not.toContain('Loading');
});
