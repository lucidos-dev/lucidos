/**
 * The composer row's anchor button. The two control menus draw it, and so does
 * the placeholder `PromptRowControls` shows while a menu's code is on its way
 * (ADR 0436). One definition, so the placeholder is the same box, label and
 * tooltip as the button that replaces it.
 */

import type { ComponentChildren } from 'preact';
import type { CodingAgent } from '../../api/types';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { resolveCodingAgent } from '../../store/composeSelections';
import { LUCIDOS_AGENT_LABEL } from '../../store/thread-events';
import { keepFocusOnPress } from '../../utils/dom';
import { ClaudeIcon, CodexIcon, LucidosMarkIcon } from '../shared/icons';

/** The backend a coding-agent menu serves: the one its row passes, else a
 *  live thread's Claude Code, else what the compose draft picked. */
export function menuCodingAgent(codingAgent: CodingAgent | null | undefined, threadId?: string, composeThreadId?: string): CodingAgent {
  return codingAgent ?? (threadId ? 'claude-code' : resolveCodingAgent(composeThreadId));
}

export function codingAgentMenuLabel(agent: CodingAgent): string {
  return `${agent === 'claude-code' ? 'Claude Code' : 'Codex'} controls`;
}

export const LUCIDOS_MENU_LABEL = `${LUCIDOS_AGENT_LABEL} model`;

/** `lucidos-commands-btn` tells the Lucidos Agent model picker from the
 *  coding-agent control button, which share the `commands-btn` base style.
 *  Surfaces that need only the coding-agent control rely on it, such as the
 *  `.commands-btn:not(.lucidos-commands-btn)` e2e selector. */
export const LUCIDOS_TRIGGER_CLASS = 'lucidos-commands-btn';

export function ControlMenuTrigger({ label, extraClass, onClick, children }: {
  label: string;
  extraClass?: string;
  onClick: (e: MouseEvent) => void;
  children: ComponentChildren;
}) {
  return (
    <button
      class={`icon-btn header-icon commands-btn${extraClass ? ` ${extraClass}` : ''}`}
      data-tooltip={tooltipWithShortcut(label, 'openAgentMenu')}
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

/** The flat mark, not `<LucidosMark/>`: this button is one of the prompt bar's
 *  gray icons, so the glyph paints in `currentColor` and carries no gradient
 *  tile. `.icon-btn.header-icon svg` sizes it. */
export function LucidosMenuIcon() {
  return <LucidosMarkIcon />;
}
