import { useState } from 'preact/hooks';
import type { ThreadEvent } from '../../generated/thread-event-wire';
import { openSettingsSubview } from '../../store/actions/menu';
import { runTerminalSuggestion, terminalBlocker, TERMINAL_SETTINGS_LINK, TERMINAL_UNAVAILABLE, type TerminalBlocker } from '../../store/actions/terminal';
import { copyToClipboard } from '../../utils/clipboard';
import { errorDetail } from '../../utils/errorDetail';
import { eventRowBody } from './EventRow';

type Suggestion = Extract<ThreadEvent, { type: 'TerminalCommandSuggested' }>;

type RunState =
  | { kind: 'idle' }
  | { kind: 'running' }
  | { kind: 'typed' }
  | { kind: 'busy' }
  | { kind: 'failed'; message: string };

export const TERMINAL_OFF_HINT_LEAD = 'Turn on Terminal in ';
export const TERMINAL_OFF_HINT_TAIL = ' to run this here.';
export const TERMINAL_BUSY_HINT = 'A program is running in the terminal. Copy the command, or wait for the prompt.';
export const TERMINAL_TYPED_HINT = 'Typed at the prompt. Press Enter in the terminal to run it.';

const terminalOffHint = (
  <>
    {TERMINAL_OFF_HINT_LEAD}
    <button type="button" class="accent-link" onClick={() => openSettingsSubview('developer')}>
      {TERMINAL_SETTINGS_LINK}
    </button>
    {TERMINAL_OFF_HINT_TAIL}
  </>
);

function hint(state: RunState, blocker: TerminalBlocker | null) {
  if (blocker === 'switch-off') return terminalOffHint;
  if (blocker === 'thread') return TERMINAL_UNAVAILABLE;
  switch (state.kind) {
    case 'busy': return TERMINAL_BUSY_HINT;
    case 'typed': return TERMINAL_TYPED_HINT;
    case 'failed': return <span role="alert">{state.message}</span>;
    default: return null;
  }
}

export function TerminalSuggestionRow({ suggestion, threadId }: { suggestion: Suggestion; threadId: string }) {
  const [state, setState] = useState<RunState>({ kind: 'idle' });
  const blocker = terminalBlocker(threadId);
  const note = hint(state, blocker);

  async function run() {
    setState({ kind: 'running' });
    try {
      setState({ kind: await runTerminalSuggestion(threadId, suggestion.command) });
    } catch (err) {
      setState({ kind: 'failed', message: `Could not type the command: ${errorDetail(err)}` });
    }
  }

  return eventRowBody({
    kind: 'form',
    role: 'terminal-suggestion',
    subject: suggestion.label ?? 'Suggested command',
    detail: (
      <>
        <code class="terminal-suggestion-command">{suggestion.command}</code>
        {note && <p class="terminal-suggestion-note">{note}</p>}
      </>
    ),
    actions: (
      <>
        <button type="button" class="action-btn action-btn-secondary"
          onClick={() => copyToClipboard(suggestion.command, 'Command copied')}>
          Copy
        </button>
        {blocker === null && (
          <button type="button" class="action-btn" disabled={state.kind === 'running'} onClick={() => void run()}>
            Run in terminal
          </button>
        )}
      </>
    ),
  });
}
