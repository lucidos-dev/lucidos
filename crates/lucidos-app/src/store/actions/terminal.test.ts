import { describe, it, expect, beforeEach, vi } from 'vitest';
import { focusedThreadId, panelOverlay, threadMap, toasts } from '../store';
import { makeThreadState } from './threads-test-helpers';

const pushNavState = vi.fn();
vi.mock('./navigation', () => ({ pushNavState }));

const revealContentPane = vi.fn();
vi.mock('./pane', () => ({ revealContentPane }));

let terminalEnabled = true;
vi.mock('./preferences', () => ({ currentTerminalEnabled: () => terminalEnabled }));

const openSettingsSubview = vi.fn();
vi.mock('./menu', () => ({ openSettingsSubview }));
vi.mock('../../utils/basePath', async (original) => ({
  ...(await original<typeof import('../../utils/basePath')>()),
  REMOTE_GATEWAY: 'cloud',
}));

const { openTerminal, typeTerminalCommand } = vi.hoisted(() => ({
  openTerminal: vi.fn(async () => ({ terminal_id: 'term-1', cols: 80, rows: 24, attachments: 0, ticket: 'x', expires_in_secs: 30 })),
  typeTerminalCommand: vi.fn(async () => 'typed' as const),
}));
vi.mock('../../api/client/terminal', () => ({ openTerminal, typeTerminalCommand }));

const {
  openThreadTerminal, runTerminalSuggestion, takeTerminalOpenIntent, terminalEntryShown,
  toggleFocusedThreadTerminal,
} = await import('./terminal');

type MetaOverrides = NonNullable<Parameters<typeof makeThreadState>[1]>['meta'];

function seed(meta: MetaOverrides = {}): void {
  threadMap.value = new Map([['t1', makeThreadState('t1', { meta: { state: 'active', section: 'inbox', messageCount: 1, ...meta } })]]);
}

describe('the terminal entry gate', () => {
  beforeEach(() => {
    terminalEnabled = true;
    seed();
  });

  it('shows the entry when the workspace opens through another gateway', () => {
    expect(terminalEntryShown('t1')).toBe(true);
  });

  it('shows the entry on an active thread while the switch is on', () => {
    expect(terminalEntryShown('t1')).toBe(true);
  });

  it('hides every entry while the Developer switch is off', () => {
    terminalEnabled = false;
    expect(terminalEntryShown('t1')).toBe(false);
  });

  it('hides the entry on an archived thread', () => {
    seed({ section: 'archived' });
    expect(terminalEntryShown('t1')).toBe(false);
  });

  it('hides the entry on a draft', () => {
    seed({ state: 'composing' });
    expect(terminalEntryShown('t1')).toBe(false);
  });

  it('waits for a coding-agent thread to have its worktree', () => {
    seed({ channel: 'claude_code', messageCount: 0 });
    expect(terminalEntryShown('t1')).toBe(false);
    seed({ channel: 'claude_code', messageCount: 1 });
    expect(terminalEntryShown('t1')).toBe(true);
  });

  it('hides the entry for an unknown thread', () => {
    expect(terminalEntryShown('missing')).toBe(false);
  });
});

describe('opening the thread terminal', () => {
  beforeEach(() => {
    terminalEnabled = true;
    seed();
    panelOverlay.value = null;
    focusedThreadId.value = 't1';
    pushNavState.mockClear();
    toasts.value = [];
    openSettingsSubview.mockClear();
    revealContentPane.mockClear();
    takeTerminalOpenIntent('t1');
  });

  it('lands the terminal in the content pane and records the step', () => {
    openThreadTerminal('t1');
    expect(panelOverlay.value).toEqual({ type: 'terminal', threadId: 't1' });
    expect(revealContentPane).toHaveBeenCalledTimes(1);
    expect(pushNavState).toHaveBeenCalledTimes(1);
  });

  it('hands the pane a one-shot open intent for that thread only', () => {
    openThreadTerminal('t1');
    expect(takeTerminalOpenIntent('t2')).toBe(false);
    openThreadTerminal('t1');
    expect(takeTerminalOpenIntent('t1')).toBe(true);
    expect(takeTerminalOpenIntent('t1')).toBe(false);
  });

  it('toggles the focused thread terminal open and shut', () => {
    toggleFocusedThreadTerminal();
    expect(panelOverlay.value).toEqual({ type: 'terminal', threadId: 't1' });
    toggleFocusedThreadTerminal();
    expect(panelOverlay.value).toBeNull();
    expect(pushNavState).toHaveBeenCalledTimes(2);
  });

  it('links to Developer settings when the shortcut is blocked by the switch', () => {
    terminalEnabled = false;
    toggleFocusedThreadTerminal();
    expect(panelOverlay.value).toBeNull();
    expect(revealContentPane).not.toHaveBeenCalled();
    const toast = toasts.value.slice(-1)[0];
    expect(toast?.message).toBe('Terminal is off.');
    toast?.action?.onClick();
    expect(openSettingsSubview).toHaveBeenCalledWith('developer');
  });

  it('explains why the menu cannot open a terminal on an archived thread', () => {
    seed({ section: 'archived' });
    openThreadTerminal('t1');
    expect(panelOverlay.value).toBeNull();
    expect(toasts.value.slice(-1)[0]?.message).toBe('This thread cannot open a terminal.');
  });
});

describe('running a terminal suggestion', () => {
  beforeEach(() => {
    seed();
    panelOverlay.value = null;
    openTerminal.mockClear();
    typeTerminalCommand.mockClear();
  });

  it('opens or joins the shell, shows the pane, and types into that shell', async () => {
    await expect(runTerminalSuggestion('t1', 'ls -la')).resolves.toBe('typed');
    expect(openTerminal).toHaveBeenCalledWith('t1', expect.objectContaining({ cols: 80, rows: 24 }));
    expect(panelOverlay.value).toEqual({ type: 'terminal', threadId: 't1' });
    expect(typeTerminalCommand).toHaveBeenCalledWith('t1', 'term-1', 'ls -la');
  });
});
