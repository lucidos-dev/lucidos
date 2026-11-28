
import type { ComponentChildren } from 'preact';
import type { CodingAgent } from '../../api/types';
import { resolveCodingAgent } from '../../store/composeSelections';
import { keepFocusOnPress } from '../../utils/dom';
import { ClaudeIcon, CodexIcon } from '../shared/icons';
import { CODING_AGENT_NAME } from './agentExitCopy';

export function menuCodingAgent(codingAgent: CodingAgent | null | undefined, threadId?: string, composeThreadId?: string): CodingAgent {
  return codingAgent ?? (threadId ? 'claude-code' : resolveCodingAgent(composeThreadId));
}

export const CODING_AGENT_MENU_FOLD_KEY = 'coding-agent-menu';

export interface CodingAgentMenuProps {
  threadId?: string;
  composeThreadId?: string;
  codingAgent?: CodingAgent | null;
  folded?: boolean;
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
