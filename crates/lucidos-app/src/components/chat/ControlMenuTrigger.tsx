/**
 * The button of a coding agent's `/` menu, the composer row's leftmost member.
 * The menu draws it, and so does the placeholder shown while the menu's code is
 * on its way (ADR 0436). One definition, so the placeholder is the same box,
 * label and tooltip as the button that replaces it.
 */

import type { ComponentChildren } from 'preact';
import type { CodingAgent } from '../../api/types';
import { resolveCodingAgent } from '../../store/composeSelections';
import { keepFocusOnPress } from '../../utils/dom';
import { ClaudeIcon, CodexIcon } from '../shared/icons';
import { CODING_AGENT_NAME } from './agentExitCopy';

/** The backend a coding-agent menu serves: the one its row passes, else a
 *  live thread's Claude Code, else what the compose draft picked. */
export function menuCodingAgent(codingAgent: CodingAgent | null | undefined, threadId?: string, composeThreadId?: string): CodingAgent {
  return codingAgent ?? (threadId ? 'claude-code' : resolveCodingAgent(composeThreadId));
}

/** The menu's name in the row's fold, which remembers its width. */
export const CODING_AGENT_MENU_FOLD_KEY = 'coding-agent-menu';

/** What the menu receives from the composer row. */
export interface CodingAgentMenuProps {
  threadId?: string;
  /** The focused COMPOSING draft's id, set only while composing (mutually
   *  exclusive with `threadId`, which is the active-session id). The commands
   *  are scoped to this draft's repo. */
  composeThreadId?: string;
  codingAgent?: CodingAgent | null;
  /** True once the row has folded the menu into the ⋯: it then draws no box
   *  and opens its panel against the ⋯. */
  folded?: boolean;
  /** The row's attributes for the button, so the row can measure and fold it. */
  rowAttrs?: Record<string, string>;
}

export function codingAgentMenuLabel(agent: CodingAgent): string {
  return `${CODING_AGENT_NAME[agent]} controls`;
}

export function ControlMenuTrigger({ label, extraClass, onClick, children }: {
  label: string;
  extraClass?: string;
  onClick: (e: MouseEvent) => void;
  children: ComponentChildren;
}) {
  return (
    <button
      class={`icon-btn header-icon commands-btn${extraClass ? ` ${extraClass}` : ''}`}
      data-tooltip={label}
      aria-label={label}
      onMouseDown={keepFocusOnPress}
      onClick={onClick}
    >
      {children}
    </button>
  );
}

export function CodingAgentIcon({ agent }: { agent: CodingAgent }) {
  return agent === 'claude-code' ? <ClaudeIcon /> : <CodexIcon />;
}
