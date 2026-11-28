import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { keepFocusOnPress } from '../../utils/dom';
import { FollowLiveEdgeIcon } from '../shared/icons';
import { ToggleIconButton } from '../shared/ToggleIconButton';
import { focusedThread, isComposeContext } from './PromptRowControls';
import { followingLiveEdge, followLiveEdgeSeed, setFollowLiveEdge } from './scrollState';

function followShown(): boolean {
  return isComposeContext(focusedThread()) ? followLiveEdgeSeed.value : followingLiveEdge.value;
}

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

export function toggleFollowLiveEdge(): void {
  setFollowLiveEdge(!followShown());
}
