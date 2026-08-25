import type { CodingAgent } from '../../api/types';
import { FollowLiveEdgeIcon } from '../shared/icons';
import { CodingAgentControlMenu } from './CodingAgentControlMenu';
import { LucidosControlMenu } from './LucidosControlMenu';
import { followingLiveEdge, followLiveEdgeSeed, setFollowLiveEdge } from './scrollState';
import { TodoListIndicator } from './TodoListPanel';
import { WaitingIndicator } from './WaitingPanel';

/** The leading control cluster of `.prompt-actions-row`: whichever backend's
 *  control menu the focused thread resolves to, the follow toggle, and the
 *  indicators that belong beside them.
 *
 *  **The first two slots are FIXED, and the conditional things come after
 *  them.** The control menu is the row's anchor, and the follow toggle is
 *  second, always: it is the one control here that renders in every state, so
 *  a reader reaching for it should find it in the same place on every thread.
 *  It sat after the indicators until 2026-08-11, which put it third on a
 *  Lucidos Agent thread, second on a coding-agent one, and fourth while a
 *  subscription was armed, so the button moved under the thumb depending on
 *  what the thread happened to be doing. Everything below this pair is
 *  conditional by nature (a todo list, a wait), so it floats
 *  behind the fixed pair rather than displacing it. A new control goes AFTER
 *  the follow toggle, never between it and the menu.
 *
 *  A Fragment, deliberately: `.prompt-actions-row` is a flex row and its
 *  children are diffed positionally (see `prompt-vdom-keys.test.ts`), so this
 *  groups the cluster for reading without putting a box between the row and its
 *  buttons.
 *
 *  **The follow toggle RENDERS the follow rather than owning it**:
 *  `followingLiveEdge` is a read-only signal the reader's own scroll also
 *  writes, so the button goes off by itself when they scroll away from a live
 *  reply. It lives in the prompt area rather than on the down chevron (which
 *  cannot hold both jobs) or in the turn header (which repeats per turn, while
 *  this is one transcript-wide mode), and being here is what lets it be armed
 *  BEFORE a send, which is exactly when a reader knows they want to be carried
 *  through the answer.
 *
 *  It is rendered in the COMPOSE view too, and that is the whole point of it
 *  being here: a brand-new thread is where a reader most reliably knows they
 *  want to be carried through the answer, and it was the one place the follow
 *  could not be armed while the button was hidden. Compose has no transcript
 *  for `followingLiveEdge` to describe, so there it shows (and the press
 *  writes) the FOLLOW SEED instead, which is what the thread this compose
 *  becomes will start as. Everywhere else the live flag is what shows, so the
 *  button can never sit lit over a transcript nothing is following.
 *
 *  **The split is by WHO OWNS the control, and only two of the three are
 *  backend-specific.** The control menu obviously is. `TodoListIndicator` is
 *  too, and it is the reason the branch exists at all: `TodoListWritten` is the
 *  Lucidos Agent's own todo list, so a coding-agent thread has none to show.
 *  The *waiting indicator* is NOT, and it sits outside the branch for the
 *  same reason `await_event` has a CLI mirror: an *event wait* belongs to the
 *  thread, and both agents arm them (`system-knowhow/glossary.md` §§ "Event
 *  wait", "Waiting indicator"; ADR 0052 item 3). Sub-threads, its other half,
 *  are no more backend-specific than that.
 *
 *  It used to live inside the Lucidos arm, and that hid it from exactly the
 *  threads that use it most. `.claude/skills/e2e-lock-wait/SKILL.md` exists to
 *  make a coding-agent thread subscribe rather than poll, so on 2026-08-09 one
 *  of them armed a six-hour watch on the e2e lock, sat in **Waiting** with the
 *  dot lit, and offered nowhere to read what it was watching and no **Stop
 *  waiting** button to end it. The dot reads the projected
 *  `liveEventWaitCount`, which is right on every thread; this is the surface
 *  that says what for, so it has to be mounted on every thread too. */
export function PromptRowControls({
  codingAgent,
  codingAgentThreadId,
  composeThreadId,
  lucidosThreadId,
  composeContext,
}: {
  /** The focused thread's resolved coding-agent backend, or `null` for a
   *  Lucidos Agent thread (`effectiveCodingAgentBackend`). */
  codingAgent: CodingAgent | null;
  /** The active coding-agent session's thread id (absent for a draft). */
  codingAgentThreadId: string | undefined;
  /** The composing draft's id (absent for an active thread), which keys the
   *  per-draft model / effort / scope picks. */
  composeThreadId: string | undefined;
  lucidosThreadId: string | undefined;
  composeContext: boolean;
}) {
  const followOn = composeContext ? followLiveEdgeSeed.value : followingLiveEdge.value;
  return (
    <>
      {codingAgent !== null ? (
        <CodingAgentControlMenu
          threadId={codingAgentThreadId}
          composeThreadId={composeThreadId}
          codingAgent={codingAgent}
        />
      ) : (
        <LucidosControlMenu threadId={lucidosThreadId} composeContext={composeContext} />
      )}
      <button
        class={`icon-btn header-icon${followOn ? ' active' : ''}`}
        data-tooltip={followOn
          ? 'Following the live edge. Click to stop, and stay where you are.'
          : 'Follow the live edge: go to the newest content and stay with it as the agent writes.'}
        aria-pressed={followOn}
        aria-label={followOn ? 'Stop following the live edge' : 'Follow the live edge'}
        onClick={() => setFollowLiveEdge(!followOn)}
        data-role="follow-live-edge"
        data-row-item
      >
        <FollowLiveEdgeIcon />
      </button>
      {codingAgent === null && <TodoListIndicator />}
      <WaitingIndicator />
    </>
  );
}
