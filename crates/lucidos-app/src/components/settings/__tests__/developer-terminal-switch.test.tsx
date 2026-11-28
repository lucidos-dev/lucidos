// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

let enabled = false;
const setTerminalEnabled = vi.fn(async (value: boolean) => { enabled = value; });
vi.mock('../../../store/actions/preferences', () => ({
  currentTerminalEnabled: () => enabled,
  setTerminalEnabled,
}));

const showConfirm = vi.fn();
vi.mock('../../../store/store', () => ({ showConfirm }));

const { DeveloperSection, TERMINAL_WARNING } = await import('../DeveloperSection');

let host: HTMLDivElement;

beforeEach(() => {
  enabled = false;
  setTerminalEnabled.mockClear();
  showConfirm.mockReset();
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

function toggle(): HTMLInputElement {
  return host.querySelector('[data-search-anchor="developer:terminal"] input[type="checkbox"]') as HTMLInputElement;
}

async function flip(): Promise<void> {
  await act(async () => { toggle().click(); });
}

it('is off by default and explains itself behind an explainer', () => {
  act(() => { render(<DeveloperSection />, host); });
  expect(toggle().checked).toBe(false);
  expect(host.querySelector('[data-search-anchor="developer:terminal"]')?.textContent).toContain('Terminal');
  expect(host.textContent).not.toContain(TERMINAL_WARNING);
});

it('asks before turning on, and stays off when the user declines', async () => {
  showConfirm.mockResolvedValue(false);
  act(() => { render(<DeveloperSection />, host); });
  await flip();
  expect(showConfirm).toHaveBeenCalledWith(TERMINAL_WARNING, 'Turn on', expect.objectContaining({ variant: 'default' }));
  expect(setTerminalEnabled).not.toHaveBeenCalled();
  expect(toggle().checked).toBe(false);
});

it('turns on once the user confirms', async () => {
  showConfirm.mockResolvedValue(true);
  act(() => { render(<DeveloperSection />, host); });
  await flip();
  expect(setTerminalEnabled).toHaveBeenCalledWith(true);
});

it('turns off without asking', async () => {
  enabled = true;
  act(() => { render(<DeveloperSection />, host); });
  await flip();
  expect(showConfirm).not.toHaveBeenCalled();
  expect(setTerminalEnabled).toHaveBeenCalledWith(false);
});
