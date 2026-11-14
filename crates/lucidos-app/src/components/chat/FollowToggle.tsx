import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { keepFocusOnPress } from '../../utils/dom';
import { FollowLiveEdgeIcon } from '../shared/icons';
import { ToggleIconButton } from '../shared/ToggleIconButton';
import { focusedThread, isComposeContext } from './PromptRowControls';
import { followingLiveEdge, followLiveEdgeSeed, setFollowLiveEdge } from './scrollState';

/** What the toggle shows for the focused thread.
 *
 *  Compose has no transcript for `followingLiveEdge` to describe. So there it
 *  shows the FOLLOW SEED, which the thread this compose becomes starts with.
 *  Everywhere else the live flag shows, so the button never sits lit over an
 *  unfollowed transcript. */
function followShown(): boolean {
  return isComposeContext(focusedThread()) ? followLiveEdgeSeed.value : followingLiveEdge.value;
}

/** The follow toggle. It floats over the composer's top edge, in the down
 *  chevron's slot, and never moves.
 *
 *  **It RENDERS the follow rather than owning it.** `followingLiveEdge` is
 *  read-only, and the reader's own scroll writes it too. So the button goes off
 *  by itself when they scroll away from a live reply.
 *
 *  It sits by the composer, so it can be armed BEFORE a send, compose view
 *  included. A new thread is where a reader most wants carrying through the
 *  answer. ADR 0064 says why the down chevron cannot hold both jobs. */
export function FollowLiveEdgeToggle() {
  const on = followShown();
  return (
    <ToggleIconButton
      class="follow-toggle"
      data-role="follow-live-edge"
      tone="glyph"
      pressed={on}
      aria-label={on ? 'Stop following the live edge' : 'Follow the live edge'}
      data-tooltip={tooltipWithShortcut(
        on
          ? 'Following the live edge. Click to stop, and stay where you are.'
          : 'Follow the live edge: go to the newest content and stay with it as the agent writes.',
        'followLiveEdge',
      )}
      onMouseDown={keepFocusOnPress}
      onClick={() => setFollowLiveEdge(!on)}
    >
      <FollowLiveEdgeIcon armed={on} />
    </ToggleIconButton>
  );
}

/** The follow shortcut. It makes the press the toggle would make, so the two
 *  never disagree about what they light. */
export function toggleFollowLiveEdge(): void {
  setFollowLiveEdge(!followShown());
}
