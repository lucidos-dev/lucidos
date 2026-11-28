// @vitest-environment jsdom
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { TypedOutcome } from '../../../api/client/terminal';

let blocker: 'switch-off' | 'thread' | null = null;
let outcome: Promise<TypedOutcome> = Promise.resolve('typed');
const runTerminalSuggestion = vi.fn((_threadId: string, _command: string) => outcome);
const copyToClipboard = vi.fn();

vi.mock('../../../store/actions/terminal', () => ({
  runTerminalSuggestion,
  terminalBlocker: () => blocker,
  TERMINAL_UNAVAILABLE: 'This thread cannot open a terminal.',
  TERMINAL_SETTINGS_LINK: 'Settings → System → Developer',
}));
vi.mock('../../../utils/clipboard', () => ({ copyToClipboard }));
const openSettingsSubview = vi.fn();
vi.mock('../../../store/actions/menu', () => ({ openSettingsSubview }));

const {
  TerminalSuggestionRow, TERMINAL_BUSY_HINT, TERMINAL_OFF_HINT_LEAD, TERMINAL_OFF_HINT_TAIL,
  TERMINAL_TYPED_HINT,
} = await import('../TerminalSuggestionRow');

const { TERMINAL_SETTINGS_LINK } = await import('../../../store/actions/terminal');

const host = document.createElement('div');
afterEach(() => render(null, host));
beforeEach(() => {
  blocker = null;
  outcome = Promise.resolve('typed');
  runTerminalSuggestion.mockClear();
  copyToClipboard.mockClear();
});

const COMMAND = 'git status --short';

function draw(label?: string) {
  render(<TerminalSuggestionRow threadId="t1" suggestion={{
    type: 'TerminalCommandSuggested',
    suggestion_id: 's1',
    command: COMMAND,
    label,
  }} />, host);
}

function button(name: string): HTMLButtonElement | undefined {
  return [...host.querySelectorAll('button')].find(b => b.textContent === name);
}

it('shows the exact command under its label, with Copy and Run in terminal', () => {
  draw('See what changed');
  expect(host.querySelector('.event-row-subject')?.textContent).toBe('See what changed');
  expect(host.querySelector('.terminal-suggestion-command')?.textContent).toBe(COMMAND);
  expect(button('Copy')).toBeDefined();
  expect(button('Run in terminal')).toBeDefined();
});

it('names an unlabelled suggestion plainly', () => {
  draw();
  expect(host.querySelector('.event-row-subject')?.textContent).toBe('Suggested command');
});

it('tells the user to turn Terminal on while the switch is off, and still offers Copy', () => {
  blocker = 'switch-off';
  draw();
  expect(host.querySelector('.terminal-suggestion-note')?.textContent).toBe(
    `${TERMINAL_OFF_HINT_LEAD}${TERMINAL_SETTINGS_LINK}${TERMINAL_OFF_HINT_TAIL}`,
  );
  expect(button('Run in terminal')).toBeUndefined();
  button('Copy')!.click();
  expect(copyToClipboard).toHaveBeenCalledWith(COMMAND, 'Command copied');
});

it('links the off hint straight to the Developer settings', () => {
  blocker = 'switch-off';
  draw();
  button(TERMINAL_SETTINGS_LINK)!.click();
  expect(openSettingsSubview).toHaveBeenCalledWith('developer');
});

it('explains why the command cannot run on an ineligible thread', () => {
  blocker = 'thread';
  draw();
  expect(host.querySelector('.terminal-suggestion-note')?.textContent).toBe('This thread cannot open a terminal.');
  expect(button('Copy')).toBeDefined();
  expect(button('Run in terminal')).toBeUndefined();
});

it('types the command and says Enter is the user’s to press', async () => {
  draw();
  await act(async () => { button('Run in terminal')!.click(); });
  expect(runTerminalSuggestion).toHaveBeenCalledWith('t1', COMMAND);
  expect(host.textContent).toContain(TERMINAL_TYPED_HINT);
});

it('offers Copy when a program holds the terminal', async () => {
  outcome = Promise.resolve('busy');
  draw();
  await act(async () => { button('Run in terminal')!.click(); });
  expect(host.textContent).toContain(TERMINAL_BUSY_HINT);
  expect(button('Copy')).toBeDefined();
});

it('shows a failure where the row is', async () => {
  outcome = Promise.reject(new Error('no shell'));
  draw();
  await act(async () => { button('Run in terminal')!.click(); });
  expect(host.querySelector('[role="alert"]')?.textContent).toContain('no shell');
});
