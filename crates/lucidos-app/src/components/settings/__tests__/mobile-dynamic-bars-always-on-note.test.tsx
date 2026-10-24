// @vitest-environment jsdom
/** Settings › Appearance › Dynamic bars: the installed iOS app always runs
 *  dynamic bars, so only there the switch is on, disabled and explained. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const isIOSPwa = vi.hoisted(() => vi.fn(() => false));
vi.mock('../../../utils/platform', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/platform')>()),
  isIOSPwa,
}));
import { MobileDynamicBarsRow } from '../MobileDynamicBarsRow';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

function note() {
  return host.querySelector('[data-testid="dynamic-bars-always-on-note"]');
}

function checkbox() {
  return host.querySelector('input[type="checkbox"]') as HTMLInputElement;
}

it('shows the switch on and disabled, with a note, in the installed iOS app', () => {
  isIOSPwa.mockReturnValue(true);
  act(() => { render(<MobileDynamicBarsRow />, host); });
  expect(checkbox().checked).toBe(true);
  expect(checkbox().disabled).toBe(true);
  expect(note()?.textContent).toContain('Always on in the iOS app');
});

it('leaves the switch live and the note off anywhere else', () => {
  isIOSPwa.mockReturnValue(false);
  act(() => { render(<MobileDynamicBarsRow />, host); });
  expect(checkbox().disabled).toBe(false);
  expect(note()).toBeNull();
});
