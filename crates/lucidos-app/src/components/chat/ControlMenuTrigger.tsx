/**
 * The composer row's anchor button, a coding agent's `/` menu. The menu draws
 * it, and so does the placeholder `PromptRowControls` shows while the menu's
 * code is on its way (ADR 0436). One definition, so the placeholder is the same
 * box, label and tooltip as the button that replaces it.
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
