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

/** The coding-agent menu's button, drawn until the menu's code arrives. A
 *  press is held for the menu, which takes it as it mounts (ADR 0436). */
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

/** The menu loads as the row first renders, off the first-paint set. */
export const LazyCodingAgentControlMenu = lazyComponent(
  () => import('./CodingAgentControlMenu').then((m) => m.CodingAgentControlMenu),
  CodingAgentMenuPlaceholder,
);

/** The coding-agent menu as a member of the row's fold. Folded, its ⋯ row asks
 *  the menu, which stays mounted, to open against the ⋯. It wears the button's
 *  classes, so it dims the same way until the commands arrive.
 *
 *  Claude Code alone gets one. Codex has no slash commands, and its model and
 *  effort are the rung control's, so its menu would never open. */
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

/** The composer row's foldable members, left to right, split by where each
 *  renders, plus the order the fold takes them in.
 *
 *  **The readouts fold first, then the rest left to right.** Todo and waiting
 *  only report state, so the toggles outlast them. The ⋯ stands at the left
 *  edge. A fold only removes boxes, so no standing member crosses another.
 *
 *  **The toggles lead the row on every thread.** A coding-agent thread puts
 *  its menu after them, so the call and side-question toggles stand where a
 *  Lucidos Agent thread has them.
 *
 *  **The picker stands right of the situational members.** Diff and the
 *  standing apply come and go, so they render left of it and fold before it.
 *  The picker then holds its spot beside the action button.
 *
 *  Nothing here reads the thread's state. So the rung pill outlasts the menu
 *  and the image in compose context and in a started thread alike.
 *
 *  Plans: `docs/plans/2026-10-21-coding-agent-menu-folds-before-the-model-picker.md`,
 *  `docs/plans/2026-10-25-composer-row-fixed-width-controls.md`. */
export function composerRowMembers(m: {
  /** The coding-agent menu, absent on a Lucidos Agent or Codex thread. */
  codingAgentMenu: HeaderActionSpec | null;
  attachImage: HeaderActionSpec;
  /** The todo and waiting indicators that are shown. */
  readouts: readonly HeaderActionSpec[];
  /** The WIP preview toggle, present only while it can preview. */
  wipPreview: HeaderActionSpec | null;
  /** Clear draft, present only while there is a draft to clear. */
  clearDraft: HeaderActionSpec | null;
  toggles: readonly HeaderActionSpec[];
  /** Diff and the standing apply, whichever stand. */
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
    // Diff and the standing apply merge before the first member after the
    // readouts folds. Attach image always stands, so one always exists.
    mergeAt: [...m.toggles, ...rest][0].key,
  };
}

/** The row's right-hand group: the cluster, then the action slot (Send, Apply,
 *  Archive) on the right edge.
 *
 *  The cluster gets its own box so the action slot is never its sibling.
 *  Preact can append a re-mounted member at the end of its parent, after a
 *  member that rendered `null` left the same render. Inside this box the end
 *  is still left of the action slot. The box is `display: contents`, so the
 *  gaps and the fold's measurement see the members as before. */
export function PromptActionsRight({ cluster, action }: { cluster: ComponentChildren; action: ComponentChildren }) {
  return (
    <div class="prompt-actions-right">
      <div class="prompt-actions-cluster">{cluster}</div>
      {action}
    </div>
  );
}

/** The row's toggles, left to right: the call toggle where a call is offered,
 *  then the side-question toggle where a side question can be asked. */
export function promptRowToggles(
  codingAgent: CodingAgent | null,
  thread: ThreadState | undefined,
  sideQuestion: HeaderActionSpec | null,
): HeaderActionSpec[] {
  const call = callToggleAction(callIsOffered(thread, codingAgent));
  return [call, sideQuestion].filter((a): a is HeaderActionSpec => a !== null);
}

/** The side-question toggle's fold key. */
const SIDE_QUESTION_KEY = 'side-question';

/** The side-question toggle, offered only where `sideQuestionBlocker` allows
 *  one. It renders the per-thread mode, which the shortcut, the pill's × and
 *  Escape drive too, and a press writes it through the same `toggle`. */
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

/** Compose context: a focused composing draft, or the fresh compose view with
 *  no thread focused. Never an active thread. */
export function isComposeContext(thread: ThreadState | undefined): boolean {
  return !thread || thread.meta.state === 'composing';
}

export function focusedThread(): ThreadState | undefined {
  const id = focusedThreadId.value;
  return id ? threadMap.value.get(id) : undefined;
}

/** The call shortcut. It presses only where the composer draws the toggle,
 *  which is the same gate `PromptInput` hands `promptRowToggles`. */
export function pressCallToggleIfShown(): void {
  const thread = focusedThread();
  const codingAgent = effectiveCodingAgentBackend(thread, resolveCodingAgent(focusedThreadId.value));
  if (callToggleAction(callIsOffered(thread, codingAgent))) pressCallToggle();
}
