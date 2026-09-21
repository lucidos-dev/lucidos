import { Fragment } from 'preact';
import type { CodingAgent } from '../../api/types';
import { renderHeaderAction, type HeaderActionSpec } from '../layout/headerActions';
import { FollowLiveEdgeIcon } from '../shared/icons';
import { callToggleAction } from './CallToggle';
import { CodingAgentControlMenu } from './CodingAgentControlMenu';
import { LucidosControlMenu } from './LucidosControlMenu';
import { followingLiveEdge, followLiveEdgeSeed, setFollowLiveEdge } from './scrollState';

/** The row's leading controls: its anchor, and whichever of the two fixed
 *  toggles are still standing.
 *
 *  **The three slots are fixed.** The control menu is the row's anchor, the
 *  follow toggle is second and the call toggle third. What is fixed is the
 *  POSITION, so a reader reaching for one finds it in the same place on every
 *  thread. The follow toggle once sat after the indicators, which moved it
 *  under the thumb depending on what the thread happened to be doing.
 *
 *  **Only the anchor never folds.** The two toggles fold LAST, after every
 *  other member, so their slots hold at every width that can show them. That is
 *  what the pin was protecting, and a glyph off the screen protects nothing.
 *  The floor is the anchor and the send button, which fit anywhere.
 *
 *  A Fragment, deliberately: `.prompt-actions-row` is a flex row and its
 *  children are diffed positionally (see `prompt-vdom-keys.test.ts`).
 *
 *  Plan: `docs/plans/2026-09-19-the-composer-row-is-one-row.md`. */
export function PromptRowControls({
  codingAgent,
  codingAgentThreadId,
  composeThreadId,
  lucidosThreadId,
  composeContext,
  toggles,
  attrsFor,
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
  /** The fixed toggles still wearing their own box, in slot order. */
  toggles: readonly HeaderActionSpec[];
  /** The row attributes each member carries, by key. */
  attrsFor: (key: string) => Record<string, string>;
}) {
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
      {toggles.map((a) => (
        <Fragment key={a.key}>{renderHeaderAction(a, attrsFor(a.key))}</Fragment>
      ))}
    </>
  );
}

/** The two fixed toggles, in both orders the row needs them in.
 *
 *  One function, so the fold and the row cannot disagree about which of them
 *  exists. `row` is slot order, follow then call. `fold` is fold order, call
 *  then follow: the follow toggle is the only control rendering in every state,
 *  so it is the last thing to leave the row. */
export function promptRowToggles(
  codingAgent: CodingAgent | null,
  composeContext: boolean,
): { row: HeaderActionSpec[]; fold: HeaderActionSpec[] } {
  const follow = followLiveEdgeAction(composeContext);
  const call = callToggleAction(codingAgent === null);
  return {
    row: call ? [follow, call] : [follow],
    fold: call ? [call, follow] : [follow],
  };
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
    tooltip: followOn
      ? 'Following the live edge. Click to stop, and stay where you are.'
      : 'Follow the live edge: go to the newest content and stay with it as the agent writes.',
    icon: () => <FollowLiveEdgeIcon />,
    active: followOn,
    // The row paints each toggle from its own `data-role`, so this asks for the
    // bare `active` those rules select on rather than the header's frame.
    activeClass: 'active',
    onClick: () => setFollowLiveEdge(!followOn),
  };
}
