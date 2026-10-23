import type { CodingAgent } from '../../api/types';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { focusedThreadId, threadMap } from '../../store/store';
import type { ThreadState } from '../../store/thread-events/thread-meta';
import type { HeaderActionSpec } from '../layout/headerActions';
import { FollowLiveEdgeIcon } from '../shared/icons';
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
import { followingLiveEdge, followLiveEdgeSeed, setFollowLiveEdge } from './scrollState';

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
 *  classes, so it dims the same way until the commands arrive. */
export function codingAgentMenuAction(agent: CodingAgent): HeaderActionSpec {
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
 *  renders. The row takes them in this order, and so does the fold.
 *
 *  **The fold takes a prefix, so the leftmost member folds first.** The ⋯
 *  stands at the left edge, where the folded members came from. So a fold never
 *  moves a standing member across the row.
 *
 *  Nothing here reads the thread's state. So the rung pill outlasts the menu
 *  and the image in compose context and in a started thread alike.
 *
 *  Plan: `docs/plans/2026-10-21-coding-agent-menu-folds-before-the-model-picker.md`. */
export function composerRowMembers(m: {
  /** The coding-agent menu, absent on a Lucidos Agent thread. */
  codingAgentMenu: HeaderActionSpec | null;
  attachImage: HeaderActionSpec;
  /** The todo and waiting indicators and the WIP preview that are shown. */
  readouts: readonly HeaderActionSpec[];
  /** Clear draft, present only while there is a draft to clear. */
  clearDraft: HeaderActionSpec | null;
  toggles: readonly HeaderActionSpec[];
  rung: HeaderActionSpec | null;
  banner: readonly HeaderActionSpec[];
}): { row: HeaderActionSpec[]; cluster: HeaderActionSpec[] } {
  const present = (a: HeaderActionSpec | null): a is HeaderActionSpec => a !== null;
  return {
    row: [m.codingAgentMenu, m.attachImage, ...m.readouts, m.clearDraft, ...m.toggles].filter(present),
    cluster: [m.rung, ...m.banner].filter(present),
  };
}

/** The row's toggles, left to right: the call toggle where a call is offered,
 *  then the follow toggle, which renders in every state. */
export function promptRowToggles(
  codingAgent: CodingAgent | null,
  composeContext: boolean,
  thread: ThreadState | undefined,
): HeaderActionSpec[] {
  const follow = followLiveEdgeAction(composeContext);
  const call = callToggleAction(callIsOffered(thread, codingAgent));
  return call ? [call, follow] : [follow];
}

/** The follow toggle.
 *
 *  **It RENDERS the follow rather than owning it.** `followingLiveEdge` is
 *  read-only, and the reader's own scroll writes it too. So the button goes off
 *  by itself when they scroll away from a live reply.
 *
 *  It lives in the prompt area rather than on the down chevron, which cannot
 *  hold both jobs. Nor in the turn header, which repeats per turn while this is
 *  one transcript-wide mode. Being here is what lets it be armed BEFORE a send.
 *
 *  It is offered in the COMPOSE view too, and that is the point of it living
 *  here. A new thread is where a reader most reliably wants carrying through
 *  the answer. It was also the one place the follow could not be armed at all.
 *  Compose has no transcript for `followingLiveEdge` to describe. So there it
 *  shows the FOLLOW SEED, which is what the thread this compose becomes starts
 *  as, and the press writes it. Everywhere else the live flag is what shows, so
 *  the button can never sit lit over an unfollowed transcript. */
export function followLiveEdgeAction(composeContext: boolean): HeaderActionSpec {
  const followOn = composeContext ? followLiveEdgeSeed.value : followingLiveEdge.value;
  return {
    key: 'follow-live-edge',
    dataRole: 'follow-live-edge',
    label: followOn ? 'Stop following the live edge' : 'Follow the live edge',
    tooltip: tooltipWithShortcut(
      followOn
        ? 'Following the live edge. Click to stop, and stay where you are.'
        : 'Follow the live edge: go to the newest content and stay with it as the agent writes.',
      'followLiveEdge',
    ),
    icon: () => <FollowLiveEdgeIcon armed={followOn} />,
    active: followOn,
    tone: 'glyph',
    keepsFocus: true,
    onClick: () => setFollowLiveEdge(!followOn),
  };
}

/** Compose context: a focused composing draft, or the fresh compose view with
 *  no thread focused. Never an active thread. */
export function isComposeContext(thread: ThreadState | undefined): boolean {
  return !thread || thread.meta.state === 'composing';
}

function focusedThread(): ThreadState | undefined {
  const id = focusedThreadId.value;
  return id ? threadMap.value.get(id) : undefined;
}

/** The follow shortcut. It makes the press the toggle would make for the
 *  focused thread, so the two never disagree about what they light. */
export function toggleFollowLiveEdge(): void {
  setFollowLiveEdge(!followLiveEdgeAction(isComposeContext(focusedThread())).active);
}

/** The call shortcut. It presses only where the composer draws the toggle,
 *  which is the same gate `PromptInput` hands `promptRowToggles`. */
export function pressCallToggleIfShown(): void {
  const thread = focusedThread();
  const codingAgent = effectiveCodingAgentBackend(thread, resolveCodingAgent(focusedThreadId.value));
  if (callToggleAction(callIsOffered(thread, codingAgent))) pressCallToggle();
}
