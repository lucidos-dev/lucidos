import { ToggleIconButton } from '../shared/ToggleIconButton';
import { TerminalIcon } from '../shared/icons';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import {
  openThreadTerminal, terminalEntryShown, terminalOpenFor, threadHasLiveTerminal,
} from '../../store/actions/terminal';

export function ThreadTerminalButton({ threadId }: { threadId: string }) {
  if (!terminalEntryShown(threadId)) return null;
  const live = threadHasLiveTerminal(threadId);
  const label = live ? 'Terminal (running)' : 'Open terminal';
  return (
    <span class="thread-view-header-actions">
      <ToggleIconButton
        tone="glyph"
        pressed={terminalOpenFor(threadId)}
        class="header-icon thread-terminal-button"
        data-live={live || undefined}
        aria-label={label}
        data-tooltip={tooltipWithShortcut(label, 'toggleThreadTerminal')}
        onClick={() => openThreadTerminal(threadId)}
      >
        <TerminalIcon />
        {live && <span class="thread-terminal-live-dot" aria-hidden="true" />}
      </ToggleIconButton>
    </span>
  );
}
