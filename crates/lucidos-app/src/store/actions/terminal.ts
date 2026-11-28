import { openTerminal, typeTerminalCommand, type TypedOutcome } from '../../api/client/terminal';
import { focusedThreadId, panelOverlay, showToast, threadMap } from '../store';
import type { ThreadMeta } from '../thread-events';
import { currentTerminalEnabled } from './preferences';
import { revealContentPane } from './pane';
import { pushNavState } from './navigation';
import { openSettingsSubview } from './menu';

export const TERMINAL_SETTINGS_LINK = 'Settings → System → Developer';
export const TERMINAL_UNAVAILABLE = 'This thread cannot open a terminal.';

export type TerminalBlocker = 'switch-off' | 'thread';

export function threadCanHaveTerminal(meta: ThreadMeta): boolean {
  if (meta.state !== 'active' || meta.section === 'archived') return false;
  return meta.channel !== 'claude_code' || meta.messageCount > 0;
}

export function terminalBlocker(threadId: string | null): TerminalBlocker | null {
  if (!currentTerminalEnabled()) return 'switch-off';
  const meta = threadId ? threadMap.value.get(threadId)?.meta : undefined;
  return meta && threadCanHaveTerminal(meta) ? null : 'thread';
}

export function terminalEntryShown(threadId: string): boolean {
  return terminalBlocker(threadId) === null;
}

export function threadHasLiveTerminal(threadId: string): boolean {
  return !!threadMap.value.get(threadId)?.meta.liveTerminal;
}

export function terminalOpenFor(threadId: string): boolean {
  const overlay = panelOverlay.value;
  return overlay?.type === 'terminal' && overlay.threadId === threadId;
}

let openIntent: string | null = null;

export function takeTerminalOpenIntent(threadId: string): boolean {
  const wanted = openIntent === threadId;
  openIntent = null;
  return wanted;
}

export function openThreadTerminal(threadId: string | null): void {
  switch (terminalBlocker(threadId)) {
    case 'switch-off':
      showToast('Terminal is off.', 'info', {
        action: { label: TERMINAL_SETTINGS_LINK, onClick: () => openSettingsSubview('developer') },
      });
      return;
    case 'thread':
      showToast(threadId ? TERMINAL_UNAVAILABLE : 'Open a thread to use the terminal.');
      return;
  }
  if (threadId) revealThreadTerminal(threadId);
}

function revealThreadTerminal(threadId: string): void {
  openIntent = threadId;
  panelOverlay.value = { type: 'terminal', threadId };
  revealContentPane();
  pushNavState();
}

export function toggleFocusedThreadTerminal(): void {
  const threadId = focusedThreadId.value;
  if (threadId && terminalOpenFor(threadId)) {
    panelOverlay.value = null;
    pushNavState();
    return;
  }
  openThreadTerminal(threadId);
}

const SUGGESTION_OPEN_SIZE = { cols: 80, rows: 24 };

export async function runTerminalSuggestion(threadId: string, command: string): Promise<TypedOutcome> {
  const { terminal_id } = await openTerminal(threadId, SUGGESTION_OPEN_SIZE);
  revealThreadTerminal(threadId);
  return typeTerminalCommand(threadId, terminal_id, command);
}
