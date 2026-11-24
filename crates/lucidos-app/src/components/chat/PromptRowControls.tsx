import type { ComponentChildren } from 'preact';
import type { CodingAgent } from '../../api/types';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { focusedThreadId, threadMap } from '../../store/store';
import type { ThreadState } from '../../store/thread-events/thread-meta';
import type { HeaderActionSpec } from '../layout/headerActions';
import { SideQuestionIcon } from '../shared/icons';
import { callToggleAction } from './CallToggle';
import { pressCallToggle } from '../../store/voice';
import { callIsOffered } from '../../voice/callOffer';
import { resolveCodingAgent } from '../../store/composeSelections';
import { effectiveCodingAgentBackend } from './promptToggleMode';
import { lazyComponent } from '../../utils/lazyComponent';
import { codingAgentCommandsReady, codingAgentFoldOpenRequest, pressControlMenu } from './controlMenuRequests';
import {
  CODING_AGENT_MENU_FOLD_KEY, CodingAgentIcon, ControlMenuTrigger, codingAgentMenuLabel, menuCodingAgent,
  type CodingAgentMenuProps,
} from './ControlMenuTrigger';

function CodingAgentMenuPlaceholder({ codingAgent, threadId, composeThreadId, folded, rowAttrs }: CodingAgentMenuProps) {
  if (folded) return null;
  const agent = menuCodingAgent(codingAgent, threadId, composeThreadId);
  return (
    <div class="control-menu" {...rowAttrs}>
      <ControlMenuTrigger label={codingAgentMenuLabel(agent)} onClick={(e) => pressControlMenu('coding-agent', e.currentTarget as Element)}>
        <CodingAgentIcon agent={agent} />
      </ControlMenuTrigger>
    </div>
  );
}

export const LazyCodingAgentControlMenu = lazyComponent(
  () => import('./CodingAgentControlMenu').then((m) => m.CodingAgentControlMenu),
  CodingAgentMenuPlaceholder,
);

export function codingAgentMenuAction(agent: CodingAgent): HeaderActionSpec | null {
  if (agent !== 'claude-code') return null;
  return {
    key: CODING_AGENT_MENU_FOLD_KEY,
    dataRole: 'coding-agent-menu',
    extraClass: codingAgentCommandsReady.value ? 'commands-btn commands-btn-active' : 'commands-btn',
    label: codingAgentMenuLabel(agent),
    icon: () => <CodingAgentIcon agent={agent} />,
    onMenuClick: (anchor) => { if (anchor) codingAgentFoldOpenRequest.value = anchor; },
  };
}

export function composerRowMembers(m: {
  codingAgentMenu: HeaderActionSpec | null;
  attachImage: HeaderActionSpec;
  readouts: readonly HeaderActionSpec[];
  wipPreview: HeaderActionSpec | null;
  clearDraft: HeaderActionSpec | null;
  toggles: readonly HeaderActionSpec[];
  situational: readonly HeaderActionSpec[];
  rung: HeaderActionSpec | null;
}): { row: HeaderActionSpec[]; cluster: HeaderActionSpec[]; foldOrder: HeaderActionSpec[]; mergeAt: string } {
  const present = (a: HeaderActionSpec | null): a is HeaderActionSpec => a !== null;
  const rest = [m.codingAgentMenu, m.attachImage].filter(present);
  const tail = [m.wipPreview, m.clearDraft].filter(present);
  const cluster = [...m.situational, m.rung].filter(present);
  return {
    row: [...m.toggles, ...rest, ...m.readouts, ...tail],
    cluster,
    foldOrder: [...m.readouts, ...m.toggles, ...rest, ...tail, ...cluster],
    mergeAt: [...m.toggles, ...rest][0].key,
  };
}

export function PromptActionsRight({ cluster, action }: { cluster: ComponentChildren; action: ComponentChildren }) {
  return (
    <div class="prompt-actions-right">
      <div class="prompt-actions-cluster">{cluster}</div>
      {action}
    </div>
  );
}

export function promptRowToggles(
  codingAgent: CodingAgent | null,
  thread: ThreadState | undefined,
  sideQuestion: HeaderActionSpec | null,
): HeaderActionSpec[] {
  const call = callToggleAction(callIsOffered(thread, codingAgent));
  return [call, sideQuestion].filter((a): a is HeaderActionSpec => a !== null);
}

const SIDE_QUESTION_KEY = 'side-question';

export function sideQuestionToggleAction(on: boolean, toggle: () => void): HeaderActionSpec {
  return {
    key: SIDE_QUESTION_KEY,
    dataRole: 'side-question-toggle',
    label: on ? 'Back to a normal message' : 'Ask a side question',
    tooltip: tooltipWithShortcut(on ? 'Back to a normal message' : 'Ask a side question', 'askSideQuestion'),
    icon: () => <SideQuestionIcon />,
    active: on,
    tone: 'glyph',
    keepsFocus: true,
    onClick: toggle,
  };
}

export function isComposeContext(thread: ThreadState | undefined): boolean {
  return !thread || thread.meta.state === 'composing';
}

export function focusedThread(): ThreadState | undefined {
  const id = focusedThreadId.value;
  return id ? threadMap.value.get(id) : undefined;
}

export function pressCallToggleIfShown(): void {
  const thread = focusedThread();
  const codingAgent = effectiveCodingAgentBackend(thread, resolveCodingAgent(focusedThreadId.value));
  if (callToggleAction(callIsOffered(thread, codingAgent))) pressCallToggle();
}
